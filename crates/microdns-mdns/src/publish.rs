//! Converge the publish zone with what the cache currently holds.
//!
//! Unlike the Kubernetes source, this one cannot replace the zone wholesale:
//! the zone may also hold records an operator curated by hand, and a discovered
//! name must never quietly delete or shadow one. So the reconcile is a diff
//! over the records this source owns (`RecordSource::Mdns`) and nothing else —
//! records from any other source are read, respected, and left alone.

use chrono::Utc;
use microdns_core::db::Db;
use microdns_core::reverse::is_reverse_zone;
use microdns_core::types::{Record, RecordSource, RecordType, SoaData, Zone};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::config::MdnsConfig;
use crate::translate::DesiredRecord;

/// What one reconcile pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Applied {
    pub created: usize,
    pub updated: usize,
    pub deleted: usize,
    /// Discovered names skipped because a curated record already owns the name.
    pub shadowed: usize,
}

impl Applied {
    pub fn changed(&self) -> bool {
        self.created + self.updated + self.deleted > 0
    }
}

/// Owns the publish zone and applies desired record sets to it.
pub struct Publisher {
    db: Db,
    zone_id: Uuid,
    zone_name: String,
    /// This instance's id, stamped on what it registers.
    instance_id: String,
    /// Names already reported as shadowed, so the warning is logged once per
    /// name rather than on every reconcile.
    warned: std::collections::HashSet<(String, RecordType)>,
}

impl Publisher {
    /// Open — creating if needed — the zone discovered names are published to.
    pub fn new(db: Db, config: &MdnsConfig, instance_id: &str) -> anyhow::Result<Self> {
        let zone_name = config.zone.trim_end_matches('.').to_lowercase();
        if zone_name.is_empty() {
            anyhow::bail!("mdns: publish zone must not be empty");
        }
        if is_reverse_zone(&zone_name) {
            anyhow::bail!("mdns: publish zone {zone_name} is a reverse zone; forward names cannot be published into it");
        }

        let zone = ensure_zone(&db, &zone_name, config.ttl_max)?;
        Ok(Self {
            db,
            zone_id: zone.id,
            zone_name,
            instance_id: instance_id.to_string(),
            warned: std::collections::HashSet::new(),
        })
    }

    pub fn zone_name(&self) -> &str {
        &self.zone_name
    }

    /// Make the zone match `desired`, touching only records this source owns.
    ///
    /// `prune` is what makes a restart safe: for the first moments after start
    /// the cache is empty simply because nothing has announced yet, and
    /// deleting on that basis would drop every discovered name and re-add it
    /// seconds later. Publishing runs from the first packet; withdrawing waits
    /// until the caller says the cache has had a chance to fill.
    pub fn apply(&mut self, desired: &[DesiredRecord], prune: bool) -> anyhow::Result<Applied> {
        let existing = self.db.list_records(&self.zone_id)?;
        // Mine = discovered *and* registered by this instance. Several
        // instances write into one shared zone, so `source` alone would have
        // this one prune names another segment is responsible for — including,
        // on the instance holding the zone, every name it did not hear itself.
        // `None` counts as mine for records written before origin existed.
        let (mine, theirs): (Vec<Record>, Vec<Record>) = existing.into_iter().partition(|r| {
            r.source == RecordSource::Mdns
                && r.origin.as_deref().is_none_or(|o| o == self.instance_id)
        });

        let mut applied = Applied::default();
        let mut keep: Vec<Uuid> = Vec::new();
        let now = Utc::now();

        for want in desired {
            // A curated record on the same name and type wins: publishing ours
            // alongside it would silently merge two answers into one RRset.
            if let Some(owner) = theirs.iter().find(|r| {
                r.name == want.name
                    && r.data.record_type() == want.record_type()
                    && r.source != RecordSource::Mdns
            }) {
                applied.shadowed += 1;
                let key = (want.name.clone(), want.record_type());
                if self.warned.insert(key) {
                    warn!(
                        "mdns: not publishing {}.{} {} — a {} record already owns that name",
                        want.name,
                        self.zone_name,
                        want.record_type(),
                        owner.source
                    );
                }
                continue;
            }

            match mine
                .iter()
                .find(|r| r.name == want.name && r.data == want.data)
            {
                Some(current) => {
                    keep.push(current.id);
                    if current.ttl != want.ttl || !current.enabled {
                        let mut updated = current.clone();
                        updated.ttl = want.ttl;
                        updated.enabled = true;
                        updated.updated_at = now;
                        self.db.update_record(&updated)?;
                        applied.updated += 1;
                    }
                }
                None => {
                    let record = Record {
                        id: Uuid::new_v4(),
                        zone_id: self.zone_id,
                        name: want.name.clone(),
                        ttl: want.ttl,
                        data: want.data.clone(),
                        enabled: true,
                        health_check: None,
                        source: RecordSource::Mdns,
                        origin: Some(self.instance_id.clone()),
                        created_at: now,
                        updated_at: now,
                    };
                    self.db.create_record(&record)?;
                    applied.created += 1;
                    debug!(
                        "mdns: published {}.{} {}",
                        record.name,
                        self.zone_name,
                        record.data.record_type()
                    );
                }
            }
        }

        // Anything of ours the cache no longer holds has gone away — a goodbye,
        // or a device that stopped answering until its TTL ran out.
        if prune {
            for record in &mine {
                if keep.contains(&record.id) {
                    continue;
                }
                self.db.delete_record(&record.id)?;
                applied.deleted += 1;
                debug!(
                    "mdns: withdrew {}.{} {}",
                    record.name,
                    self.zone_name,
                    record.data.record_type()
                );
            }
        }

        if applied.changed() {
            self.db.increment_soa_serial(&self.zone_id)?;
            info!(
                "mdns: zone {} updated (+{} ~{} -{})",
                self.zone_name, applied.created, applied.updated, applied.deleted
            );
        }
        Ok(applied)
    }

