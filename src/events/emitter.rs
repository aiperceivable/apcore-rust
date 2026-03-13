// APCore Protocol — Event emitter
// Spec reference: Event types and emission

use serde::{Deserialize, Serialize};

use super::subscribers::EventSubscriber;
use crate::errors::ModuleError;

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

    /// Add a subscriber (matching Python's void return signature).
    pub fn subscribe(&mut self, subscriber: Box<dyn EventSubscriber>) {
        self.subscribers.push(subscriber);
    }

    /// Remove the first subscriber whose `subscriber_id()` matches the given
    /// subscriber's ID, matching Python's identity-based removal semantics.
    pub fn unsubscribe(&mut self, subscriber: &dyn EventSubscriber) -> bool {
        let target_id = subscriber.subscriber_id();
        let pos = self.subscribers.iter().position(|s| s.subscriber_id() == target_id);
        if let Some(i) = pos {
            self.subscribers.remove(i);
            true
        } else {
            false
        }
    }

    /// Emit an event to all subscribers whose pattern matches the event type.
    ///
    /// Errors from individual subscribers are logged but not propagated
    /// (error isolation), matching Python's behaviour.
    pub async fn emit(&self, event: &ApCoreEvent) -> Result<(), ModuleError> {
        for subscriber in &self.subscribers {
            if Self::matches_pattern(subscriber.event_pattern(), &event.event_type) {
                if let Err(e) = subscriber.on_event(event).await {
                    eprintln!(
                        "Subscriber {} failed: {}",
                        subscriber.subscriber_id(),
                        e
                    );
                }
            }
        }
        Ok(())
    }

    /// Emit an event to subscribers matching the given event type pattern.
    pub async fn emit_filtered(
        &self,
        event: &ApCoreEvent,
        pattern: &str,
    ) -> Result<(), ModuleError> {
        for subscriber in &self.subscribers {
            if Self::matches_pattern(pattern, &event.event_type) {
                if let Err(e) = subscriber.on_event(event).await {
                    eprintln!(
                        "Subscriber {} failed: {}",
                        subscriber.subscriber_id(),
                        e
                    );
                }
            }
        }
        Ok(())
    }

    /// Flush all pending events, waiting up to timeout_ms milliseconds.
    pub async fn flush(&self, _timeout_ms: u64) -> Result<(), ModuleError> {
        // Synchronous dispatch model — nothing to flush.
        Ok(())
    }

    /// Simple glob-style pattern matching with `*` wildcard.
    ///
    /// - `"*"` matches everything.
    /// - `"foo.*"` matches `"foo.bar"`, `"foo.baz"`, etc.
    /// - An exact string matches only itself.
    fn matches_pattern(pattern: &str, event_type: &str) -> bool {
        if pattern == "*" {
            return true;
        }
        // Split pattern by '*' and check that all parts appear in order.
        let parts: Vec<&str> = pattern.split('*').collect();
        let mut remaining = event_type;
        for (i, part) in parts.iter().enumerate() {
            if part.is_empty() {
                continue;
            }
            if i == 0 {
                // First part must be a prefix.
                if let Some(rest) = remaining.strip_prefix(part) {
                    remaining = rest;
                } else {
                    return false;
                }
            } else if let Some(pos) = remaining.find(part) {
                remaining = &remaining[pos + part.len()..];
            } else {
                return false;
            }
        }
        // If pattern doesn't end with *, remaining must be empty.
        if !pattern.ends_with('*') && !remaining.is_empty() {
            return false;
        }
        true
    }
}

impl Default for EventEmitter {
    fn default() -> Self {
        Self::new()
    }
}
