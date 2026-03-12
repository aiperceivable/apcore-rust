// APCore Protocol — Event subscribers
// Spec reference: Event subscription and webhook delivery

use async_trait::async_trait;

use crate::errors::ModuleError;
use super::emitter::ApCoreEvent;

/// Trait for receiving events from the EventEmitter.
#[async_trait]
pub trait EventSubscriber: Send + Sync + std::fmt::Debug {
    /// The event type pattern this subscriber is interested in.
    fn event_pattern(&self) -> &str;

    /// Handle an incoming event.
    async fn on_event(&self, event: &ApCoreEvent) -> Result<(), ModuleError>;
}

/// An event subscriber that delivers events to a webhook URL.
#[derive(Debug, Clone)]
pub struct WebhookSubscriber {
    pub url: String,
    pub event_pattern: String,
    pub headers: std::collections::HashMap<String, String>,
}

impl WebhookSubscriber {
    /// Create a new webhook subscriber.
    pub fn new(url: impl Into<String>, event_pattern: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            event_pattern: event_pattern.into(),
            headers: std::collections::HashMap::new(),
        }
    }
}

#[async_trait]
impl EventSubscriber for WebhookSubscriber {
    fn event_pattern(&self) -> &str {
        &self.event_pattern
    }

    async fn on_event(&self, _event: &ApCoreEvent) -> Result<(), ModuleError> {
        // TODO: Implement — HTTP POST to webhook URL
        todo!()
    }
}
