use crate::heartbeat::HeartbeatTracker;
use microdns_msg::events::Event;
use microdns_msg::MessageBus;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{debug, info, warn};

/// Coordinator agent: subscribes to all leaf events, tracks health, aggregates status.
pub struct CoordinatorAgent {
    instance_id: String,
    message_bus: Arc<dyn MessageBus>,
    heartbeat_tracker: Arc<HeartbeatTracker>,
    topic_prefix: String,
}

impl CoordinatorAgent {
    pub fn new(
        instance_id: &str,
        message_bus: Arc<dyn MessageBus>,
        heartbeat_tracker: Arc<HeartbeatTracker>,
        topic_prefix: &str,
    ) -> Self {
        Self {
            instance_id: instance_id.to_string(),
            message_bus,
            heartbeat_tracker,
            topic_prefix: topic_prefix.to_string(),
        }
    }

    pub fn heartbeat_tracker(&self) -> &HeartbeatTracker {
        &self.heartbeat_tracker
    }

    /// Run the coordinator: subscribes to all leaf events and processes them.
    pub async fn run(&self, shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
        info!(
            instance_id = %self.instance_id,
            "coordinator agent started"
        );

        // Subscribe to all heartbeats
        let heartbeat_pattern = format!("{}.*.heartbeat", self.topic_prefix);
        let mut heartbeat_rx = self.message_bus.subscribe(&heartbeat_pattern).await?;

        // Subscribe to all lease events
        let lease_pattern = format!("{}.*.leases", self.topic_prefix);
        let mut lease_rx = self.message_bus.subscribe(&lease_pattern).await?;

        // Subscribe to all health events
        let health_pattern = format!("{}.*.health", self.topic_prefix);
        let mut health_rx = self.message_bus.subscribe(&health_pattern).await?;

        let mut shutdown = shutdown;

        // Periodic prune of stale instances
        let tracker = self.heartbeat_tracker.clone();
        let prune_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                tracker.prune_stale().await;
            }
        });

        loop {
            tokio::select! {
                Some(event) = heartbeat_rx.recv() => {
                    self.handle_heartbeat(&event).await;
                }
                Some(event) = lease_rx.recv() => {
                    self.handle_lease_event(&event).await;
                }
                Some(event) = health_rx.recv() => {
                    self.handle_health_event(&event).await;
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!(instance_id = %self.instance_id, "coordinator agent shutting down");
                        prune_handle.abort();
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_heartbeat(&self, event: &Event) {
        if let Event::Heartbeat {
            instance_id,
            mode,
            uptime_secs,
            active_leases,
            zones_served,
            ..
        } = event
        {
            debug!(
                from = %instance_id,
                uptime = uptime_secs,
                leases = active_leases,
                zones = zones_served,
                "received heartbeat"
            );

            self.heartbeat_tracker
                .record_heartbeat(
                    instance_id,
                    mode,
                    *uptime_secs,
                    *active_leases,
                    *zones_served,
                )
                .await;
        }
    }

    async fn handle_lease_event(&self, event: &Event) {
        match event {
            Event::LeaseCreated {
                instance_id,
                ip_addr,
                mac_addr,
                hostname,
                ..
            } => {
                debug!(
                    from = %instance_id,
                    ip = %ip_addr,
                    mac = %mac_addr,
                    hostname = ?hostname,
                    "lease created on remote instance"
                );
            }
            Event::LeaseReleased {
                instance_id,
                ip_addr,
                mac_addr,
                ..
            } => {
                debug!(
                    from = %instance_id,
                    ip = %ip_addr,
                    mac = %mac_addr,
                    "lease released on remote instance"
                );
            }
            _ => {
                warn!("unexpected event type in lease handler");
            }
        }
    }

    async fn handle_health_event(&self, event: &Event) {
        if let Event::HealthChanged {
            instance_id,
            record_name,
            healthy,
            ..
        } = event
        {
            debug!(
                from = %instance_id,
                record = %record_name,
                healthy = healthy,
                "health state changed on remote instance"
            );
        }
    }

    /// Push a configuration update to a specific leaf or broadcast to all.
    pub async fn push_config(
        &self,
        target: Option<&str>,
        config_toml: &str,
    ) -> anyhow::Result<()> {
        let event = Event::ConfigPush {
            source: self.instance_id.clone(),
            target: target.map(String::from),
            payload: microdns_msg::events::ConfigPayload::ConfigUpdate {
                config_toml: config_toml.to_string(),
            },
            timestamp: chrono::Utc::now(),
        };

        self.message_bus.publish(&event).await?;

        if let Some(target) = target {
            info!(target = target, "pushed config update to leaf");
        } else {
            info!("broadcast config update to all leaves");
        }

        Ok(())
    }
}