    /// Remove every record this source published, leaving the zone's curated
    /// records untouched. Used when the source is turned off or re-homed to a
    /// different zone: a discovered name must not outlive the source that
    /// vouched for it.
    pub fn withdraw_all(&self) -> anyhow::Result<usize> {
        let mut removed = 0;
        for record in self.db.list_records(&self.zone_id)? {
            if record.source == RecordSource::Mdns
                && record.origin.as_deref().is_none_or(|o| o == self.instance_id)
            {
                self.db.delete_record(&record.id)?;
                removed += 1;
            }
        }
        if removed > 0 {
            self.db.increment_soa_serial(&self.zone_id)?;
        }
        Ok(removed)
    }
}

/// Create the publish zone with a sane SOA if it does not already exist.
fn ensure_zone(db: &Db, zone_name: &str, default_ttl: u32) -> anyhow::Result<Zone> {
    if let Some(zone) = db.get_zone_by_name(zone_name)? {
        return Ok(zone);
    }
    let now = Utc::now();
    let zone = Zone {
        id: Uuid::new_v4(),
        name: zone_name.to_string(),
        soa: SoaData {
            mname: format!("ns1.{zone_name}"),
            rname: format!("admin.{zone_name}"),
            serial: now.format("%Y%m%d00").to_string().parse().unwrap_or(1),
            refresh: 3600,
            retry: 900,
            expire: 604800,
            minimum: default_ttl,
        },
        default_ttl,
        created_at: now,
        updated_at: now,
    };
    db.create_zone(zone_name, &zone)?;
    info!("mdns: created publish zone {zone_name}");
    Ok(zone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use microdns_core::types::RecordData;

    fn test_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("test.redb")).unwrap();
        (db, dir)
    }

    fn config() -> MdnsConfig {
        MdnsConfig {
            zone: "mdns.g9.lo".into(),
            ..Default::default()
        }
    }

    fn want(name: &str, ip: &str) -> DesiredRecord {
        DesiredRecord {
            name: name.into(),
            ttl: 120,
            data: RecordData::A(ip.parse().unwrap()),
        }
    }

    #[test]
    fn publishing_is_idempotent_and_prunes_what_went_away() {
        let (db, _dir) = test_db();
        let mut publisher = Publisher::new(db.clone(), &config(), "test").unwrap();

        let applied = publisher.apply(&[want("tracker", "192.168.9.134")], true).unwrap();
        assert_eq!(applied.created, 1);
        assert_eq!(
            db.query_fqdn("tracker.mdns.g9.lo", RecordType::A).unwrap().len(),
            1
        );

        // Same cache contents again: no churn, no duplicate.
        let applied = publisher.apply(&[want("tracker", "192.168.9.134")], true).unwrap();
        assert_eq!(applied, Applied::default());
        assert_eq!(
            db.query_fqdn("tracker.mdns.g9.lo", RecordType::A).unwrap().len(),
            1
        );

        // Device goes away: its record goes with it.
        let applied = publisher.apply(&[], true).unwrap();
        assert_eq!(applied.deleted, 1);
        assert!(db
            .query_fqdn("tracker.mdns.g9.lo", RecordType::A)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_changed_address_replaces_the_old_one() {
        let (db, _dir) = test_db();
        let mut publisher = Publisher::new(db.clone(), &config(), "test").unwrap();

        publisher.apply(&[want("tracker", "192.168.9.134")], true).unwrap();
        let applied = publisher.apply(&[want("tracker", "192.168.9.200")], true).unwrap();
        assert_eq!(applied.created, 1);
        assert_eq!(applied.deleted, 1);

        let records = db.query_fqdn("tracker.mdns.g9.lo", RecordType::A).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].data, RecordData::A("192.168.9.200".parse().unwrap()));
    }

    #[test]
    fn a_ttl_change_updates_in_place_rather_than_recreating() {
        let (db, _dir) = test_db();
        let mut publisher = Publisher::new(db.clone(), &config(), "test").unwrap();

        publisher.apply(&[want("tracker", "192.168.9.134")], true).unwrap();
        let id = db.query_fqdn("tracker.mdns.g9.lo", RecordType::A).unwrap()[0].id;

        let mut longer = want("tracker", "192.168.9.134");
        longer.ttl = 600;
        let applied = publisher.apply(&[longer], true).unwrap();
        assert_eq!(applied.updated, 1);

        let records = db.query_fqdn("tracker.mdns.g9.lo", RecordType::A).unwrap();
        assert_eq!(records[0].id, id, "record identity should survive a TTL bump");
        assert_eq!(records[0].ttl, 600);
    }

    #[test]
    fn a_curated_record_is_never_shadowed_or_deleted() {
        let (db, _dir) = test_db();
        let mut publisher = Publisher::new(db.clone(), &config(), "test").unwrap();
        let zone = db.get_zone_by_name("mdns.g9.lo").unwrap().unwrap();

        // An operator pinned this name by hand.
        let curated = Record {
            id: Uuid::new_v4(),
            zone_id: zone.id,
            name: "tracker".into(),
            ttl: 300,
            data: RecordData::A("10.0.0.1".parse().unwrap()),
            enabled: true,
            health_check: None,
            source: RecordSource::Manual,
            origin: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        db.create_record(&curated).unwrap();

        let applied = publisher.apply(&[want("tracker", "192.168.9.134")], true).unwrap();
        assert_eq!(applied.shadowed, 1);
        assert_eq!(applied.created, 0);

        let records = db.query_fqdn("tracker.mdns.g9.lo", RecordType::A).unwrap();
        assert_eq!(records.len(), 1, "the curated record must stand alone");
        assert_eq!(records[0].data, RecordData::A("10.0.0.1".parse().unwrap()));

        // And an empty cache must not sweep it up either.
        publisher.apply(&[], true).unwrap();
        assert_eq!(
            db.query_fqdn("tracker.mdns.g9.lo", RecordType::A).unwrap().len(),
            1
        );
    }

    #[test]
    fn nothing_is_withdrawn_while_the_cache_is_still_filling() {
        let (db, _dir) = test_db();
        let mut publisher = Publisher::new(db.clone(), &config(), "test").unwrap();
        publisher
            .apply(&[want("tracker", "192.168.9.134")], true)
            .unwrap();

        // A restart starts with an empty cache; without the grace window this
        // would drop the name and re-add it seconds later.
        let applied = publisher.apply(&[], false).unwrap();
        assert_eq!(applied.deleted, 0);
        assert_eq!(
            db.query_fqdn("tracker.mdns.g9.lo", RecordType::A)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn another_instances_discovered_names_are_left_alone() {
        let (db, _dir) = test_db();
        let zone_name = config().zone.clone();
        let mut publisher = Publisher::new(db.clone(), &config(), "g9").unwrap();
        let zone = db.get_zone_by_name(&zone_name).unwrap().unwrap();

        // A name another instance heard on its own segment and registered here.
        db.create_record(&Record {
            id: Uuid::new_v4(),
            zone_id: zone.id,
            name: "printer".into(),
            ttl: 120,
            data: RecordData::A("192.168.8.20".parse().unwrap()),
            enabled: true,
            health_check: None,
            source: RecordSource::Mdns,
            origin: Some("g8".into()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();

        // This instance hears nothing at all. Before ownership was tracked this
        // wiped the zone, since every discovered record looked like its own.
        let applied = publisher.apply(&[], true).unwrap();
        assert_eq!(applied.deleted, 0);
        assert_eq!(db.list_records(&zone.id).unwrap().len(), 1);

        // Its own names are still withdrawn normally.
        publisher.apply(&[want("mine", "192.168.9.5")], true).unwrap();
        assert_eq!(db.list_records(&zone.id).unwrap().len(), 2);
        let applied = publisher.apply(&[], true).unwrap();
        assert_eq!(applied.deleted, 1);
        let left = db.list_records(&zone.id).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].origin.as_deref(), Some("g8"));
    }

    #[test]
    fn registered_names_carry_the_instance_that_heard_them() {
        let (db, _dir) = test_db();
        let mut publisher = Publisher::new(db.clone(), &config(), "g9").unwrap();
        publisher.apply(&[want("tracker", "192.168.9.134")], true).unwrap();

        let records = db.query_fqdn("tracker.mdns.g9.lo", RecordType::A).unwrap();
        assert_eq!(records[0].origin.as_deref(), Some("g9"));
        assert_eq!(records[0].source, RecordSource::Mdns);
    }

    #[test]
    fn a_reverse_zone_is_rejected_as_a_publish_target() {
        let (db, _dir) = test_db();
        let cfg = MdnsConfig {
            zone: "9.168.192.in-addr.arpa".into(),
            ..Default::default()
        };
        assert!(Publisher::new(db, &cfg, "test").is_err());
    }
}
