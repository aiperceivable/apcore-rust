// Issue #61 — Event delivery semantics: retry, DLQ, on_failure
// Tests emit_delivery_semantics() with retry configurations.

use apcore::errors::ModuleError;
use apcore::events::emitter::{ApCoreEvent, EventEmitter};
use apcore::events::retry::EventRetryConfig;
use apcore::events::subscribers::EventSubscriber;
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::json;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Helper: subscriber that fails N times then succeeds
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct CountingSubscriber {
    id: String,
    pattern: String,
    fail_count: u32,
    attempt_count: Arc<AtomicU32>,
    received: Arc<Mutex<Vec<String>>>,
    retry_config: EventRetryConfig,
}

impl CountingSubscriber {
    fn new(
        id: &str,
        fail_count: u32,
        retry_config: EventRetryConfig,
    ) -> (Self, Arc<AtomicU32>, Arc<Mutex<Vec<String>>>) {
        let attempt_count = Arc::new(AtomicU32::new(0));
        let received = Arc::new(Mutex::new(Vec::new()));
        let sub = Self {
            id: id.to_string(),
            pattern: "*".to_string(),
            fail_count,
            attempt_count: Arc::clone(&attempt_count),
            received: Arc::clone(&received),
            retry_config,
        };
        (sub, attempt_count, received)
    }
}

