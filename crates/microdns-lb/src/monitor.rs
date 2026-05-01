use crate::probe;
use crate::state::{HealthState, RecordHealth};
use chrono::Utc;
use futures::stream::{FuturesUnordered, StreamExt};
use microdns_core::db::Db;
use microdns_core::types::{HealthStatus, ProbeType, RecordData};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch, Mutex, Semaphore};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Tunables for the LB monitor. Mirrors `DnsLbConfig` plus a few runtime
/// extras the API surface needs.
#[derive(Debug, Clone)]
pub struct MonitorConfig {
    pub check_interval: Duration,
    pub default_probe: ProbeType,
    pub default_timeout: Duration,
    pub probe_concurrency: usize,
    pub ping_packet_count: u8,
}

impl MonitorConfig {
    pub fn default_for(probe: ProbeType) -> Self {
        Self {
            check_interval: Duration::from_secs(10),
            default_probe: probe,
            default_timeout: Duration::from_secs(5),
            probe_concurrency: 32,
            ping_packet_count: 3,
        }
    }
}

/// One published state-change event. Broadcast on the monitor's channel and
/// consumed by the dashboard WebSocket / API layer.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StateChange {
    pub record_id: Uuid,
    pub zone_id: Uuid,
    pub zone_name: String,
    pub name: String,
    pub ip: String,
    pub record_type: String,
    pub status: HealthStatus,
    pub failsafe: bool,
    pub probe_type: ProbeType,
    pub detail: String,
    pub at: chrono::DateTime<Utc>,
}

/// The health-check monitor. Runs a probe cycle every `check_interval`
/// and updates record `enabled` flags + persists the latest health view.
pub struct HealthMonitor {
    db: Db,
    config: MonitorConfig,
    state: Arc<Mutex<HealthState>>,
    events: broadcast::Sender<StateChange>,
}

impl HealthMonitor {
    pub fn new(db: Db, check_interval: Duration, default_probe: ProbeType) -> Self {
        Self::with_config(
            db,
            MonitorConfig {
                check_interval,
                default_probe,
                default_timeout: Duration::from_secs(5),
                probe_concurrency: 32,
                ping_packet_count: 3,
            },
        )
    }

