use crate::probe;
use crate::state::HealthState;
use microdns_core::db::Db;
use microdns_core::types::{ProbeType, RecordData};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, Mutex};
use tracing::{error, info, warn};

/// The health check monitor. Periodically scans all records with health checks
/// and runs probes to update their enabled/disabled state.
pub struct HealthMonitor {
    db: Db,
    state: Arc<Mutex<HealthState>>,
    check_interval: Duration,
    default_probe: ProbeType,
}

impl HealthMonitor {
    pub fn new(
        db: Db,
        check_interval: Duration,
        default_probe: ProbeType,
    ) -> Self {
        Self {
            db,
            state: Arc::new(Mutex::new(HealthState::new())),
            check_interval,
            default_probe,
        }
    }

    pub async fn run(self, shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
        info!(
            "health monitor started, interval: {:?}, default probe: {:?}",
            self.check_interval, self.default_probe
        );

        let mut shutdown = shutdown;
        let mut interval = tokio::time::interval(self.check_interval);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = self.run_check_cycle().await {
                        error!("health check cycle error: {e}");
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("health monitor shutting down");
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    /// Run one cycle of health checks across all zones and records.
    async fn run_check_cycle(&self) -> anyhow::Result<()> {
        let zones = self.db.list_zones()?;

        for zone in &zones {
            let records = self.db.list_records(&zone.id)?;

            for record in &records {
                let health_check = match &record.health_check {
                    Some(hc) => hc,
                    None => continue,
                };

                // Extract target IP from record data
                let target_ip = match &record.data {
                    RecordData::A(addr) => IpAddr::V4(*addr),
                    RecordData::AAAA(addr) => IpAddr::V6(*addr),
                    _ => continue, // Only A/AAAA records can be health-checked
                };

                let probe_type = health_check.probe_type;
                let timeout = Duration::from_secs(health_check.timeout_secs as u64);
                let endpoint = health_check.endpoint.as_deref();

                // Register record in state tracker
                {
                    let mut state = self.state.lock().await;
                    state.register(
                        record.id,
                        health_check.healthy_threshold,
                        health_check.unhealthy_threshold,
                        record.zone_id,
                        record.name.clone(),
                        record.data.record_type().to_string(),
                    );
                }

                // Run probe
                let result = probe::run_probe(probe_type, target_ip, timeout, endpoint).await;

                // Update state
                let state_changed = {
                    let mut state = self.state.lock().await;
                    state.record_probe_result(&record.id, result.success)
                };

                // If state changed, update the record in the database
                if let Some(new_healthy) = state_changed {
                    info!(
                        "record {} ({}.{}) health changed to {}",
                        record.id,
                        record.name,
                        zone.name,
                        if new_healthy { "HEALTHY" } else { "UNHEALTHY" }
                    );

                    let mut updated = record.clone();
                    updated.enabled = new_healthy;
                    if let Err(e) = self.db.update_record(&updated) {
                        error!("failed to update record {} enabled state: {e}", record.id);
                    }
                }
            }
        }

        // Check failsafe: if all records for a name are down, force-enable one
        self.apply_failsafe().await?;

        Ok(())
    }

    /// Apply failsafe: if all records for a (zone, name, type) group are unhealthy,
    /// force-enable one to maintain availability.
    async fn apply_failsafe(&self) -> anyhow::Result<()> {
        let failsafe_ids = {
            let state = self.state.lock().await;
            state.failsafe_records()
        };

        for record_id in failsafe_ids {
            if let Ok(Some(mut record)) = self.db.get_record(&record_id) {
                if !record.enabled {
                    warn!(
                        "failsafe: force-enabling record {} ({}) - all peers unhealthy",
                        record_id, record.name
                    );
                    record.enabled = true;
                    if let Err(e) = self.db.update_record(&record) {
                        error!("failsafe: failed to enable record {record_id}: {e}");
                    }
                }
            }
        }

        Ok(())
    }

    pub fn state(&self) -> &Arc<Mutex<HealthState>> {
        &self.state
    }
}