#[async_trait]
impl EventSubscriber for CountingSubscriber {
    fn subscriber_id(&self) -> &str {
        &self.id
    }
    fn event_pattern(&self) -> &str {
        &self.pattern
    }
    fn retry(&self) -> EventRetryConfig {
        self.retry_config
    }
    async fn on_event(&self, event: &ApCoreEvent) -> Result<(), ModuleError> {
        let attempt = self.attempt_count.fetch_add(1, Ordering::SeqCst);
        if attempt < self.fail_count {
            Err(ModuleError::new(
                apcore::errors::ErrorCode::GeneralInternalError,
                format!("intentional failure on attempt {attempt}"),
            ))
        } else {
            self.received.lock().push(event.event_type.clone());
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: subscriber that always fails (for DLQ tests)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct AlwaysFailSubscriber {
    id: String,
    attempt_count: Arc<AtomicU32>,
    failure_recorded: Arc<Mutex<Option<u32>>>,
    retry_config: EventRetryConfig,
}

impl AlwaysFailSubscriber {
    fn new(
        id: &str,
        retry_config: EventRetryConfig,
    ) -> (Self, Arc<AtomicU32>, Arc<Mutex<Option<u32>>>) {
        let attempt_count = Arc::new(AtomicU32::new(0));
        let failure_recorded = Arc::new(Mutex::new(None));
        let sub = Self {
            id: id.to_string(),
            attempt_count: Arc::clone(&attempt_count),
            failure_recorded: Arc::clone(&failure_recorded),
            retry_config,
        };
        (sub, attempt_count, failure_recorded)
    }
}

#[async_trait]
impl EventSubscriber for AlwaysFailSubscriber {
    fn subscriber_id(&self) -> &str {
        &self.id
    }
    // Use a specific pattern to avoid receiving the DLQ event itself.
    #[allow(clippy::unnecessary_literal_bound)]
    fn event_pattern(&self) -> &str {
        "test.*"
    }
    fn retry(&self) -> EventRetryConfig {
        self.retry_config
    }
    async fn on_event(&self, _event: &ApCoreEvent) -> Result<(), ModuleError> {
        self.attempt_count.fetch_add(1, Ordering::SeqCst);
        Err(ModuleError::new(
            apcore::errors::ErrorCode::GeneralInternalError,
            "always fails",
        ))
    }
    async fn on_failure(&self, _event: &ApCoreEvent, _error: &ModuleError, attempt_count: u32) {
        *self.failure_recorded.lock() = Some(attempt_count);
    }
}

// ---------------------------------------------------------------------------
// Helper: DLQ subscriber that captures events
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct DlqSubscriber {
    id: String,
    received: Arc<Mutex<Vec<ApCoreEvent>>>,
}

impl DlqSubscriber {
    fn new(id: &str) -> (Self, Arc<Mutex<Vec<ApCoreEvent>>>) {
        let received = Arc::new(Mutex::new(Vec::new()));
        let sub = Self {
            id: id.to_string(),
            received: Arc::clone(&received),
        };
        (sub, received)
    }
}

#[async_trait]
impl EventSubscriber for DlqSubscriber {
    fn subscriber_id(&self) -> &str {
        &self.id
    }
    #[allow(clippy::unnecessary_literal_bound)]
    fn event_pattern(&self) -> &str {
        "apcore.event.delivery_failed"
    }
    async fn on_event(&self, event: &ApCoreEvent) -> Result<(), ModuleError> {
        self.received.lock().push(event.clone());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retry_before_exhaustion_succeeds_on_third_attempt() {
    // fails twice, succeeds on 3rd — no DLQ emitted
    let retry_cfg = EventRetryConfig {
        max_attempts: 3,
        initial_backoff_ms: 1, // 1ms for fast tests
        max_backoff_ms: 10,
        backoff_multiplier: 2.0,
    };
    let (sub, attempt_count, received) = CountingSubscriber::new("sub-1", 2, retry_cfg);

    let (dlq_sub, dlq_received) = DlqSubscriber::new("dlq-1");

    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(sub));
    emitter.subscribe(Box::new(dlq_sub));

    let event = ApCoreEvent::new("test.retry", json!({"v": 1}));
    emitter.emit_delivery_semantics(event);

    // Wait for async tasks to complete.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Exactly 3 attempts (2 failures + 1 success).
    assert_eq!(attempt_count.load(Ordering::SeqCst), 3);
    // Event delivered successfully.
    assert_eq!(received.lock().as_slice(), ["test.retry"]);
    // No DLQ emitted (delivery succeeded before exhaustion).
    assert!(dlq_received.lock().is_empty(), "no DLQ on success");
}

#[tokio::test]
async fn permanent_failure_emits_dlq_event() {
    // Always fails → DLQ emitted after exhausting max_attempts.
    let retry_cfg = EventRetryConfig {
        max_attempts: 3,
        initial_backoff_ms: 1,
        max_backoff_ms: 10,
        backoff_multiplier: 2.0,
    };
    let (sub, attempt_count, failure_recorded) = AlwaysFailSubscriber::new("sub-fail", retry_cfg);
    let (dlq_sub, dlq_received) = DlqSubscriber::new("dlq-2");

    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(sub));
    emitter.subscribe(Box::new(dlq_sub));

    let event = ApCoreEvent::new("test.permanent_fail", json!({}));
    emitter.emit_delivery_semantics(event);

    tokio::time::sleep(Duration::from_millis(150)).await;

    // All 3 attempts made.
    assert_eq!(attempt_count.load(Ordering::SeqCst), 3);
    // DLQ received exactly 1 event.
    let dlq_events = dlq_received.lock();
    assert_eq!(dlq_events.len(), 1, "exactly one DLQ event");
    // DLQ payload has expected keys.
    let dlq_data = &dlq_events[0].data;
    assert!(
        dlq_data.get("subscriber_id").is_some(),
        "dlq has subscriber_id"
    );
    assert!(
        dlq_data.get("original_event").is_some(),
        "dlq has original_event"
    );
    assert!(dlq_data.get("error").is_some(), "dlq has error");
    assert!(
        dlq_data.get("attempt_count").is_some(),
        "dlq has attempt_count"
    );
    assert_eq!(
        dlq_data["attempt_count"].as_u64(),
        Some(3),
        "attempt_count matches max_attempts"
    );
    // on_failure was called with correct attempt count.
    assert_eq!(*failure_recorded.lock(), Some(3));
}

#[tokio::test]
async fn single_attempt_failure_logs_warn_no_dlq() {
    // Default retry (single attempt) — no DLQ emitted, only a warn log.
    let (sub, attempt_count, failure_recorded) =
        AlwaysFailSubscriber::new("sub-single", EventRetryConfig::default());
    let (dlq_sub, dlq_received) = DlqSubscriber::new("dlq-single");

    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(sub));
    emitter.subscribe(Box::new(dlq_sub));

    let event = ApCoreEvent::new("test.single", json!({}));
    emitter.emit_delivery_semantics(event);

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Exactly 1 attempt (default single-attempt).
    assert_eq!(attempt_count.load(Ordering::SeqCst), 1);
    // No DLQ emitted for single-attempt mode.
    assert!(
        dlq_received.lock().is_empty(),
        "no DLQ for single-attempt mode"
    );
    // on_failure was still called.
    assert_eq!(*failure_recorded.lock(), Some(1));
}

#[tokio::test]
async fn shutdown_drops_event_as_noop() {
    let retry_cfg = EventRetryConfig {
        max_attempts: 3,
        initial_backoff_ms: 1,
        max_backoff_ms: 10,
        backoff_multiplier: 2.0,
    };
    let (sub, attempt_count, _received) = CountingSubscriber::new("sub-shutdown", 0, retry_cfg);

    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(sub));

    // Shut down before emitting.
    emitter.shutdown(100).await.unwrap();

    let event = ApCoreEvent::new("test.after_shutdown", json!({}));
    emitter.emit_delivery_semantics(event);

    tokio::time::sleep(Duration::from_millis(50)).await;

    // No attempts made after shutdown.
    assert_eq!(attempt_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn sdk_generated_subscriber_ids_are_distinct() {
    // Verify that multiple instances of the same built-in subscriber type
    // get distinct IDs (SDK-generated id pattern).
    use apcore::events::subscribers::StdoutSubscriber;

    let s1 = StdoutSubscriber::new();
    let s2 = StdoutSubscriber::new();
    assert_ne!(
        s1.subscriber_id(),
        s2.subscriber_id(),
        "each StdoutSubscriber instance must have a unique ID"
    );
    assert!(
        s1.subscriber_id().starts_with("stdout-"),
        "ID format: stdout-<uuid>"
    );
}

#[tokio::test]
async fn dlq_payload_contains_original_event_type() {
    let retry_cfg = EventRetryConfig {
        max_attempts: 2,
        initial_backoff_ms: 1,
        max_backoff_ms: 10,
        backoff_multiplier: 2.0,
    };
    let (sub, _attempts, _failure) = AlwaysFailSubscriber::new("sub-dlq-payload", retry_cfg);
    let (dlq_sub, dlq_received) = DlqSubscriber::new("dlq-payload");

    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(sub));
    emitter.subscribe(Box::new(dlq_sub));

    // The pattern for AlwaysFailSubscriber is "test.*", so use a test.* event.
    let event = ApCoreEvent::new("test.specific.event", json!({"key": "value"}));
    emitter.emit_delivery_semantics(event);

    tokio::time::sleep(Duration::from_millis(100)).await;

    let dlq_events = dlq_received.lock();
    assert_eq!(dlq_events.len(), 1);
    let original_event = &dlq_events[0].data["original_event"];
    assert_eq!(
        original_event["event_type"].as_str(),
        Some("test.specific.event"),
        "DLQ payload must include original event_type"
    );
}
