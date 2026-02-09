pub mod events;
pub mod kafka;
pub mod noop;

use async_trait::async_trait;
use events::Event;

/// Trait for the message bus abstraction.
/// Implementations can use Kafka/Redpanda, or be a no-op for standalone mode.
#[async_trait]
pub trait MessageBus: Send + Sync + 'static {
    /// Publish an event to the appropriate topic.
    async fn publish(&self, event: &Event) -> anyhow::Result<()>;

    /// Subscribe to events matching a topic pattern.
    /// Returns a receiver that yields events as they arrive.
    async fn subscribe(
        &self,
        topic_pattern: &str,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<Event>>;

    /// Gracefully shut down the message bus.
    async fn shutdown(&self) -> anyhow::Result<()>;
}

/// Create a message bus from configuration.
pub fn create_message_bus(
    backend: &str,
    instance_id: &str,
    topic_prefix: &str,
    brokers: &[String],
) -> anyhow::Result<Box<dyn MessageBus>> {
    match backend {
        "kafka" | "redpanda" => Ok(Box::new(kafka::KafkaMessageBus::new(
            instance_id,
            topic_prefix,
            brokers,
        )?)),
        _ => Ok(Box::new(noop::NoopMessageBus::new(instance_id))),
    }
}
