use std::collections::HashMap;
use uuid::Uuid;

/// Tracks the health state of records that have health checks configured.
pub struct HealthState {
    records: HashMap<Uuid, RecordHealth>,
}

#[derive(Debug, Clone)]
pub struct RecordHealth {
    /// Current health status
    pub healthy: bool,
    /// Consecutive successful probes
    pub success_count: u32,
    /// Consecutive failed probes
    pub failure_count: u32,
    /// Threshold: how many consecutive successes to mark healthy
    pub healthy_threshold: u32,
    /// Threshold: how many consecutive failures to mark unhealthy
    pub unhealthy_threshold: u32,
    /// Zone ID this record belongs to (for failsafe grouping)
    pub zone_id: Uuid,
    /// Record name (for failsafe grouping)
    pub record_name: String,
    /// Record type string (for failsafe grouping)
    pub record_type: String,
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
            healthy: true, // Start healthy (optimistic)
            success_count: 0,
            failure_count: 0,
            healthy_threshold,
            unhealthy_threshold,
            zone_id,
            record_name,
            record_type,
        }
    }

    /// Record a probe result. Returns true if the health state changed.
    pub fn record_result(&mut self, success: bool) -> bool {
        let was_healthy = self.healthy;

        if success {
            self.success_count += 1;
            self.failure_count = 0;

            if !self.healthy && self.success_count >= self.healthy_threshold {
                self.healthy = true;
            }
        } else {
            self.failure_count += 1;
            self.success_count = 0;

            if self.healthy && self.failure_count >= self.unhealthy_threshold {
                self.healthy = false;
            }
        }

        was_healthy != self.healthy
    }
}

impl HealthState {
    pub fn new() -> Self {
        Self {
            records: HashMap::new(),
        }
    }

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

    pub fn unregister(&mut self, record_id: &Uuid) {
        self.records.remove(record_id);
    }

    /// Record a probe result. Returns Some(new_healthy_state) if state changed.
    pub fn record_probe_result(&mut self, record_id: &Uuid, success: bool) -> Option<bool> {
        let health = self.records.get_mut(record_id)?;
        if health.record_result(success) {
            Some(health.healthy)
        } else {
            None
        }
    }

    pub fn get(&self, record_id: &Uuid) -> Option<&RecordHealth> {
        self.records.get(record_id)
    }

    /// Failsafe check: if ALL records for a given (zone_id, name, type) are unhealthy,
    /// return the record IDs that should be force-enabled to maintain availability.
    /// We pick the first one as the failsafe.
    pub fn failsafe_records(&self) -> Vec<Uuid> {
        // Group records by (zone_id, name, type)
        type GroupKey<'a> = (Uuid, &'a str, &'a str);
        let mut groups: HashMap<GroupKey<'_>, Vec<(Uuid, bool)>> = HashMap::new();

        for (id, health) in &self.records {
            let key: GroupKey<'_> = (health.zone_id, health.record_name.as_str(), health.record_type.as_str());
            groups.entry(key).or_default().push((*id, health.healthy));
        }

        let mut failsafe = Vec::new();
        for members in groups.values() {
            // If all members are unhealthy, failsafe the first one
            if members.len() > 1 && members.iter().all(|(_, healthy)| !healthy) {
                if let Some((id, _)) = members.first() {
                    failsafe.push(*id);
                }
            }
        }

        failsafe
    }
}

impl Default for HealthState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_health_transitions() {
        let mut health = RecordHealth::new(2, 3, Uuid::new_v4(), "www".into(), "A".into());

        // Starts healthy
        assert!(health.healthy);

        // 2 failures - not enough yet
        assert!(!health.record_result(false));
        assert!(!health.record_result(false));
        assert!(health.healthy);

        // 3rd failure - transitions to unhealthy
        assert!(health.record_result(false));
        assert!(!health.healthy);

        // 1 success - not enough
        assert!(!health.record_result(true));
        assert!(!health.healthy);

        // 2nd success - transitions back to healthy
        assert!(health.record_result(true));
        assert!(health.healthy);
    }

    #[test]
    fn test_failsafe() {
        let mut state = HealthState::new();
        let zone_id = Uuid::new_v4();
        let r1 = Uuid::new_v4();
        let r2 = Uuid::new_v4();

        state.register(r1, 1, 1, zone_id, "www".into(), "A".into());
        state.register(r2, 1, 1, zone_id, "www".into(), "A".into());

        // Both healthy - no failsafe
        assert!(state.failsafe_records().is_empty());

        // Make both unhealthy
        state.record_probe_result(&r1, false);
        state.record_probe_result(&r2, false);

        // Should trigger failsafe
        let failsafe = state.failsafe_records();
        assert_eq!(failsafe.len(), 1);
    }

    #[test]
    fn test_no_failsafe_single_record() {
        let mut state = HealthState::new();
        let zone_id = Uuid::new_v4();
        let r1 = Uuid::new_v4();

        state.register(r1, 1, 1, zone_id, "www".into(), "A".into());
        state.record_probe_result(&r1, false);

        // Single record groups don't trigger failsafe
        assert!(state.failsafe_records().is_empty());
    }
}
