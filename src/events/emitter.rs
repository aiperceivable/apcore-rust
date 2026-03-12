// APCore Protocol — Event emitter
// Spec reference: Event types and emission

use serde::{Deserialize, Serialize};

use crate::errors::ModuleError;
use super::subscribers::EventSubscriber;

/// An event emitted by the APCore system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApCoreEvent {
    pub event_type: String,
    /// ISO 8601 timestamp string.
    pub timestamp: String,
    pub data: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_id: Option<String>,
    pub severity: String,
}

impl ApCoreEvent {
    /// Create a new event with "info" severity.
    pub fn new(event_type: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            event_type: event_type.into(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            data,
            module_id: None,
            severity: "info".to_string(),
        }
    }

    /// Create a new event with explicit module_id and severity.
    pub fn with_module(
        event_type: impl Into<String>,
        data: serde_json::Value,
        module_id: impl Into<String>,
        severity: impl Into<String>,
    ) -> Self {
        Self {
            event_type: event_type.into(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            data,
            module_id: Some(module_id.into()),
            severity: severity.into(),
        }
    }
}

/// Manages event subscribers and dispatches events.
#[derive(Debug)]
pub struct EventEmitter {
    subscribers: Vec<Box<dyn EventSubscriber>>,
    pub max_workers: usize,
}

impl EventEmitter {
    /// Create a new event emitter.
    pub fn new() -> Self {
        Self {
            subscribers: vec![],
            max_workers: 4,
        }
    }

    /// Add a subscriber. Returns the subscriber ID.
    pub fn subscribe(&mut self, subscriber: Box<dyn EventSubscriber>) -> String {
        let id = subscriber.subscriber_id().to_string();
        self.subscribers.push(subscriber);
        id
    }

    /// Remove a subscriber by ID.
    pub fn unsubscribe(&mut self, subscriber_id: &str) -> bool {
        let len_before = self.subscribers.len();
        self.subscribers.retain(|s| s.subscriber_id() != subscriber_id);
        self.subscribers.len() < len_before
    }

    /// Emit an event to all matching subscribers.
    pub async fn emit(&self, event: &ApCoreEvent) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Emit an event to subscribers matching the given event type pattern.
    pub async fn emit_filtered(
        &self,
        event: &ApCoreEvent,
        pattern: &str,
    ) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Flush all pending events, waiting up to timeout_ms milliseconds.
    pub async fn flush(&self, timeout_ms: u64) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }
}

impl Default for EventEmitter {
    fn default() -> Self {
        Self::new()
    }
}
