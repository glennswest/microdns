use crate::events::Event;
use crate::MessageBus;
use async_nats::Client;
use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// NATS-backed message bus using async-nats.
pub struct NatsMessageBus {
    client: Client,
    instance_id: String,
    topic_prefix: String,
}

impl NatsMessageBus {
    pub async fn new(
        instance_id: &str,
        topic_prefix: &str,
        url: &str,
    ) -> anyhow::Result<Self> {
        info!(
            instance_id = instance_id,
            topic_prefix = topic_prefix,
            url = url,
            "connecting to NATS"
        );

        let client = async_nats::connect(url).await.map_err(|e| {
            anyhow::anyhow!("failed to connect to NATS at {url}: {e}")
        })?;

        info!(
            instance_id = instance_id,
            url = url,
            "NATS connection established"
        );

        Ok(Self {
            client,
            instance_id: instance_id.to_string(),
            topic_prefix: topic_prefix.to_string(),
        })
    }

    fn subject_for_event(&self, event: &Event) -> String {
        format!(
            "{}.{}.{}",
            self.topic_prefix,
            event.instance_id(),
            event.topic_suffix()
        )
    }
}

#[async_trait]
impl MessageBus for NatsMessageBus {
    async fn publish(&self, event: &Event) -> anyhow::Result<()> {
        let subject = self.subject_for_event(event);
        let payload = serde_json::to_vec(event)?;

        debug!(
            subject = %subject,
            payload_len = payload.len(),
            "nats: publishing event"
        );

        self.client
            .publish(subject.clone(), payload.into())
            .await
            .map_err(|e| anyhow::anyhow!("nats publish to {subject}: {e}"))?;

        Ok(())
    }

    async fn subscribe(
        &self,
        topic_pattern: &str,
    ) -> anyhow::Result<mpsc::Receiver<Event>> {
        let (tx, rx) = mpsc::channel(256);

        info!(
            instance_id = %self.instance_id,
            pattern = topic_pattern,
            "nats: subscribing to subject pattern"
        );

        let mut subscriber = self
            .client
            .subscribe(topic_pattern.to_string())
            .await
            .map_err(|e| anyhow::anyhow!("nats subscribe to {topic_pattern}: {e}"))?;

        tokio::spawn(async move {
            while let Some(msg) = subscriber.next().await {
                match serde_json::from_slice::<Event>(&msg.payload) {
                    Ok(event) => {
                        if tx.send(event).await.is_err() {
                            debug!("nats: subscriber receiver dropped, stopping");
                            break;
                        }
                    }
                    Err(e) => {
                        warn!(
                            subject = %msg.subject,
                            "nats: failed to deserialize event: {e}"
                        );
                    }
                }
            }
            debug!("nats: subscription loop ended");
        });

        Ok(rx)
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        info!(instance_id = %self.instance_id, "nats: draining connection");
        self.client.drain().await.map_err(|e| {
            anyhow::anyhow!("nats drain: {e}")
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    // Integration test — requires a running NATS server.
    // Run with: cargo test -p microdns-msg nats_roundtrip -- --ignored
    #[tokio::test]
    #[ignore]
    async fn test_nats_roundtrip() {
        let bus = NatsMessageBus::new("test-01", "microdns", "nats://127.0.0.1:4222")
            .await
            .expect("failed to connect to NATS");

        let mut rx = bus
            .subscribe("microdns.test-01.heartbeat")
            .await
            .expect("failed to subscribe");

        // Give subscription time to establish
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let event = Event::Heartbeat {
            instance_id: "test-01".to_string(),
            mode: "standalone".to_string(),
            uptime_secs: 42,
            active_leases: 0,
            zones_served: 1,
            timestamp: Utc::now(),
        };

        bus.publish(&event).await.expect("failed to publish");

        let received = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            rx.recv(),
        )
        .await
        .expect("timeout waiting for event")
        .expect("channel closed");

        assert_eq!(received.instance_id(), "test-01");
        assert_eq!(received.topic_suffix(), "heartbeat");
    }
}
