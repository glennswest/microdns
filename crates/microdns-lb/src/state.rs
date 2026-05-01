use chrono::{DateTime, Utc};
use microdns_core::types::{HealthStatus, PersistedHealth, ProbeType};
use std::collections::HashMap;
use uuid::Uuid;

/// In-memory health state for one record. Hydrated from `PersistedHealth`
/// at startup and serialized back at the end of each probe cycle.
#[derive(Debug, Clone)]
pub struct RecordHealth {
    /// Tri-state status. New records register as `Unknown` and stay `enabled`
    /// until the first probe completes.
    pub status: HealthStatus,
    /// Consecutive successful probes (resets on failure).
    pub success_count: u32,
    /// Consecutive failed probes (resets on success).
    pub failure_count: u32,
    /// Threshold: how many consecutive successes to mark healthy.
    pub healthy_threshold: u32,
    /// Threshold: how many consecutive failures to mark unhealthy.
    pub unhealthy_threshold: u32,
    /// Zone ID (for failsafe grouping).
    pub zone_id: Uuid,
    /// Record name (for failsafe grouping).
    pub record_name: String,
    /// Record type as string (for failsafe grouping).
    pub record_type: String,
    /// When the last probe completed.
    pub last_checked_at: Option<DateTime<Utc>>,
    /// When the status last flipped.
    pub last_state_change_at: Option<DateTime<Utc>>,
    /// Last time the record was Healthy (drives "last alive" failsafe).
    pub last_healthy_at: Option<DateTime<Utc>>,
    /// Human-readable detail from the most recent probe.
    pub last_probe_detail: String,
    /// Probe type used on the most recent probe.
    pub last_probe_type: ProbeType,
}

impl RecordHealth {
    pub fn new(
        healthy_threshold: u32,
        unhealthy_threshold: u32,
        zone_id: Uuid,
        record_name: String,
        record_type: String,
    ) -> Self {
        Self {
            status: HealthStatus::Unknown,
            success_count: 0,
            failure_count: 0,
            healthy_threshold,
            unhealthy_threshold,
            zone_id,
            record_name,
            record_type,
            last_checked_at: None,
            last_state_change_at: None,
            last_healthy_at: None,
            last_probe_detail: String::new(),
            last_probe_type: ProbeType::Ping,
        }
    }

    /// Build from a previously persisted row. Group identifiers come from the
    /// live record because they may have changed since the row was written.
    pub fn from_persisted(
        persisted: &PersistedHealth,
        healthy_threshold: u32,
        unhealthy_threshold: u32,
        zone_id: Uuid,
        record_name: String,
        record_type: String,
    ) -> Self {
        Self {
            status: persisted.status,
            success_count: persisted.consecutive_successes,
            failure_count: persisted.consecutive_failures,
            healthy_threshold,
            unhealthy_threshold,
            zone_id,
            record_name,
            record_type,
            last_checked_at: Some(persisted.last_checked_at),
            last_state_change_at: Some(persisted.last_state_change_at),
            last_healthy_at: persisted.last_healthy_at,
            last_probe_detail: persisted.last_probe_detail.clone(),
            last_probe_type: persisted.last_probe_type,
        }
    }

    /// Record a probe result. Returns `Some(new_status)` if the status
    /// transitioned (Unknown→Healthy/Unhealthy is a transition; same-status
    /// is not).
    pub fn record_result(
        &mut self,
        success: bool,
        now: DateTime<Utc>,
        probe_type: ProbeType,
        detail: String,
    ) -> Option<HealthStatus> {
        let prev = self.status;

        if success {
            self.success_count = self.success_count.saturating_add(1);
            self.failure_count = 0;
            self.last_healthy_at = Some(now);
            // Unknown → Healthy after first success regardless of threshold
            // (we're optimistic on first hit). Unhealthy → Healthy needs
            // healthy_threshold consecutive successes.
            match self.status {
                HealthStatus::Unknown => self.status = HealthStatus::Healthy,
                HealthStatus::Unhealthy if self.success_count >= self.healthy_threshold => {
                    self.status = HealthStatus::Healthy
                }
                _ => {}
            }
        } else {
            self.failure_count = self.failure_count.saturating_add(1);
            self.success_count = 0;
            // Unknown → Unhealthy needs unhealthy_threshold consecutive failures.
            // Healthy → Unhealthy also needs unhealthy_threshold.
            if self.failure_count >= self.unhealthy_threshold
                && self.status != HealthStatus::Unhealthy
            {
                self.status = HealthStatus::Unhealthy;
            }
        }

        self.last_checked_at = Some(now);
        self.last_probe_detail = detail;
        self.last_probe_type = probe_type;

        if prev != self.status {
            self.last_state_change_at = Some(now);
            Some(self.status)
        } else {
            None
        }
    }

