//! Tests for the events subsystem — EventEmitter, subscribers, and factory.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;

use apcore::errors::{ErrorCode, ModuleError};
use apcore::events::emitter::{ApCoreEvent, EventEmitter};
use apcore::events::subscribers::{
    create_subscriber, register_subscriber_type, reset_subscriber_registry,
    unregister_subscriber_type, A2ASubscriber, EventSubscriber, WebhookSubscriber,
};

// ---------------------------------------------------------------------------
// Test subscriber — records received events for assertions
// ---------------------------------------------------------------------------

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

    #[allow(dead_code)] // useful for debugging test failures
    fn received_events(&self) -> Vec<String> {
        self.received.lock().unwrap().clone()
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
        self.received.lock().unwrap().push(event.event_type.clone());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Failing subscriber — always returns an error
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct FailingSubscriber {
    id: String,
    pattern: String,
}

#[async_trait]
impl EventSubscriber for FailingSubscriber {
    fn subscriber_id(&self) -> &str {
        &self.id
    }

    fn event_pattern(&self) -> &str {
        &self.pattern
    }

    async fn on_event(&self, _event: &ApCoreEvent) -> Result<(), ModuleError> {
        Err(ModuleError::new(
            ErrorCode::GeneralInternalError,
            "deliberate failure",
        ))
    }
}

// ---------------------------------------------------------------------------
// ApCoreEvent construction tests
// ---------------------------------------------------------------------------

#[test]
fn test_event_new_sets_defaults() {
    let event = ApCoreEvent::new("module.loaded", json!({"key": "value"}));
    assert_eq!(event.event_type, "module.loaded");
    assert_eq!(event.severity, "info");
    assert!(event.module_id.is_none());
    assert!(!event.timestamp.is_empty());
}

#[test]
fn test_event_with_module() {
    let event = ApCoreEvent::with_module(
        "module.error",
        json!({"reason": "timeout"}),
        "executor.email.send",
        "error",
    );
    assert_eq!(event.event_type, "module.error");
    assert_eq!(event.module_id.as_deref(), Some("executor.email.send"));
    assert_eq!(event.severity, "error");
}

#[test]
fn test_event_serialization_roundtrip() {
    let event = ApCoreEvent::with_module("test.event", json!(42), "mod_a", "warning");
    let json_str = serde_json::to_string(&event).expect("serialize");
    let restored: ApCoreEvent = serde_json::from_str(&json_str).expect("deserialize");
    assert_eq!(restored.event_type, "test.event");
    assert_eq!(restored.severity, "warning");
    assert_eq!(restored.module_id.as_deref(), Some("mod_a"));
}

#[test]
fn test_event_module_id_omitted_when_none() {
    let event = ApCoreEvent::new("x", json!(null));
    let v = serde_json::to_value(&event).expect("serialize");
    assert!(v.get("module_id").is_none());
}

// ---------------------------------------------------------------------------
// EventEmitter — subscribe / emit / unsubscribe
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_emitter_subscribe_and_emit() {
    let sub = RecordingSubscriber::new("sub-1", "*");
    let received = sub.received.clone();

    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(sub));

    let event = ApCoreEvent::new("test.ping", json!({}));
    emitter.emit(&event).await.expect("emit should succeed");

    let events = received.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0], "test.ping");
}

#[tokio::test]
async fn test_emitter_pattern_filtering() {
    let sub_all = RecordingSubscriber::new("all", "*");
    let sub_mod = RecordingSubscriber::new("mod", "module.*");
    let sub_exact = RecordingSubscriber::new("exact", "module.loaded");

    let recv_all = sub_all.received.clone();
    let recv_mod = sub_mod.received.clone();
    let recv_exact = sub_exact.received.clone();

    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(sub_all));
    emitter.subscribe(Box::new(sub_mod));
    emitter.subscribe(Box::new(sub_exact));

    emitter
        .emit(&ApCoreEvent::new("module.loaded", json!({})))
        .await
        .unwrap();
    emitter
        .emit(&ApCoreEvent::new("module.error", json!({})))
        .await
        .unwrap();
    emitter
        .emit(&ApCoreEvent::new("registry.changed", json!({})))
        .await
        .unwrap();

    assert_eq!(recv_all.lock().unwrap().len(), 3);
    assert_eq!(recv_mod.lock().unwrap().len(), 2);
    assert_eq!(recv_exact.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn test_emitter_unsubscribe_by_id() {
    let sub = RecordingSubscriber::new("remove-me", "*");
    let received = sub.received.clone();

    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(sub.clone()));

    assert!(emitter.unsubscribe_by_id("remove-me"));
    // Second call should return false (already removed).
    assert!(!emitter.unsubscribe_by_id("remove-me"));

    emitter
        .emit(&ApCoreEvent::new("post.remove", json!({})))
        .await
        .unwrap();

    assert!(received.lock().unwrap().is_empty());
}

