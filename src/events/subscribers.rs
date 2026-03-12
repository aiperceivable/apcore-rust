// APCore Protocol — Event subscribers
// Spec reference: Event subscription and webhook delivery

use async_trait::async_trait;

use super::emitter::ApCoreEvent;
use crate::errors::ModuleError;

/// Trait for receiving events from the EventEmitter.
#[async_trait]
pub trait EventSubscriber: Send + Sync + std::fmt::Debug {
    /// Unique ID for this subscriber (used by unsubscribe).
    fn subscriber_id(&self) -> &str;

    /// The event type pattern this subscriber is interested in.
    fn event_pattern(&self) -> &str;

    /// Handle an incoming event.
    async fn on_event(&self, event: &ApCoreEvent) -> Result<(), ModuleError>;
}

/// An event subscriber that delivers events to a webhook URL.
#[derive(Debug, Clone)]
pub struct WebhookSubscriber {
    pub id: String,
    pub url: String,
    pub event_pattern: String,
    pub headers: std::collections::HashMap<String, String>,
}

impl WebhookSubscriber {
    /// Create a new webhook subscriber.
    pub fn new(
        id: impl Into<String>,
        url: impl Into<String>,
        event_pattern: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            url: url.into(),
            event_pattern: event_pattern.into(),
            headers: std::collections::HashMap::new(),
        }
    }
}

#[async_trait]
impl EventSubscriber for WebhookSubscriber {
    fn subscriber_id(&self) -> &str {
        &self.id
    }

    fn event_pattern(&self) -> &str {
        &self.event_pattern
    }

    async fn on_event(&self, _event: &ApCoreEvent) -> Result<(), ModuleError> {
        // TODO: Implement — HTTP POST to webhook URL
        todo!()
    }
}
