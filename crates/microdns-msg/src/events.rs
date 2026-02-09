use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// All events that can be published/consumed through the message bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    /// A DHCP lease was created or renewed
    LeaseCreated {
        instance_id: String,
        ip_addr: String,
        mac_addr: String,
        hostname: Option<String>,
        pool_id: String,
        timestamp: DateTime<Utc>,
    },

    /// A DHCP lease was released
    LeaseReleased {
        instance_id: String,
        ip_addr: String,
        mac_addr: String,
        timestamp: DateTime<Utc>,
    },

    /// A DNS zone was created or updated
    ZoneChanged {
        instance_id: String,
        zone_id: Uuid,
        zone_name: String,
        action: ChangeAction,
        timestamp: DateTime<Utc>,
    },

    /// A DNS record was created, updated, or deleted
    RecordChanged {
        instance_id: String,
        zone_id: Uuid,
        record_id: Uuid,
        record_name: String,
        action: ChangeAction,
        timestamp: DateTime<Utc>,
    },

    /// Health check state changed for a record
    HealthChanged {
        instance_id: String,
        record_id: Uuid,
        record_name: String,
        healthy: bool,
        timestamp: DateTime<Utc>,
    },

    /// Instance heartbeat
    Heartbeat {
        instance_id: String,
        mode: String,
        uptime_secs: u64,
        active_leases: u64,
        zones_served: u64,
        timestamp: DateTime<Utc>,
    },

    /// Configuration push from coordinator to leaves
    ConfigPush {
        source: String,
        target: Option<String>, // None = broadcast to all leaves
        payload: ConfigPayload,
        timestamp: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeAction {
    Created,
    Updated,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ConfigPayload {
    /// Push a zone definition to a leaf instance
    ZoneSync {
        zone_json: String,
        records_json: String,
    },
    /// Update a leaf's runtime configuration
    ConfigUpdate { config_toml: String },
}

impl Event {
    pub fn instance_id(&self) -> &str {
        match self {
            Event::LeaseCreated { instance_id, .. } => instance_id,
            Event::LeaseReleased { instance_id, .. } => instance_id,
            Event::ZoneChanged { instance_id, .. } => instance_id,
            Event::RecordChanged { instance_id, .. } => instance_id,
            Event::HealthChanged { instance_id, .. } => instance_id,
            Event::Heartbeat { instance_id, .. } => instance_id,
            Event::ConfigPush { source, .. } => source,
        }
    }

    pub fn topic_suffix(&self) -> &str {
        match self {
            Event::LeaseCreated { .. } | Event::LeaseReleased { .. } => "leases",
            Event::ZoneChanged { .. } | Event::RecordChanged { .. } => "dns",
            Event::HealthChanged { .. } => "health",
            Event::Heartbeat { .. } => "heartbeat",
            Event::ConfigPush { .. } => "config",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_serialization() {
        let event = Event::Heartbeat {
            instance_id: "vlan10".to_string(),
            mode: "leaf".to_string(),
            uptime_secs: 3600,
            active_leases: 42,
            zones_served: 3,
            timestamp: Utc::now(),
        };

        let json = serde_json::to_string(&event).unwrap();
        let parsed: Event = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.instance_id(), "vlan10");
        assert_eq!(parsed.topic_suffix(), "heartbeat");
    }

    #[test]
    fn test_lease_event() {
        let event = Event::LeaseCreated {
            instance_id: "vlan10".to_string(),
            ip_addr: "10.0.10.100".to_string(),
            mac_addr: "aa:bb:cc:dd:ee:ff".to_string(),
            hostname: Some("host1".to_string()),
            pool_id: "pool1".to_string(),
            timestamp: Utc::now(),
        };

        assert_eq!(event.topic_suffix(), "leases");
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("LeaseCreated"));
    }
}
