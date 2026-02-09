use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Tracks heartbeats from instances for health monitoring.
pub struct HeartbeatTracker {
    instances: Arc<RwLock<HashMap<String, InstanceStatus>>>,
    timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceStatus {
    pub instance_id: String,
    pub mode: String,
    pub uptime_secs: u64,
    pub active_leases: u64,
    pub zones_served: u64,
    pub last_seen: DateTime<Utc>,
    pub healthy: bool,
}

impl HeartbeatTracker {
    pub fn new(timeout_secs: u64) -> Self {
        Self {
            instances: Arc::new(RwLock::new(HashMap::new())),
            timeout_secs,
        }
    }

    /// Record a heartbeat from an instance.
    pub async fn record_heartbeat(
        &self,
        instance_id: &str,
        mode: &str,
        uptime_secs: u64,
        active_leases: u64,
        zones_served: u64,
    ) {
        let status = InstanceStatus {
            instance_id: instance_id.to_string(),
            mode: mode.to_string(),
            uptime_secs,
            active_leases,
            zones_served,
            last_seen: Utc::now(),
            healthy: true,
        };

        let mut instances = self.instances.write().await;
        instances.insert(instance_id.to_string(), status);
    }

    /// Get status of all known instances, marking stale ones as unhealthy.
    pub async fn get_all_status(&self) -> Vec<InstanceStatus> {
        let now = Utc::now();
        let mut instances = self.instances.write().await;

        for status in instances.values_mut() {
            let elapsed = (now - status.last_seen).num_seconds() as u64;
            status.healthy = elapsed < self.timeout_secs;
        }

        instances.values().cloned().collect()
    }

    /// Get status of a specific instance.
    pub async fn get_instance_status(&self, instance_id: &str) -> Option<InstanceStatus> {
        let now = Utc::now();
        let mut instances = self.instances.write().await;

        if let Some(status) = instances.get_mut(instance_id) {
            let elapsed = (now - status.last_seen).num_seconds() as u64;
            status.healthy = elapsed < self.timeout_secs;
            Some(status.clone())
        } else {
            None
        }
    }

    /// Remove stale instances that haven't been seen for 3x the timeout.
    pub async fn prune_stale(&self) {
        let now = Utc::now();
        let max_age = self.timeout_secs * 3;
        let mut instances = self.instances.write().await;
        instances.retain(|_, status| {
            let elapsed = (now - status.last_seen).num_seconds() as u64;
            elapsed < max_age
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_heartbeat_tracking() {
        let tracker = HeartbeatTracker::new(30);

        tracker.record_heartbeat("vlan10", "leaf", 100, 42, 3).await;
        tracker.record_heartbeat("vlan20", "leaf", 200, 10, 2).await;

        let all = tracker.get_all_status().await;
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|s| s.healthy));
    }

    #[tokio::test]
    async fn test_instance_lookup() {
        let tracker = HeartbeatTracker::new(30);

        tracker.record_heartbeat("vlan10", "leaf", 100, 42, 3).await;

        let status = tracker.get_instance_status("vlan10").await.unwrap();
        assert_eq!(status.active_leases, 42);

        assert!(tracker.get_instance_status("unknown").await.is_none());
    }
}