#[tokio::test]
async fn test_emitter_unsubscribe_via_trait() {
    let sub = RecordingSubscriber::new("trait-unsub", "*");
    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(sub.clone()));

    assert!(emitter.unsubscribe(&sub));
    assert!(!emitter.unsubscribe(&sub));
}

#[tokio::test]
async fn test_emitter_unsubscribe_nonexistent_returns_false() {
    let emitter = EventEmitter::new();
    let sub = RecordingSubscriber::new("ghost", "*");
    // No mutable needed — we check a non-mutating path; but unsubscribe_by_id needs &mut.
    let mut emitter = emitter;
    assert!(!emitter.unsubscribe(&sub));
}

#[tokio::test]
async fn test_emitter_error_isolation() {
    // A failing subscriber must not prevent other subscribers from receiving events.
    let good = RecordingSubscriber::new("good", "*");
    let received = good.received.clone();

    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(FailingSubscriber {
        id: "bad".into(),
        pattern: "*".into(),
    }));
    emitter.subscribe(Box::new(good));

    let result = emitter.emit(&ApCoreEvent::new("err.test", json!({}))).await;

    // emit itself should still succeed (errors are logged, not propagated).
    assert!(result.is_ok());
    assert_eq!(received.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn test_emitter_no_subscribers_is_ok() {
    let emitter = EventEmitter::new();
    let result = emitter
        .emit(&ApCoreEvent::new("lonely.event", json!({})))
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_emitter_emit_filtered() {
    let sub = RecordingSubscriber::new("s1", "module.*");
    let received = sub.received.clone();

    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(sub));

    // Caller filter "module.loaded" AND subscriber pattern "module.*" both match.
    emitter
        .emit_filtered(&ApCoreEvent::new("module.loaded", json!({})), "module.*")
        .await
        .unwrap();

    // Caller filter "registry.*" does NOT match "module.loaded", so subscriber skipped.
    emitter
        .emit_filtered(&ApCoreEvent::new("module.loaded", json!({})), "registry.*")
        .await
        .unwrap();

    assert_eq!(received.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn test_emitter_flush_is_noop() {
    let emitter = EventEmitter::new();
    let result = emitter.flush(1000).await;
    assert!(result.is_ok());
}

#[test]
fn test_emitter_default_max_workers() {
    let emitter = EventEmitter::new();
    assert_eq!(emitter.max_workers, 4);
}

#[test]
fn test_emitter_default_trait() {
    let emitter = EventEmitter::default();
    assert_eq!(emitter.max_workers, 4);
}

// ---------------------------------------------------------------------------
// Pattern matching edge cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_pattern_wildcard_matches_everything() {
    let sub = RecordingSubscriber::new("s", "*");
    let received = sub.received.clone();
    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(sub));

    emitter
        .emit(&ApCoreEvent::new("anything.at.all", json!({})))
        .await
        .unwrap();
    assert_eq!(received.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn test_pattern_exact_match() {
    let sub = RecordingSubscriber::new("s", "exact.match");
    let received = sub.received.clone();
    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(sub));

    emitter
        .emit(&ApCoreEvent::new("exact.match", json!({})))
        .await
        .unwrap();
    emitter
        .emit(&ApCoreEvent::new("exact.match.extra", json!({})))
        .await
        .unwrap();

    let events = received.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0], "exact.match");
}

