use chrono::Utc;
use microdns_msg::events::Event;
use microdns_msg::MessageBus;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;
use tracing::{debug, error, info};

/// Leaf instance agent: publishes heartbeats and events to the coordinator.
pub struct LeafAgent {
    instance_id: String,
    message_bus: Arc<dyn MessageBus>,
    heartbeat_interval_secs: u64,
    start_time: Instant,
}

impl LeafAgent {
    pub fn new(
        instance_id: &str,
        message_bus: Arc<dyn MessageBus>,
        heartbeat_interval_secs: u64,
    ) -> Self {
        Self {
            instance_id: instance_id.to_string(),
            message_bus,
            heartbeat_interval_secs,
            start_time: Instant::now(),
        }
    }

    /// Run the leaf agent: periodically sends heartbeats.
    pub async fn run(
        &self,
        active_leases_fn: Arc<dyn Fn() -> u64 + Send + Sync>,
        zones_served_fn: Arc<dyn Fn() -> u64 + Send + Sync>,
        shutdown: watch::Receiver<bool>,
    ) -> anyhow::Result<()> {
        info!(
            instance_id = %self.instance_id,
            interval_secs = self.heartbeat_interval_secs,
            "leaf agent started"
        );

        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(self.heartbeat_interval_secs));
        let mut shutdown = shutdown;

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let event = Event::Heartbeat {
                        instance_id: self.instance_id.clone(),
                        mode: "leaf".to_string(),
                        uptime_secs: self.start_time.elapsed().as_secs(),
                        active_leases: active_leases_fn(),
                        zones_served: zones_served_fn(),
                        timestamp: Utc::now(),
                    };

                    if let Err(e) = self.message_bus.publish(&event).await {
                        error!("failed to publish heartbeat: {e}");
                    } else {
                        debug!(instance_id = %self.instance_id, "heartbeat sent");
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!(instance_id = %self.instance_id, "leaf agent shutting down");
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    /// Publish a lease event.
    pub async fn publish_lease_created(
        &self,
        ip_addr: &str,
        mac_addr: &str,
        hostname: Option<&str>,
        pool_id: &str,
    ) -> anyhow::Result<()> {
        let event = Event::LeaseCreated {
            instance_id: self.instance_id.clone(),
            ip_addr: ip_addr.to_string(),
            mac_addr: mac_addr.to_string(),
            hostname: hostname.map(String::from),
            pool_id: pool_id.to_string(),
            timestamp: Utc::now(),
        };
        self.message_bus.publish(&event).await
    }

    /// Publish a lease release event.
    pub async fn publish_lease_released(
        &self,
        ip_addr: &str,
        mac_addr: &str,
    ) -> anyhow::Result<()> {
        let event = Event::LeaseReleased {
            instance_id: self.instance_id.clone(),
            ip_addr: ip_addr.to_string(),
            mac_addr: mac_addr.to_string(),
            timestamp: Utc::now(),
        };
        self.message_bus.publish(&event).await
    }

    /// Publish a health change event.
    pub async fn publish_health_changed(
        &self,
        record_id: uuid::Uuid,
        record_name: &str,
        healthy: bool,
    ) -> anyhow::Result<()> {
        let event = Event::HealthChanged {
            instance_id: self.instance_id.clone(),
            record_id,
            record_name: record_name.to_string(),
            healthy,
            timestamp: Utc::now(),
        };
        self.message_bus.publish(&event).await
    }
}
