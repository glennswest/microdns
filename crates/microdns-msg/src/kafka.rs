use crate::events::Event;
use crate::MessageBus;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, info, warn};

type Subscribers = Vec<(String, mpsc::Sender<Event>)>;

/// Kafka/Redpanda-backed message bus.
///
/// Note: The actual rdkafka integration requires the C library librdkafka.
/// This implementation uses an in-memory channel for development/testing
/// and logs what would be sent to Kafka. For production use with a real
/// Kafka cluster, this would be backed by rdkafka producer/consumer.
pub struct KafkaMessageBus {
    instance_id: String,
    topic_prefix: String,
    brokers: Vec<String>,
    subscribers: Arc<Mutex<Subscribers>>,
}

impl KafkaMessageBus {
    pub fn new(
        instance_id: &str,
        topic_prefix: &str,
        brokers: &[String],
    ) -> anyhow::Result<Self> {
        info!(
            instance_id = instance_id,
            topic_prefix = topic_prefix,
            brokers = ?brokers,
            "initializing Kafka message bus"
        );

        if brokers.is_empty() {
            warn!("no Kafka brokers configured, messages will be queued locally");
        }

        Ok(Self {
            instance_id: instance_id.to_string(),
            topic_prefix: topic_prefix.to_string(),
            brokers: brokers.to_vec(),
            subscribers: Arc::new(Mutex::new(Vec::new())),
        })
    }

    fn topic_for_event(&self, event: &Event) -> String {
        format!(
            "{}.{}.{}",
            self.topic_prefix,
            event.instance_id(),
            event.topic_suffix()
        )
    }
}

#[async_trait]
impl MessageBus for KafkaMessageBus {
    async fn publish(&self, event: &Event) -> anyhow::Result<()> {
        let topic = self.topic_for_event(event);
        let payload = serde_json::to_string(event)?;

        debug!(
            topic = %topic,
            payload_len = payload.len(),
            "kafka: publishing event"
        );

        // In production, this would use rdkafka::producer::FutureProducer
        // to send the message to the Kafka cluster.
        //
        // For now, fan out to local subscribers that match the topic.
        let subscribers = self.subscribers.lock().await;
        for (pattern, tx) in subscribers.iter() {
            if topic_matches(pattern, &topic) {
                if let Err(e) = tx.try_send(event.clone()) {
                    error!(topic = %topic, pattern = %pattern, "failed to deliver to subscriber: {e}");
                }
            }
        }

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
            brokers = ?self.brokers,
            "kafka: subscribing to topic pattern"
        );

        // In production, this would use rdkafka::consumer::StreamConsumer
        // with a regex-based topic subscription.
        let mut subscribers = self.subscribers.lock().await;
        subscribers.push((topic_pattern.to_string(), tx));

        Ok(rx)
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        info!(instance_id = %self.instance_id, "kafka: shutting down message bus");
        let mut subscribers = self.subscribers.lock().await;
        subscribers.clear();
        Ok(())
    }
}

/// Simple topic pattern matching. Supports `*` as a wildcard for a single segment.
fn topic_matches(pattern: &str, topic: &str) -> bool {
    let pattern_parts: Vec<&str> = pattern.split('.').collect();
    let topic_parts: Vec<&str> = topic.split('.').collect();

    if pattern_parts.len() != topic_parts.len() {
        return false;
    }

    pattern_parts
        .iter()
        .zip(topic_parts.iter())
        .all(|(p, t)| *p == "*" || p == t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Event;
    use chrono::Utc;

    #[test]
    fn test_topic_matching() {
        assert!(topic_matches("microdns.*.heartbeat", "microdns.vlan10.heartbeat"));
        assert!(topic_matches("microdns.vlan10.heartbeat", "microdns.vlan10.heartbeat"));
        assert!(!topic_matches("microdns.*.heartbeat", "microdns.vlan10.leases"));
        assert!(!topic_matches("microdns.*.heartbeat", "microdns.vlan10.heartbeat.extra"));
    }

    #[tokio::test]
    async fn test_kafka_local_pub_sub() {
        let bus = KafkaMessageBus::new("test-01", "microdns", &[]).unwrap();
        let mut rx = bus.subscribe("microdns.*.heartbeat").await.unwrap();

        let event = Event::Heartbeat {
            instance_id: "test-01".to_string(),
            mode: "leaf".to_string(),
            uptime_secs: 100,
            active_leases: 5,
            zones_served: 2,
            timestamp: Utc::now(),
        };

        bus.publish(&event).await.unwrap();

        let received = rx.try_recv().unwrap();
        assert_eq!(received.instance_id(), "test-01");
        assert_eq!(received.topic_suffix(), "heartbeat");
    }

    #[tokio::test]
    async fn test_kafka_no_match() {
        let bus = KafkaMessageBus::new("test-01", "microdns", &[]).unwrap();
        let mut rx = bus.subscribe("microdns.*.leases").await.unwrap();

        let event = Event::Heartbeat {
            instance_id: "test-01".to_string(),
            mode: "leaf".to_string(),
            uptime_secs: 100,
            active_leases: 5,
            zones_served: 2,
            timestamp: Utc::now(),
        };

        bus.publish(&event).await.unwrap();

        // Heartbeat should not match leases pattern
        assert!(rx.try_recv().is_err());
    }
}