    /// Whether this record should currently be enabled in DNS responses.
    /// Unknown records are kept enabled (optimistic) so a fresh restart
    /// doesn't black-hole traffic before the first probe completes.
    pub fn should_be_enabled(&self) -> bool {
        match self.status {
            HealthStatus::Healthy | HealthStatus::Unknown => true,
            HealthStatus::Unhealthy => false,
        }
    }

    /// Serialize current state for persistence.
    pub fn to_persisted(&self, record_id: Uuid) -> Option<PersistedHealth> {
        // Don't persist a row that has never been probed — there's nothing
        // useful to write. Hydration code already treats "no row" as Unknown.
        let last_checked_at = self.last_checked_at?;
        Some(PersistedHealth {
            record_id,
            status: self.status,
            last_checked_at,
            last_state_change_at: self.last_state_change_at.unwrap_or(last_checked_at),
            last_healthy_at: self.last_healthy_at,
            last_probe_detail: self.last_probe_detail.clone(),
            last_probe_type: self.last_probe_type,
            consecutive_successes: self.success_count,
            consecutive_failures: self.failure_count,
        })
    }
}

/// In-memory map of record_id → RecordHealth.
pub struct HealthState {
    records: HashMap<Uuid, RecordHealth>,
}

impl HealthState {
    pub fn new() -> Self {
        Self {
            records: HashMap::new(),
        }
    }

    /// Register a record under monitoring if it isn't already.
    pub fn register(
        &mut self,
        record_id: Uuid,
        healthy_threshold: u32,
        unhealthy_threshold: u32,
        zone_id: Uuid,
        record_name: String,
        record_type: String,
    ) {
        self.records.entry(record_id).or_insert_with(|| {
            RecordHealth::new(
                healthy_threshold,
                unhealthy_threshold,
                zone_id,
                record_name,
                record_type,
            )
        });
    }

    /// Insert (or overwrite) a record's health using a persisted row.
    pub fn hydrate(&mut self, record_id: Uuid, health: RecordHealth) {
        self.records.insert(record_id, health);
    }

    pub fn unregister(&mut self, record_id: &Uuid) {
        self.records.remove(record_id);
    }

    /// Drop any record IDs that aren't in `live`. Returns the dropped IDs so
    /// the caller can clean up persisted rows too.
    pub fn retain_only(&mut self, live: &std::collections::HashSet<Uuid>) -> Vec<Uuid> {
        let dropped: Vec<Uuid> = self
            .records
            .keys()
            .filter(|k| !live.contains(*k))
            .copied()
            .collect();
        for id in &dropped {
            self.records.remove(id);
        }
        dropped
    }

    pub fn get(&self, record_id: &Uuid) -> Option<&RecordHealth> {
        self.records.get(record_id)
    }

