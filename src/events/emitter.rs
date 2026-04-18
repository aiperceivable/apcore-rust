// APCore Protocol — Event emitter
// Spec reference: Event types and emission

use serde::{Deserialize, Serialize};

use super::subscribers::EventSubscriber;
use crate::errors::ModuleError;

/// An event emitted by the `APCore` system.
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

    /// Create a new event with explicit `module_id` and severity.
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
    #[must_use]
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
        self.unsubscribe_by_id(target_id)
    }

    /// Remove the first subscriber whose `subscriber_id()` matches the given ID string.
    pub fn unsubscribe_by_id(&mut self, subscriber_id: &str) -> bool {
        let pos = self
            .subscribers
            .iter()
            .position(|s| s.subscriber_id() == subscriber_id);
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
                    tracing::warn!(
                        subscriber_id = %subscriber.subscriber_id(),
                        event_type = %event.event_type,
                        error = %e,
                        "event subscriber failed"
                    );
                }
            }
        }
        Ok(())
    }

    /// Emit an event to subscribers matching both the caller's filter pattern
    /// AND the subscriber's own `event_pattern`.
    pub async fn emit_filtered(
        &self,
        event: &ApCoreEvent,
        pattern: &str,
    ) -> Result<(), ModuleError> {
        for subscriber in &self.subscribers {
            if Self::matches_pattern(pattern, &event.event_type)
                && Self::matches_pattern(subscriber.event_pattern(), &event.event_type)
            {
                if let Err(e) = subscriber.on_event(event).await {
                    tracing::warn!(
                        subscriber_id = %subscriber.subscriber_id(),
                        event_type = %event.event_type,
                        error = %e,
                        "event subscriber failed"
                    );
                }
            }
        }
        Ok(())
    }

    /// Flush all pending events, waiting up to `timeout_ms` milliseconds.
    #[allow(clippy::unused_async)] // API stub for cross-language parity; future batched dispatch will await
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use parking_lot::Mutex;
    use serde_json::json;
    use std::sync::Arc;

    #[derive(Debug, Clone)]
    struct RecordingSubscriber {
        id: String,
        pattern: String,
        received: Arc<Mutex<Vec<String>>>,
    }

    impl RecordingSubscriber {
        fn new(id: &str, pattern: &str) -> Self {
            Self {
                id: id.to_string(),
                pattern: pattern.to_string(),
                received: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl EventSubscriber for RecordingSubscriber {
        fn subscriber_id(&self) -> &str {
            &self.id
        }
        fn event_pattern(&self) -> &str {
            &self.pattern
        }
        async fn on_event(&self, event: &ApCoreEvent) -> Result<(), ModuleError> {
            self.received.lock().push(event.event_type.clone());
            Ok(())
        }
    }

    #[test]
    fn test_event_new_defaults() {
        let event = ApCoreEvent::new("test.event", json!({"key": "val"}));
        assert_eq!(event.event_type, "test.event");
        assert_eq!(event.severity, "info");
        assert!(event.module_id.is_none());
        assert!(!event.timestamp.is_empty());
    }

    #[test]
    fn test_event_with_module() {
        let event = ApCoreEvent::with_module("err.event", json!({}), "mod.a", "error");
        assert_eq!(event.event_type, "err.event");
        assert_eq!(event.severity, "error");
        assert_eq!(event.module_id.as_deref(), Some("mod.a"));
    }

    #[test]
    fn test_event_serialization_skips_none_module_id() {
        let event = ApCoreEvent::new("test", json!(null));
        let serialized = serde_json::to_value(&event).unwrap();
        assert!(serialized.get("module_id").is_none());
    }

    #[test]
    fn test_emitter_default_max_workers() {
        let emitter = EventEmitter::new();
        assert_eq!(emitter.max_workers, 4);
    }

    #[tokio::test]
    async fn test_emit_to_matching_subscriber() {
        let mut emitter = EventEmitter::new();
        let sub = RecordingSubscriber::new("sub1", "test.*");
        let received = sub.received.clone();
        emitter.subscribe(Box::new(sub));

        let event = ApCoreEvent::new("test.hello", json!({}));
        emitter.emit(&event).await.unwrap();
        assert_eq!(received.lock().len(), 1);
        assert_eq!(received.lock()[0], "test.hello");
    }

    #[tokio::test]
    async fn test_emit_skips_non_matching_subscriber() {
        let mut emitter = EventEmitter::new();
        let sub = RecordingSubscriber::new("sub1", "other.*");
        let received = sub.received.clone();
        emitter.subscribe(Box::new(sub));

        let event = ApCoreEvent::new("test.hello", json!({}));
        emitter.emit(&event).await.unwrap();
        assert!(received.lock().is_empty());
    }

    #[tokio::test]
    async fn test_emit_wildcard_matches_all() {
        let mut emitter = EventEmitter::new();
        let sub = RecordingSubscriber::new("sub1", "*");
        let received = sub.received.clone();
        emitter.subscribe(Box::new(sub));

        let event = ApCoreEvent::new("anything.at.all", json!({}));
        emitter.emit(&event).await.unwrap();
        assert_eq!(received.lock().len(), 1);
    }

    #[tokio::test]
    async fn test_unsubscribe_by_id() {
        let mut emitter = EventEmitter::new();
        let sub = RecordingSubscriber::new("sub1", "*");
        emitter.subscribe(Box::new(sub));
        assert!(emitter.unsubscribe_by_id("sub1"));
        assert!(!emitter.unsubscribe_by_id("sub1"));
    }

    #[tokio::test]
    async fn test_unsubscribe_removes_subscriber() {
        let mut emitter = EventEmitter::new();
        let sub = RecordingSubscriber::new("sub1", "*");
        let received = sub.received.clone();
        emitter.subscribe(Box::new(sub.clone()));
        emitter.unsubscribe(&sub);

        let event = ApCoreEvent::new("test", json!({}));
        emitter.emit(&event).await.unwrap();
        assert!(received.lock().is_empty());
    }

    #[tokio::test]
    async fn test_emit_filtered() {
        let mut emitter = EventEmitter::new();
        let sub = RecordingSubscriber::new("sub1", "test.*");
        let received = sub.received.clone();
        emitter.subscribe(Box::new(sub));

        let event = ApCoreEvent::new("test.hello", json!({}));
        emitter.emit_filtered(&event, "test.*").await.unwrap();
        assert_eq!(received.lock().len(), 1);

        emitter.emit_filtered(&event, "other.*").await.unwrap();
        assert_eq!(received.lock().len(), 1);
    }

    #[tokio::test]
    async fn test_flush_succeeds() {
        let emitter = EventEmitter::new();
        emitter.flush(1000).await.unwrap();
    }

    #[test]
    fn test_matches_pattern_wildcard() {
        assert!(EventEmitter::matches_pattern("*", "anything"));
    }

    #[test]
    fn test_matches_pattern_exact() {
        assert!(EventEmitter::matches_pattern("test.event", "test.event"));
        assert!(!EventEmitter::matches_pattern("test.event", "test.other"));
    }

    #[test]
    fn test_matches_pattern_prefix_wildcard() {
        assert!(EventEmitter::matches_pattern("test.*", "test.hello"));
        assert!(EventEmitter::matches_pattern("test.*", "test."));
        assert!(!EventEmitter::matches_pattern("test.*", "other.hello"));
    }

    #[test]
    fn test_matches_pattern_suffix_wildcard() {
        assert!(EventEmitter::matches_pattern("*.event", "test.event"));
        assert!(!EventEmitter::matches_pattern("*.event", "test.other"));
    }

    #[test]
    fn test_matches_pattern_middle_wildcard() {
        assert!(EventEmitter::matches_pattern("a.*.z", "a.b.z"));
        assert!(EventEmitter::matches_pattern("a.*.z", "a.anything.z"));
        assert!(!EventEmitter::matches_pattern("a.*.z", "a.b.c"));
    }

    #[tokio::test]
    async fn test_emit_error_isolation() {
        #[derive(Debug)]
        struct FailingSub;

        #[async_trait]
        impl EventSubscriber for FailingSub {
            fn subscriber_id(&self) -> &'static str {
                "fail"
            }
            fn event_pattern(&self) -> &'static str {
                "*"
            }
            async fn on_event(&self, _event: &ApCoreEvent) -> Result<(), ModuleError> {
                Err(ModuleError::new(
                    crate::errors::ErrorCode::GeneralInternalError,
                    "boom",
                ))
            }
        }

        let mut emitter = EventEmitter::new();
        emitter.subscribe(Box::new(FailingSub));
        let good_sub = RecordingSubscriber::new("good", "*");
        let received = good_sub.received.clone();
        emitter.subscribe(Box::new(good_sub));

        let event = ApCoreEvent::new("test", json!({}));
        emitter.emit(&event).await.unwrap();
        assert_eq!(received.lock().len(), 1);
    }
}
