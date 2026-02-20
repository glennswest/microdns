use chrono::{DateTime, Utc};
use crate::proto;
use microdns_core::config::{PeerConfig, ReplicationConfig};
use microdns_core::db::Db;
use microdns_core::types::{Record, RecordData, ReplicationMeta, SoaData, Zone};
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::watch;
use tonic::transport::Channel;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Periodically pulls zones and records from peers via gRPC.
pub struct ReplicationAgent {
    instance_id: String,
    db: Db,
    peers: Vec<PeerConfig>,
    config: ReplicationConfig,
}

impl ReplicationAgent {
    pub fn new(
        instance_id: &str,
        db: Db,
        peers: Vec<PeerConfig>,
        config: ReplicationConfig,
    ) -> Self {
        Self {
            instance_id: instance_id.to_string(),
            db,
            peers,
            config,
        }
    }

    pub async fn run(&self, shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
        info!(
            instance_id = %self.instance_id,
            peer_count = self.peers.len(),
            pull_interval = self.config.pull_interval_secs,
            "replication agent started"
        );

        let mut interval =
            tokio::time::interval(Duration::from_secs(self.config.pull_interval_secs));
        let mut shutdown = shutdown;

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.sync_all_peers().await;
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!(instance_id = %self.instance_id, "replication agent shutting down");
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    async fn sync_all_peers(&self) {
        for peer in &self.peers {
            if let Err(e) = self.sync_peer(peer).await {
                let stale_zones = self.db.get_zones_for_peer(&peer.id).unwrap_or_default();
                let stale_count = stale_zones
                    .iter()
                    .filter(|m| {
                        let age = Utc::now()
                            .signed_duration_since(m.last_synced)
                            .num_seconds();
                        age > self.config.stale_threshold_secs as i64
                    })
                    .count();

                if stale_count > 0 {
                    warn!(
                        peer = %peer.id,
                        stale_zones = stale_count,
                        error = %e,
                        "peer unreachable, serving stale data"
                    );
                } else {
                    warn!(peer = %peer.id, error = %e, "failed to sync from peer");
                }
            }
        }
    }

    async fn sync_peer(&self, peer: &PeerConfig) -> anyhow::Result<()> {
        let endpoint = format!("http://{}:{}", peer.addr, peer.grpc_port);
        let channel = Channel::from_shared(endpoint.clone())?
            .timeout(Duration::from_secs(self.config.peer_timeout_secs))
            .connect_timeout(Duration::from_secs(self.config.peer_timeout_secs))
            .connect()
            .await?;

        debug!(peer = %peer.id, endpoint = %endpoint, "connected to peer");

        // List zones from peer
        let mut zone_client =
            proto::zone_service_client::ZoneServiceClient::new(channel.clone());
        let zones_resp = zone_client
            .list_zones(proto::ListZonesRequest {})
            .await?
            .into_inner();

        let mut seen_zone_ids = HashSet::new();

        for proto_zone in &zones_resp.zones {
            let zone_id: Uuid = match proto_zone.id.parse() {
                Ok(id) => id,
                Err(_) => {
                    warn!(
                        peer = %peer.id,
                        zone_id = %proto_zone.id,
                        "skipping zone with invalid UUID"
                    );
                    continue;
                }
            };

            seen_zone_ids.insert(zone_id);

            // Check if we need to sync this zone
            let remote_serial = proto_zone
                .soa
                .as_ref()
                .map(|s| s.serial)
                .unwrap_or(0);

            let needs_sync = match self.db.get_replication_meta(&zone_id) {
                Ok(Some(meta)) => meta.source_serial < remote_serial,
                Ok(None) => true,
                Err(e) => {
                    error!(zone_id = %zone_id, error = %e, "failed to read replication meta");
                    true
                }
            };

            if !needs_sync {
                debug!(
                    peer = %peer.id,
                    zone = %proto_zone.name,
                    serial = remote_serial,
                    "zone up to date, skipping"
                );
                // Still update last_synced timestamp
                if let Ok(Some(mut meta)) = self.db.get_replication_meta(&zone_id) {
                    meta.last_synced = Utc::now();
                    let _ = self.db.set_replication_meta(&meta);
                }
                continue;
            }

            // Fetch records for this zone
            let mut record_client =
                proto::record_service_client::RecordServiceClient::new(channel.clone());
            let records_resp = record_client
                .list_records(proto::ListRecordsRequest {
                    zone_id: proto_zone.id.clone(),
                })
                .await?
                .into_inner();

            // Convert proto types to domain types
            let zone = proto_zone_to_domain(proto_zone)?;
            let records: Vec<Record> = records_resp
                .records
                .iter()
                .filter_map(|r| match proto_record_to_domain(r) {
                    Ok(rec) => Some(rec),
                    Err(e) => {
                        warn!(
                            record_id = %r.id,
                            error = %e,
                            "skipping record with conversion error"
                        );
                        None
                    }
                })
                .collect();

            // Upsert zone and replace records
            self.db.upsert_zone(&zone)?;
            self.db.replace_zone_records(&zone_id, &records)?;

            // Update replication metadata
            let meta = ReplicationMeta {
                zone_id,
                zone_name: zone.name.clone(),
                source_peer_id: peer.id.clone(),
                last_synced: Utc::now(),
                source_serial: remote_serial,
            };
            self.db.set_replication_meta(&meta)?;

            info!(
                peer = %peer.id,
                zone = %zone.name,
                serial = remote_serial,
                records = records.len(),
                "replicated zone"
            );
        }

        // Clean up zones this peer no longer serves
        let peer_zones = self.db.get_zones_for_peer(&peer.id)?;
        for meta in peer_zones {
            if !seen_zone_ids.contains(&meta.zone_id) {
                info!(
                    peer = %peer.id,
                    zone = %meta.zone_name,
                    "removing zone no longer served by peer"
                );
                if let Err(e) = self.db.delete_replicated_zone(&meta.zone_id) {
                    error!(
                        zone_id = %meta.zone_id,
                        error = %e,
                        "failed to delete replicated zone"
                    );
                }
            }
        }

        Ok(())
    }
}

fn proto_zone_to_domain(pz: &proto::Zone) -> anyhow::Result<Zone> {
    let id: Uuid = pz.id.parse()?;
    let soa = pz
        .soa
        .as_ref()
        .map(|s| SoaData {
            mname: s.mname.clone(),
            rname: s.rname.clone(),
            serial: s.serial,
            refresh: s.refresh,
            retry: s.retry,
            expire: s.expire,
            minimum: s.minimum,
        })
        .unwrap_or_else(|| SoaData {
            mname: String::new(),
            rname: String::new(),
            serial: 0,
            refresh: 3600,
            retry: 900,
            expire: 604800,
            minimum: 300,
        });

    let created_at = parse_timestamp(&pz.created_at)?;
    let updated_at = parse_timestamp(&pz.updated_at)?;

    Ok(Zone {
        id,
        name: pz.name.clone(),
        soa,
        default_ttl: pz.default_ttl,
        created_at,
        updated_at,
    })
}

fn proto_record_to_domain(pr: &proto::Record) -> anyhow::Result<Record> {
    let id: Uuid = pr.id.parse()?;
    let zone_id: Uuid = pr.zone_id.parse()?;
    let data: RecordData = serde_json::from_str(&pr.data_json)?;
    let created_at = parse_timestamp(&pr.created_at)?;
    let updated_at = parse_timestamp(&pr.updated_at)?;

    Ok(Record {
        id,
        zone_id,
        name: pr.name.clone(),
        ttl: pr.ttl,
        data,
        enabled: pr.enabled,
        health_check: None,
        created_at,
        updated_at,
    })
}

fn parse_timestamp(s: &str) -> anyhow::Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(s)?.with_timezone(&Utc))
}
