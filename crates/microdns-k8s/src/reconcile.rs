//! Converge the managed cluster zone (and its reverse zones) with the desired
//! record set computed by [`crate::translate`].
//!
//! The forward zone is replaced atomically via [`Db::replace_zone_records`],
//! which makes the whole thing idempotent — reconciling twice with the same
//! desired set is a no-op observable to clients. PTR records live in separate
//! reverse zones, so they are reconciled here with a small in-memory diff.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

use chrono::Utc;
use microdns_core::db::Db;
use microdns_core::reverse;
use microdns_core::types::{Record, RecordData, SoaData, Zone};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::config::K8sConfig;
use crate::translate::DesiredRecord;

pub struct Reconciler {
    db: Arc<Db>,
    zone_id: Uuid,
    cluster_domain: String,
    default_ttl: u32,
    manage_ptr: bool,
    /// IP → owning record name for every PTR we have synced, so we can prune
    /// PTRs whose forward record went away.
    synced_ptrs: HashMap<IpAddr, String>,
}

impl Reconciler {
    /// Open (creating if needed) the managed cluster zone and return a ready
    /// reconciler.
    pub fn new(db: Arc<Db>, config: &K8sConfig) -> anyhow::Result<Self> {
        let zone = ensure_cluster_zone(&db, &config.cluster_domain, config.default_ttl)?;
        Ok(Self {
            db,
            zone_id: zone.id,
            cluster_domain: config.cluster_domain.clone(),
            default_ttl: config.default_ttl,
            manage_ptr: config.manage_ptr,
            synced_ptrs: HashMap::new(),
        })
    }

    /// Replace the forward zone with `desired` and reconcile reverse PTRs.
    pub fn apply(&mut self, desired: Vec<DesiredRecord>) -> anyhow::Result<()> {
        let now = Utc::now();
        let records: Vec<Record> = desired
            .iter()
            .map(|d| Record {
                id: Uuid::new_v4(),
                zone_id: self.zone_id,
                name: d.name.clone(),
                ttl: d.ttl,
                data: d.data.clone(),
                enabled: true,
                health_check: None,
                created_at: now,
                updated_at: now,
            })
            .collect();

        self.db.replace_zone_records(&self.zone_id, &records)?;
        self.db.increment_soa_serial(&self.zone_id)?;
        debug!(
            "reconciled cluster zone {} — {} forward records",
            self.cluster_domain,
            records.len()
        );

        if self.manage_ptr {
            self.reconcile_ptrs(&desired);
        }
        Ok(())
    }

    /// Sync one PTR per forward A/AAAA IP and delete PTRs for IPs that vanished.
    fn reconcile_ptrs(&mut self, desired: &[DesiredRecord]) {
        // Desired IP → owning forward record name (first writer wins; one PTR/IP).
        let mut want: HashMap<IpAddr, String> = HashMap::new();
        for d in desired {
            let ip = match &d.data {
                RecordData::A(v4) => IpAddr::V4(*v4),
                RecordData::AAAA(v6) => IpAddr::V6(*v6),
                _ => continue,
            };
            want.entry(ip).or_insert_with(|| d.name.clone());
        }

        // Upserts (idempotent when unchanged).
        for (ip, name) in &want {
            let res = match ip {
                IpAddr::V4(v4) => reverse::sync_ptr_for_a(
                    &self.db,
                    name,
                    &self.cluster_domain,
                    *v4,
                    self.default_ttl,
                ),
                IpAddr::V6(v6) => reverse::sync_ptr_for_aaaa(
                    &self.db,
                    name,
                    &self.cluster_domain,
                    *v6,
                    self.default_ttl,
                ),
            };
            if let Err(e) = res {
                warn!("failed to sync PTR for {ip}: {e}");
            }
        }

        // Prune PTRs for IPs no longer present.
        for (ip, name) in &self.synced_ptrs {
            if want.contains_key(ip) {
                continue;
            }
            let res = match ip {
                IpAddr::V4(v4) => {
                    reverse::delete_ptr_for_a(&self.db, name, &self.cluster_domain, *v4)
                }
                IpAddr::V6(v6) => {
                    reverse::delete_ptr_for_aaaa(&self.db, name, &self.cluster_domain, *v6)
                }
            };
            if let Err(e) = res {
                warn!("failed to delete stale PTR for {ip}: {e}");
            }
        }

        self.synced_ptrs = want;
    }
}

/// Create the cluster zone with a sane SOA if it does not already exist.
fn ensure_cluster_zone(db: &Db, cluster_domain: &str, default_ttl: u32) -> anyhow::Result<Zone> {
    if let Some(zone) = db.get_zone_by_name(cluster_domain)? {
        return Ok(zone);
    }
    let now = Utc::now();
    let zone = Zone {
        id: Uuid::new_v4(),
        name: cluster_domain.to_string(),
        soa: SoaData {
            mname: format!("ns1.{cluster_domain}"),
            rname: format!("admin.{cluster_domain}"),
            serial: now
                .format("%Y%m%d00")
                .to_string()
                .parse()
                .unwrap_or(1),
            refresh: 3600,
            retry: 900,
            expire: 604800,
            minimum: default_ttl,
        },
        default_ttl,
        created_at: now,
        updated_at: now,
    };
    db.create_zone(cluster_domain, &zone)?;
    debug!("created managed cluster zone: {cluster_domain}");
    Ok(zone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::translate::DesiredRecord;
    use microdns_core::types::RecordType;

    fn test_db() -> Arc<Db> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");
        // Keep the tempdir alive for the process — leak is fine in a unit test.
        std::mem::forget(dir);
        Arc::new(Db::open(&path).unwrap())
    }

    fn cfg() -> K8sConfig {
        K8sConfig {
            cluster_domain: "cluster.local".into(),
            default_ttl: 30,
            manage_ptr: true,
            ..Default::default()
        }
    }

    #[test]
    fn apply_is_idempotent_and_prunes() {
        let db = test_db();
        let mut r = Reconciler::new(db.clone(), &cfg()).unwrap();

        let rec = |name: &str, ip: &str| DesiredRecord {
            name: name.into(),
            ttl: 30,
            data: RecordData::A(ip.parse().unwrap()),
        };

        // First apply: one service record.
        r.apply(vec![rec("kubernetes.default.svc", "10.96.0.1")])
            .unwrap();
        let got = db
            .query_fqdn("kubernetes.default.svc.cluster.local", RecordType::A)
            .unwrap();
        assert_eq!(got.len(), 1);

        // Re-apply identical: still exactly one (idempotent, no dupes).
        r.apply(vec![rec("kubernetes.default.svc", "10.96.0.1")])
            .unwrap();
        let got = db
            .query_fqdn("kubernetes.default.svc.cluster.local", RecordType::A)
            .unwrap();
        assert_eq!(got.len(), 1);

        // PTR was created for the IP.
        let ptr = db
            .query_fqdn("1.0.96.10.in-addr.arpa", RecordType::PTR)
            .unwrap();
        assert_eq!(ptr.len(), 1);

        // Apply an empty set: forward record and its PTR are both pruned.
        r.apply(vec![]).unwrap();
        let got = db
            .query_fqdn("kubernetes.default.svc.cluster.local", RecordType::A)
            .unwrap();
        assert!(got.is_empty());
        let ptr = db
            .query_fqdn("1.0.96.10.in-addr.arpa", RecordType::PTR)
            .unwrap();
        assert!(ptr.is_empty());
    }
}