    pub fn get_mut(&mut self, record_id: &Uuid) -> Option<&mut RecordHealth> {
        self.records.get_mut(record_id)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Iterate over (record_id, health) for all monitored records.
    pub fn iter(&self) -> impl Iterator<Item = (&Uuid, &RecordHealth)> {
        self.records.iter()
    }

    /// Apply a probe result and return the new status if the record
    /// transitioned.
    pub fn record_probe_result(
        &mut self,
        record_id: &Uuid,
        success: bool,
        now: DateTime<Utc>,
        probe_type: ProbeType,
        detail: String,
    ) -> Option<HealthStatus> {
        self.records
            .get_mut(record_id)?
            .record_result(success, now, probe_type, detail)
    }

    /// Failsafe grouping: for every `(zone_id, name, type)` group with 2+
    /// members where every member is Unhealthy, return the record_id with
    /// the most recent `last_healthy_at` (deterministic "last alive"). If
    /// none of the members has ever been seen healthy, the group does not
    /// trigger a failsafe.
    pub fn failsafe_records(&self) -> Vec<Uuid> {
        type Key<'a> = (Uuid, &'a str, &'a str);
        let mut groups: HashMap<Key<'_>, Vec<(Uuid, &RecordHealth)>> = HashMap::new();
        for (id, h) in &self.records {
            let k: Key<'_> = (h.zone_id, h.record_name.as_str(), h.record_type.as_str());
            groups.entry(k).or_default().push((*id, h));
        }

        let mut out = Vec::new();
        for members in groups.values() {
            if members.len() < 2 {
                continue;
            }
            if !members.iter().all(|(_, h)| h.status == HealthStatus::Unhealthy) {
                continue;
            }
            // Pick the member with the most recent last_healthy_at.
            let pick = members
                .iter()
                .max_by_key(|(_, h)| h.last_healthy_at)
                .and_then(|(id, h)| h.last_healthy_at.map(|_| *id));
            if let Some(id) = pick {
                out.push(id);
            }
        }
        out
    }

    /// Snapshot every record's persistable state. Returns rows for records
    /// that have been probed at least once.
    pub fn snapshot_persisted(&self) -> Vec<PersistedHealth> {
        self.records
            .iter()
            .filter_map(|(id, h)| h.to_persisted(*id))
            .collect()
    }

    /// Aggregate counts for the dashboard "stat" cards.
    pub fn aggregate(&self) -> HealthAggregate {
        let mut total = 0;
        let mut healthy = 0;
        let mut unhealthy = 0;
        let mut unknown = 0;
        let mut groups: std::collections::HashSet<(Uuid, String, String)> =
            std::collections::HashSet::new();
        for h in self.records.values() {
            total += 1;
            match h.status {
                HealthStatus::Healthy => healthy += 1,
                HealthStatus::Unhealthy => unhealthy += 1,
                HealthStatus::Unknown => unknown += 1,
            }
            groups.insert((h.zone_id, h.record_name.clone(), h.record_type.clone()));
        }
        HealthAggregate {
            total,
            healthy,
            unhealthy,
            unknown,
            groups: groups.len(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HealthAggregate {
    pub total: usize,
    pub healthy: usize,
    pub unhealthy: usize,
    pub unknown: usize,
    pub groups: usize,
}

impl Default for HealthState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn unknown_to_healthy_on_first_success() {
        let mut h = RecordHealth::new(2, 3, Uuid::new_v4(), "www".into(), "A".into());
        assert_eq!(h.status, HealthStatus::Unknown);
        let t = h.record_result(true, now(), ProbeType::Tcp, "ok".into());
        assert_eq!(t, Some(HealthStatus::Healthy));
        assert_eq!(h.status, HealthStatus::Healthy);
    }

    #[test]
    fn healthy_to_unhealthy_after_threshold() {
        let mut h = RecordHealth::new(2, 3, Uuid::new_v4(), "www".into(), "A".into());
        h.record_result(true, now(), ProbeType::Tcp, "ok".into()); // Unknown → Healthy
        assert_eq!(h.record_result(false, now(), ProbeType::Tcp, "x".into()), None);
        assert_eq!(h.record_result(false, now(), ProbeType::Tcp, "x".into()), None);
        assert_eq!(
            h.record_result(false, now(), ProbeType::Tcp, "x".into()),
            Some(HealthStatus::Unhealthy)
        );
    }

    #[test]
    fn last_alive_failsafe() {
        let mut state = HealthState::new();
        let zone = Uuid::new_v4();
        let r1 = Uuid::new_v4();
        let r2 = Uuid::new_v4();
        state.register(r1, 1, 1, zone, "api".into(), "A".into());
        state.register(r2, 1, 1, zone, "api".into(), "A".into());

        // Both healthy at different times.
        let early = "2026-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let late = "2026-01-02T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        state.record_probe_result(&r1, true, early, ProbeType::Tcp, "ok".into());
        state.record_probe_result(&r2, true, late, ProbeType::Tcp, "ok".into());

        // Both go down.
        state.record_probe_result(&r1, false, late, ProbeType::Tcp, "x".into());
        state.record_probe_result(&r2, false, late, ProbeType::Tcp, "x".into());

        let pick = state.failsafe_records();
        assert_eq!(pick, vec![r2], "must pick the most recently alive member");
    }

    #[test]
    fn no_failsafe_for_single_member() {
        let mut state = HealthState::new();
        let zone = Uuid::new_v4();
        let r1 = Uuid::new_v4();
        state.register(r1, 1, 1, zone, "api".into(), "A".into());
        state.record_probe_result(&r1, true, now(), ProbeType::Tcp, "ok".into());
        state.record_probe_result(&r1, false, now(), ProbeType::Tcp, "x".into());
        assert!(state.failsafe_records().is_empty());
    }

    #[test]
    fn snapshot_skips_never_probed() {
        let mut state = HealthState::new();
        let zone = Uuid::new_v4();
        let r1 = Uuid::new_v4();
        let r2 = Uuid::new_v4();
        state.register(r1, 1, 1, zone, "api".into(), "A".into());
        state.register(r2, 1, 1, zone, "api".into(), "A".into());
        state.record_probe_result(&r1, true, now(), ProbeType::Tcp, "ok".into());

        let snap = state.snapshot_persisted();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].record_id, r1);
    }
}
