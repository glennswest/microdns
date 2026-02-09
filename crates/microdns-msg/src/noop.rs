use crate::events::Event;
use crate::MessageBus;
use async_trait::async_trait;
use tracing::debug;

/// No-op message bus for standalone mode. Events are logged but not transmitted.
pub struct NoopMessageBus {
    instance_id: String,
}

impl NoopMessageBus {
    pub fn new(instance_id: &str) -> Self {
        Self {
            instance_id: instance_id.to_string(),
        }
    }
}

#[async_trait]
impl MessageBus for NoopMessageBus {
    async fn publish(&self, event: &Event) -> anyhow::Result<()> {
        debug!(
            instance_id = %self.instance_id,
            event_type = event.topic_suffix(),
            "noop: event published (discarded)"
        );
        Ok(())
    }

    async fn subscribe(
        &self,
        topic_pattern: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<Event>> {
        debug!(
            instance_id = %self.instance_id,
            topic = topic_pattern,
            "noop: subscribe (no events will be received)"
        );
        // Return a receiver that never yields any events
        let (_tx, rx) = tokio::sync::mpsc::channel(1);
        Ok(rx)
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        debug!(instance_id = %self.instance_id, "noop: message bus shutdown");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[tokio::test]
    async fn test_noop_publish() {
        let bus = NoopMessageBus::new("test-01");
        let event = Event::Heartbeat {
            instance_id: "test-01".to_string(),
            mode: "standalone".to_string(),
            uptime_secs: 60,
            active_leases: 0,
            zones_served: 1,
            timestamp: Utc::now(),
        };
        assert!(bus.publish(&event).await.is_ok());
    }

    #[tokio::test]
    async fn test_noop_subscribe() {
        let bus = NoopMessageBus::new("test-01");
        let rx = bus.subscribe("microdns.*").await.unwrap();
        // Receiver should be open but no messages
        assert!(rx.is_empty());
    }
}