#[tokio::test]
async fn test_pattern_prefix_wildcard() {
    let sub = RecordingSubscriber::new("s", "foo.*");
    let received = sub.received.clone();
    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(sub));

    emitter
        .emit(&ApCoreEvent::new("foo.bar", json!({})))
        .await
        .unwrap();
    emitter
        .emit(&ApCoreEvent::new("foo.baz.qux", json!({})))
        .await
        .unwrap();
    emitter
        .emit(&ApCoreEvent::new("bar.foo", json!({})))
        .await
        .unwrap();

    let events = received.lock().unwrap();
    assert_eq!(events.len(), 2);
}

#[tokio::test]
async fn test_pattern_no_match() {
    let sub = RecordingSubscriber::new("s", "alpha.beta");
    let received = sub.received.clone();
    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(sub));

    emitter
        .emit(&ApCoreEvent::new("gamma.delta", json!({})))
        .await
        .unwrap();

    assert!(received.lock().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// WebhookSubscriber and A2ASubscriber unit tests (construction only)
// ---------------------------------------------------------------------------

#[test]
fn test_webhook_subscriber_defaults() {
    let ws = WebhookSubscriber::new("wh-1", "https://example.com/hook", "module.*");
    assert_eq!(ws.subscriber_id(), "wh-1");
    assert_eq!(ws.event_pattern(), "module.*");
    assert_eq!(ws.retry_count, 3);
    assert_eq!(ws.timeout_ms, 5000);
    assert!(ws.headers.is_empty());
}

#[test]
fn test_a2a_subscriber_defaults() {
    let a2a = A2ASubscriber::new("a2a-1", "https://platform.example.com", "*");
    assert_eq!(a2a.subscriber_id(), "a2a-1");
    assert_eq!(a2a.event_pattern(), "*");
    assert_eq!(a2a.timeout_ms, 5000);
    assert!(a2a.auth.is_none());
}

// ---------------------------------------------------------------------------
// Subscriber factory tests — single sequential test to avoid global state races
// ---------------------------------------------------------------------------

#[test]
fn test_subscriber_factory_operations() {
    reset_subscriber_registry();

    // Create webhook subscriber
    let config = json!({ "type": "webhook", "url": "https://example.com/hook" });
    let sub = create_subscriber(&config).expect("should create webhook subscriber");
    assert!(sub.subscriber_id().starts_with("webhook-"));
    assert_eq!(sub.event_pattern(), "*");

    // Create a2a subscriber
    let config = json!({ "type": "a2a", "platform_url": "https://platform.example.com" });
    let sub = create_subscriber(&config).expect("should create a2a subscriber");
    assert!(sub.subscriber_id().starts_with("a2a-"));

    // Missing type field
    let config = json!({ "url": "https://example.com" });
    let err = create_subscriber(&config).unwrap_err();
    assert_eq!(err.code, ErrorCode::ConfigInvalid);

    // Unknown type
    let config = json!({ "type": "unknown" });
    let err = create_subscriber(&config).unwrap_err();
    assert_eq!(err.code, ErrorCode::ConfigInvalid);

    // Webhook missing url
    let config = json!({ "type": "webhook" });
    let err = create_subscriber(&config).unwrap_err();
    assert_eq!(err.code, ErrorCode::ConfigInvalid);

    // A2A missing platform_url
    let config = json!({ "type": "a2a" });
    let err = create_subscriber(&config).unwrap_err();
    assert_eq!(err.code, ErrorCode::ConfigInvalid);

    // Register custom type
    register_subscriber_type(
        "custom",
        Box::new(|config| {
            let id = config
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("custom-default")
                .to_string();
            Ok(Box::new(RecordingSubscriber::new(&id, "*")) as Box<dyn EventSubscriber>)
        }),
    );
    let config = json!({ "type": "custom", "id": "my-custom" });
    let sub = create_subscriber(&config).expect("should create custom subscriber");
    assert_eq!(sub.subscriber_id(), "my-custom");

    // Unregister webhook
    assert!(unregister_subscriber_type("webhook").is_ok());
    let config = json!({ "type": "webhook", "url": "https://example.com" });
    assert!(create_subscriber(&config).is_err());

    // Unregister nonexistent type
    let err = unregister_subscriber_type("nonexistent").unwrap_err();
    assert_eq!(err.code, ErrorCode::GeneralInternalError);

    // Reset restores built-ins
    reset_subscriber_registry();
    let config = json!({ "type": "webhook", "url": "https://restored.com" });
    assert!(create_subscriber(&config).is_ok());
}