    pub fn with_config(db: Db, config: MonitorConfig) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            db,
            config,
            state: Arc::new(Mutex::new(HealthState::new())),
            events,
        }
    }

    pub fn state(&self) -> Arc<Mutex<HealthState>> {
        self.state.clone()
    }

    pub fn config(&self) -> &MonitorConfig {
        &self.config
    }

    pub fn subscribe(&self) -> broadcast::Receiver<StateChange> {
        self.events.subscribe()
    }

    pub fn events(&self) -> broadcast::Sender<StateChange> {
        self.events.clone()
    }

    /// Run a single probe cycle out-of-band (used by REST one-shot probe
    /// and tests). Returns the number of records probed.
    pub async fn run_one_cycle(&self) -> anyhow::Result<usize> {
        self.run_check_cycle().await
    }

    pub async fn run(self, shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
        info!(
            "health monitor starting: interval={:?} default_probe={:?} concurrency={} ping_count={}",
            self.config.check_interval,
            self.config.default_probe,
            self.config.probe_concurrency,
            self.config.ping_packet_count,
        );

        // One-time hydration from persisted storage.
        if let Err(e) = self.hydrate().await {
            warn!("LB hydrate from persisted storage failed: {e}");
        }

        let mut shutdown = shutdown;
        let mut interval = tokio::time::interval(self.config.check_interval);
        // Fire on the next tick, not immediately, to let DB/API stabilize.
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    match self.run_check_cycle().await {
                        Ok(n) => debug!("LB cycle probed {n} records"),
                        Err(e) => error!("LB cycle error: {e}"),
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

    /// Read every persisted health row and load it into `state`. Drops any
    /// row whose underlying record no longer exists.
    async fn hydrate(&self) -> anyhow::Result<()> {
        let persisted = self.db.list_lb_health()?;
        if persisted.is_empty() {
            return Ok(());
        }

        let mut hydrated = 0usize;
        let mut orphaned: Vec<Uuid> = Vec::new();
        let mut state = self.state.lock().await;

        for row in &persisted {
            let record = match self.db.get_record(&row.record_id) {
                Ok(Some(r)) => r,
                Ok(None) => {
                    orphaned.push(row.record_id);
                    continue;
                }
                Err(e) => {
                    warn!("hydrate: get_record({}) failed: {e}", row.record_id);
                    continue;
                }
            };

            let hc = match &record.health_check {
                Some(hc) => hc,
                None => {
                    // Record no longer has health-check config — drop the row.
                    orphaned.push(row.record_id);
                    continue;
                }
            };

            let rh = RecordHealth::from_persisted(
                row,
                hc.healthy_threshold,
                hc.unhealthy_threshold,
                record.zone_id,
                record.name,
                record.data.record_type().to_string(),
            );
            state.hydrate(row.record_id, rh);
            hydrated += 1;
        }
        drop(state);

        for id in &orphaned {
            if let Err(e) = self.db.delete_lb_health(id) {
                warn!("hydrate: delete_lb_health({id}) failed: {e}");
            }
        }

        info!(
            "LB hydrated {hydrated} record(s); dropped {} orphan row(s)",
            orphaned.len()
        );
        Ok(())
    }

    /// One end-to-end probe cycle:
    ///   1. Collect every record with a HealthCheck configured.
    ///   2. Probe everything in parallel (capped concurrency).
    ///   3. Apply state transitions in one decision pass.
    ///   4. Apply last-alive failsafe.
    ///   5. Persist a snapshot in a single batched txn.
    async fn run_check_cycle(&self) -> anyhow::Result<usize> {
        let started = std::time::Instant::now();

        // ── 1. Collect targets ─────────────────────────────────────────────
        let zones = self.db.list_zones()?;
        let mut zone_names: std::collections::HashMap<Uuid, String> =
            std::collections::HashMap::new();
        let mut targets: Vec<ProbeTarget> = Vec::new();

        for zone in &zones {
            zone_names.insert(zone.id, zone.name.clone());
            for record in self.db.list_records(&zone.id)? {
                let hc = match &record.health_check {
                    Some(hc) => hc.clone(),
                    None => continue,
                };
                let target_ip = match &record.data {
                    RecordData::A(addr) => IpAddr::V4(*addr),
                    RecordData::AAAA(addr) => IpAddr::V6(*addr),
                    _ => continue,
                };

                // Register in HealthState if not already present.
                {
                    let mut state = self.state.lock().await;
                    state.register(
                        record.id,
                        hc.healthy_threshold.max(1),
                        hc.unhealthy_threshold.max(1),
                        record.zone_id,
                        record.name.clone(),
                        record.data.record_type().to_string(),
                    );
                }

                let probe_type = hc.probe_type;
                let timeout = if hc.timeout_secs > 0 {
                    Duration::from_secs(hc.timeout_secs as u64)
                } else {
                    self.config.default_timeout
                };
                let endpoint = hc.endpoint.clone();
                targets.push(ProbeTarget {
                    record_id: record.id,
                    zone_id: record.zone_id,
                    name: record.name.clone(),
                    rtype: record.data.record_type().to_string(),
                    target_ip,
                    probe_type,
                    timeout,
                    endpoint,
                    enabled_in_db: record.enabled,
                });
            }
        }

        // Drop in-memory state for records that no longer exist.
        let live: std::collections::HashSet<Uuid> =
            targets.iter().map(|t| t.record_id).collect();
        let dropped = {
            let mut state = self.state.lock().await;
            state.retain_only(&live)
        };
        for id in dropped {
            if let Err(e) = self.db.delete_lb_health(&id) {
                warn!("cleanup: delete_lb_health({id}) failed: {e}");
            }
        }

        if targets.is_empty() {
            return Ok(0);
        }

        // ── 2. Probe in parallel, capped ───────────────────────────────────
        let sem = Arc::new(Semaphore::new(self.config.probe_concurrency.max(1)));
        let ping_count = self.config.ping_packet_count;
        let mut futs = FuturesUnordered::new();
        for t in &targets {
            let sem = sem.clone();
            let target = t.clone();
            futs.push(async move {
                let _permit = sem.acquire_owned().await.expect("semaphore closed");
                let result = probe::run_probe(
                    target.probe_type,
                    target.target_ip,
                    target.timeout,
                    target.endpoint.as_deref(),
                    ping_count,
                )
                .await;
                (target, result)
            });
        }

        let mut results: Vec<(ProbeTarget, probe::ProbeResult)> = Vec::with_capacity(targets.len());
        while let Some(item) = futs.next().await {
            results.push(item);
        }

        // ── 3. Apply transitions, accumulate state-change events ───────────
        let now = Utc::now();
        let mut transitions: Vec<StateChange> = Vec::new();
        let mut to_update_in_db: Vec<(Uuid, bool)> = Vec::new();

        {
            let mut state = self.state.lock().await;
            for (target, result) in &results {
                let change = state.record_probe_result(
                    &target.record_id,
                    result.success,
                    now,
                    target.probe_type,
                    result.detail.clone(),
                );

                let should_be_enabled = state
                    .get(&target.record_id)
                    .map(|h| h.should_be_enabled())
                    .unwrap_or(true);
                if should_be_enabled != target.enabled_in_db {
                    to_update_in_db.push((target.record_id, should_be_enabled));
                }

                if let Some(new_status) = change {
                    let zone_name = zone_names
                        .get(&target.zone_id)
                        .cloned()
                        .unwrap_or_default();
                    transitions.push(StateChange {
                        record_id: target.record_id,
                        zone_id: target.zone_id,
                        zone_name,
                        name: target.name.clone(),
                        ip: target.target_ip.to_string(),
                        record_type: target.rtype.clone(),
                        status: new_status,
                        failsafe: false,
                        probe_type: target.probe_type,
                        detail: result.detail.clone(),
                        at: now,
                    });
                }
            }
        }

        // ── 4. Failsafe ────────────────────────────────────────────────────
        let failsafe_ids: Vec<Uuid> = {
            let state = self.state.lock().await;
            state.failsafe_records()
        };

        if !failsafe_ids.is_empty() {
            for id in &failsafe_ids {
                // Force this record's enabled flag to true and emit a
                // failsafe event. Don't mutate HealthStatus — it stays
                // Unhealthy; "failsafe" is layered on top in DB/UI.
                to_update_in_db.retain(|(rid, _)| rid != id);
                to_update_in_db.push((*id, true));

                let info_for_event = {
                    let state = self.state.lock().await;
                    state.get(id).map(|h| {
                        (
                            h.zone_id,
                            h.record_name.clone(),
                            h.record_type.clone(),
                            h.last_probe_type,
                            h.last_probe_detail.clone(),
                        )
                    })
                };
                if let Some((zone_id, name, rtype, ptype, detail)) = info_for_event {
                    let zone_name = zone_names.get(&zone_id).cloned().unwrap_or_default();
                    // Find the matching record IP (we don't track it on
                    // RecordHealth; resolve via DB).
                    let ip = match self.db.get_record(id) {
                        Ok(Some(r)) => match r.data {
                            RecordData::A(a) => a.to_string(),
                            RecordData::AAAA(a) => a.to_string(),
                            _ => String::new(),
                        },
                        _ => String::new(),
                    };
                    transitions.push(StateChange {
                        record_id: *id,
                        zone_id,
                        zone_name,
                        name,
                        ip,
                        record_type: rtype,
                        status: HealthStatus::Unhealthy,
                        failsafe: true,
                        probe_type: ptype,
                        detail,
                        at: now,
                    });
                    info!(
                        "failsafe: keeping record {id} enabled (group all-down, last alive)"
                    );
                }
            }
        }

        // ── 5. Apply DB updates and persist snapshot ───────────────────────
        for (id, enabled) in &to_update_in_db {
            if let Ok(Some(mut r)) = self.db.get_record(id) {
                if r.enabled != *enabled {
                    r.enabled = *enabled;
                    if let Err(e) = self.db.update_record(&r) {
                        error!("failed to update record {id} enabled state: {e}");
                    }
                }
            }
        }

        let snapshot = {
            let state = self.state.lock().await;
            state.snapshot_persisted()
        };
        if let Err(e) = self.db.upsert_lb_health_batch(&snapshot) {
            warn!("LB persist snapshot failed: {e}");
        }

        for change in &transitions {
            // record state-change in the log even if no subscribers.
            if change.failsafe {
                info!(
                    "{}.{} {} {} → {} (failsafe)",
                    change.name, change.zone_name, change.ip, change.probe_type, change.status
                );
            } else {
                info!(
                    "{}.{} {} {} → {}",
                    change.name, change.zone_name, change.ip, change.probe_type, change.status
                );
            }
            // Best-effort broadcast.
            let _ = self.events.send(change.clone());
        }

        debug!(
            "LB cycle: probed={} transitions={} failsafe={} took={:?}",
            results.len(),
            transitions.iter().filter(|c| !c.failsafe).count(),
            failsafe_ids.len(),
            started.elapsed(),
        );

        Ok(results.len())
    }
}

#[derive(Debug, Clone)]
struct ProbeTarget {
    record_id: Uuid,
    zone_id: Uuid,
    name: String,
    rtype: String,
    target_ip: IpAddr,
    probe_type: ProbeType,
    timeout: Duration,
    endpoint: Option<String>,
    enabled_in_db: bool,
}
