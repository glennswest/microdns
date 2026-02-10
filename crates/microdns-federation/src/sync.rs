use microdns_core::db::Db;
use microdns_msg::events::{ConfigPayload, Event};
use microdns_msg::MessageBus;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{debug, info, warn};

/// Maximum size for sync payloads (10 MB)
const MAX_SYNC_PAYLOAD_SIZE: usize = 10 * 1024 * 1024;

/// Listens for config push events from the coordinator and applies them locally.
pub struct ConfigSyncAgent {
    instance_id: String,
    message_bus: Arc<dyn MessageBus>,
    _db: Db,
    topic_prefix: String,
}

impl ConfigSyncAgent {
    pub fn new(
        instance_id: &str,
        message_bus: Arc<dyn MessageBus>,
        db: Db,
        topic_prefix: &str,
    ) -> Self {
        Self {
            instance_id: instance_id.to_string(),
            message_bus,
            _db: db,
            topic_prefix: topic_prefix.to_string(),
        }
    }

    /// Run the sync agent: listens for config push events.
    pub async fn run(&self, shutdown: watch::Receiver<bool>) -> anyhow::Result<()> {
        info!(
            instance_id = %self.instance_id,
            "config sync agent started"
        );

        let config_pattern = format!("{}.*.config", self.topic_prefix);
        let mut config_rx = self.message_bus.subscribe(&config_pattern).await?;
        let mut shutdown = shutdown;

        loop {
            tokio::select! {
                Some(event) = config_rx.recv() => {
                    self.handle_config_event(&event).await;
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!(instance_id = %self.instance_id, "config sync agent shutting down");
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_config_event(&self, event: &Event) {
        if let Event::ConfigPush {
            target, payload, ..
        } = event
        {
            // Check if this config push is for us (or broadcast)
            if let Some(target_id) = target {
                if target_id != &self.instance_id {
                    return;
                }
            }

            match payload {
                ConfigPayload::ZoneSync {
                    zone_json,
                    records_json,
                } => {
                    if zone_json.len() + records_json.len() > MAX_SYNC_PAYLOAD_SIZE {
                        warn!(
                            instance_id = %self.instance_id,
                            zone_len = zone_json.len(),
                            records_len = records_json.len(),
                            "rejecting oversized zone sync payload"
                        );
                        return;
                    }
                    debug!(
                        instance_id = %self.instance_id,
                        zone_len = zone_json.len(),
                        records_len = records_json.len(),
                        "received zone sync from coordinator"
                    );
                    // In production: deserialize zone + records, upsert into local redb
                }
                ConfigPayload::ConfigUpdate { config_toml } => {
                    if config_toml.len() > MAX_SYNC_PAYLOAD_SIZE {
                        warn!(
                            instance_id = %self.instance_id,
                            config_len = config_toml.len(),
                            "rejecting oversized config update payload"
                        );
                        return;
                    }
                    debug!(
                        instance_id = %self.instance_id,
                        config_len = config_toml.len(),
                        "received config update from coordinator"
                    );
                    // In production: parse TOML, apply config changes, restart affected services
                    warn!("config hot-reload not yet implemented");
                }
            }
        }
    }
}
