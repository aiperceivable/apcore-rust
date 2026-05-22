// Conformance tests for Event Delivery Semantics (Issue #61).
// Fixture: apcore/conformance/fixtures/event_delivery_semantics.json
#![allow(clippy::pedantic)] // fixture-driven test file: casts and struct layouts follow fixture schema

use apcore::errors::{ErrorCode, ModuleError};
use apcore::events::emitter::{ApCoreEvent, EventEmitter};
use apcore::events::retry::EventRetryConfig;
use apcore::events::subscribers::EventSubscriber;
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Fixture loading
// ---------------------------------------------------------------------------

fn find_fixtures_root() -> PathBuf {
    if let Ok(spec_repo) = std::env::var("APCORE_SPEC_REPO") {
        let p = PathBuf::from(&spec_repo)
            .join("conformance")
            .join("fixtures");
        if p.is_dir() {
            return p;
        }
        panic!("APCORE_SPEC_REPO={spec_repo} does not contain conformance/fixtures/");
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let sibling = manifest_dir
        .parent()
        .unwrap()
        .join("apcore")
        .join("conformance")
        .join("fixtures");
    if sibling.is_dir() {
        return sibling;
    }
    panic!(
        "Cannot find apcore conformance fixtures.\n\
         Set APCORE_SPEC_REPO or clone apcore as a sibling of {}",
        manifest_dir.parent().unwrap().display()
    );
}

fn load_fixture() -> Value {
    let path = find_fixtures_root().join("event_delivery_semantics.json");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("Failed to read fixture: {}", path.display()));
    serde_json::from_str(&content).unwrap_or_else(|e| panic!("Invalid JSON: {e}"))
}

fn fixture_case<'a>(fixture: &'a Value, id: &str) -> &'a Value {
    fixture["test_cases"]
        .as_array()
        .expect("test_cases must be an array")
        .iter()
        .find(|c| c["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("test case '{id}' not found in fixture"))
}

// ---------------------------------------------------------------------------
// Test subscriber helpers
// ---------------------------------------------------------------------------

/// Fails the first `fail_count` attempts, then succeeds.
#[derive(Debug)]
struct FailNSubscriber {
    id: String,
    fail_count: u32,
    attempt_count: Arc<AtomicU32>,
    received: Arc<Mutex<Vec<String>>>,
    retry_config: EventRetryConfig,
}

#[async_trait]
impl EventSubscriber for FailNSubscriber {
    fn subscriber_id(&self) -> &str {
        &self.id
    }
    fn event_pattern(&self) -> &str {
        "*"
    }
    fn retry(&self) -> EventRetryConfig {
        self.retry_config
    }
    async fn on_event(&self, event: &ApCoreEvent) -> Result<(), ModuleError> {
        let attempt = self.attempt_count.fetch_add(1, Ordering::SeqCst);
        if attempt < self.fail_count {
            Err(ModuleError::new(
                ErrorCode::GeneralInternalError,
                "transient failure",
            ))
        } else {
            self.received.lock().push(event.event_type.clone());
            Ok(())
        }
    }
}

/// Always fails.
#[derive(Debug)]
struct AlwaysFailSubscriber {
    id: String,
    pattern: String,
    attempt_count: Arc<AtomicU32>,
    on_failure_count: Arc<AtomicU32>,
    retry_config: EventRetryConfig,
}

#[async_trait]
impl EventSubscriber for AlwaysFailSubscriber {
    fn subscriber_id(&self) -> &str {
        &self.id
    }
    fn event_pattern(&self) -> &str {
        &self.pattern
    }
    fn retry(&self) -> EventRetryConfig {
        self.retry_config
    }
    async fn on_event(&self, _event: &ApCoreEvent) -> Result<(), ModuleError> {
        self.attempt_count.fetch_add(1, Ordering::SeqCst);
        Err(ModuleError::new(
            ErrorCode::GeneralInternalError,
            "permanent failure",
        ))
    }
    async fn on_failure(&self, _event: &ApCoreEvent, _err: &ModuleError, _count: u32) {
        self.on_failure_count.fetch_add(1, Ordering::SeqCst);
    }
}

/// Records received events.
#[derive(Debug)]
struct RecordingSubscriber {
    id: String,
    pattern: String,
    received: Arc<Mutex<Vec<ApCoreEvent>>>,
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
        self.received.lock().push(event.clone());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Case: retry_succeeds_before_exhaustion
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_retry_succeeds_before_exhaustion() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "retry_succeeds_before_exhaustion");

    // Setup from fixture
    let retry_cfg = &case["setup"]["subscriber"]["retry"];
    let max_attempts = retry_cfg["max_attempts"].as_u64().unwrap() as u32;
    let initial_backoff_ms = retry_cfg["initial_backoff_ms"].as_u64().unwrap();
    let backoff_multiplier = retry_cfg["backoff_multiplier"].as_f64().unwrap();
    let fail_attempts_count = case["setup"]["subscriber"]["fail_attempts"]
        .as_array()
        .map(|a| a.len() as u32)
        .unwrap_or(2);

    let attempt_count = Arc::new(AtomicU32::new(0));
    let received = Arc::new(Mutex::new(Vec::new()));
    let sub = FailNSubscriber {
        id: case["setup"]["subscriber"]["id"]
            .as_str()
            .unwrap()
            .to_string(),
        fail_count: fail_attempts_count,
        attempt_count: Arc::clone(&attempt_count),
        received: Arc::clone(&received),
        retry_config: EventRetryConfig {
            max_attempts,
            initial_backoff_ms,
            max_backoff_ms: retry_cfg["max_backoff_ms"].as_u64().unwrap_or(100),
            backoff_multiplier,
        },
    };

    // DLQ recording subscriber
    let dlq_received = Arc::new(Mutex::new(Vec::new()));
    let dlq_sub = RecordingSubscriber {
        id: "dlq-recorder".to_string(),
        pattern: "apcore.event.delivery_failed".to_string(),
        received: Arc::clone(&dlq_received),
    };

    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(sub));
    emitter.subscribe(Box::new(dlq_sub));

    let event_name = case["trigger"]["event"]["name"]
        .as_str()
        .unwrap()
        .to_string();
    let event = ApCoreEvent::new(event_name, json!({"value": 42}));
    emitter.emit_delivery_semantics(event);

    // Allow tasks to settle
    tokio::time::sleep(Duration::from_millis(200)).await;

    let expected_attempts = case["expected"]["attempt_count"].as_u64().unwrap() as u32;
    let dlq_expected = case["expected"]["dlq_event_emitted"].as_bool().unwrap();

    // Verify attempt count
    assert_eq!(
        attempt_count.load(Ordering::SeqCst),
        expected_attempts,
        "attempt_count mismatch"
    );

    // Verify DLQ not emitted (succeeded before exhaustion)
    assert_eq!(
        !dlq_received.lock().is_empty(),
        dlq_expected,
        "dlq_event_emitted mismatch: expected={dlq_expected}"
    );

    // Verify the event was ultimately received by the subscriber
    assert!(
        !received.lock().is_empty(),
        "event must be received on success"
    );
}

// ---------------------------------------------------------------------------
// Case: permanent_failure_emits_dlq_event
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_permanent_failure_emits_dlq_event() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "permanent_failure_emits_dlq_event");

    let retry_cfg = &case["setup"]["subscriber"]["retry"];
    let max_attempts = retry_cfg["max_attempts"].as_u64().unwrap() as u32;

    let attempt_count = Arc::new(AtomicU32::new(0));
    let on_failure_count = Arc::new(AtomicU32::new(0));
    let event_name_for_pattern = case["trigger"]["event"]["name"]
        .as_str()
        .unwrap()
        .to_string();
    let sub = AlwaysFailSubscriber {
        id: case["setup"]["subscriber"]["id"]
            .as_str()
            .unwrap()
            .to_string(),
        // Use an exact pattern so this subscriber does NOT receive its own DLQ event.
        pattern: event_name_for_pattern.clone(),
        attempt_count: Arc::clone(&attempt_count),
        on_failure_count: Arc::clone(&on_failure_count),
        retry_config: EventRetryConfig {
            max_attempts,
            initial_backoff_ms: retry_cfg["initial_backoff_ms"].as_u64().unwrap_or(10),
            max_backoff_ms: 200,
            backoff_multiplier: 2.0,
        },
    };

    let dlq_received: Arc<Mutex<Vec<ApCoreEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let dlq_sub = RecordingSubscriber {
        id: "dlq-recorder".to_string(),
        pattern: "apcore.event.delivery_failed".to_string(),
        received: Arc::clone(&dlq_received),
    };

    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(sub));
    emitter.subscribe(Box::new(dlq_sub));

    let event_name = case["trigger"]["event"]["name"]
        .as_str()
        .unwrap()
        .to_string();
    let event = ApCoreEvent::new(event_name.clone(), json!({"service": "billing"}));
    emitter.emit_delivery_semantics(event);

    // Allow retry + DLQ to settle (3 attempts * 10ms each + overhead)
    tokio::time::sleep(Duration::from_millis(300)).await;

    let expected_attempts = case["expected"]["attempt_count"].as_u64().unwrap() as u32;
    assert_eq!(
        attempt_count.load(Ordering::SeqCst),
        expected_attempts,
        "attempt_count mismatch"
    );

    // DLQ emitted
    assert!(
        case["expected"]["dlq_event_emitted"].as_bool().unwrap(),
        "fixture must expect DLQ"
    );
    let dlq_events = dlq_received.lock();
    assert!(
        !dlq_events.is_empty(),
        "DLQ event must be emitted after exhaustion"
    );

    let dlq = &dlq_events[0];
    assert_eq!(dlq.event_type, "apcore.event.delivery_failed");

    // Verify required keys per fixture
    let required_keys: Vec<&str> = case["expected"]["dlq_event"]["data_required_keys"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    for key in &required_keys {
        assert!(
            dlq.data.get(key).is_some(),
            "DLQ event data missing required key: {key}"
        );
    }

    // Verify specific data values
    let data_contains = &case["expected"]["dlq_event"]["data_contains"];
    assert_eq!(
        dlq.data["subscriber_id"].as_str(),
        data_contains["subscriber_id"].as_str(),
        "subscriber_id mismatch"
    );
    assert_eq!(
        dlq.data["attempt_count"].as_u64(),
        data_contains["attempt_count"].as_u64(),
        "attempt_count in DLQ payload mismatch"
    );
    assert_eq!(
        dlq.data["original_event"]["event_type"].as_str(),
        data_contains["original_event"]["name"].as_str(),
        "original_event.event_type mismatch"
    );

    // on_failure was called
    assert_eq!(
        on_failure_count.load(Ordering::SeqCst),
        1,
        "on_failure must be called once"
    );
}

// ---------------------------------------------------------------------------
// Case: dlq_event_subscriber_failure_is_not_retried
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_dlq_event_subscriber_failure_is_not_retried() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "dlq_event_subscriber_failure_is_not_retried");

    let primary_cfg = &case["setup"]["primary_subscriber"]["retry"];
    let primary_max = primary_cfg["max_attempts"].as_u64().unwrap() as u32;
    let primary_attempts = Arc::new(AtomicU32::new(0));
    let primary_sub = AlwaysFailSubscriber {
        id: case["setup"]["primary_subscriber"]["id"]
            .as_str()
            .unwrap()
            .to_string(),
        // Explicit pattern so primary subscriber does NOT receive the DLQ event it triggers.
        pattern: "apcore.test.broken".to_string(),
        attempt_count: Arc::clone(&primary_attempts),
        on_failure_count: Arc::new(AtomicU32::new(0)),
        retry_config: EventRetryConfig {
            max_attempts: primary_max,
            initial_backoff_ms: primary_cfg["initial_backoff_ms"].as_u64().unwrap_or(10),
            max_backoff_ms: 200,
            backoff_multiplier: 2.0,
        },
    };

    // DLQ subscriber that always fails but has high retry count
    let dlq_cfg = &case["setup"]["dlq_subscriber"]["retry"];
    let dlq_attempts = Arc::new(AtomicU32::new(0));
    let dlq_sub = AlwaysFailSubscriber {
        id: case["setup"]["dlq_subscriber"]["id"]
            .as_str()
            .unwrap()
            .to_string(),
        pattern: "apcore.event.delivery_failed".to_string(),
        attempt_count: Arc::clone(&dlq_attempts),
        on_failure_count: Arc::new(AtomicU32::new(0)),
        retry_config: EventRetryConfig {
            max_attempts: dlq_cfg["max_attempts"].as_u64().unwrap() as u32,
            initial_backoff_ms: 5,
            max_backoff_ms: 50,
            backoff_multiplier: 1.0,
        },
    };

    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(primary_sub));
    emitter.subscribe(Box::new(dlq_sub));

    let event = ApCoreEvent::new("apcore.test.broken", json!({}));
    emitter.emit_delivery_semantics(event);

    // Allow primary retries + DLQ delivery attempt to settle
    tokio::time::sleep(Duration::from_millis(400)).await;

    let expected_primary = case["expected"]["primary_attempt_count"].as_u64().unwrap() as u32;
    assert_eq!(
        primary_attempts.load(Ordering::SeqCst),
        expected_primary,
        "primary subscriber attempt_count mismatch"
    );

    // DLQ subscriber is called EXACTLY once (no retry on DLQ delivery)
    let expected_dlq_attempts = case["expected"]["dlq_subscriber_attempt_count"]
        .as_u64()
        .unwrap() as u32;
    assert_eq!(
        dlq_attempts.load(Ordering::SeqCst),
        expected_dlq_attempts,
        "DLQ subscriber must be called exactly {expected_dlq_attempts} time(s) — DLQ delivery is never retried"
    );
}

// ---------------------------------------------------------------------------
// Case: subscriber_id_sdk_generated_when_omitted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_subscriber_id_sdk_generated_when_omitted() {
    let fixture = load_fixture();
    let _case = fixture_case(&fixture, "subscriber_id_sdk_generated_when_omitted");

    // Both subscribers omit explicit IDs — use StdoutSubscriber whose new()
    // auto-generates ids following "{type}-{counter}" convention.
    use apcore::events::subscribers::StdoutSubscriber;

    let s1 = StdoutSubscriber::new();
    let s2 = StdoutSubscriber::new();

    let id1 = s1.subscriber_id().to_string();
    let id2 = s2.subscriber_id().to_string();

    // IDs must be distinct
    assert_ne!(id1, id2, "auto-generated subscriber IDs must be distinct");

    // IDs must match the expected pattern "stdout-{something}"
    assert!(
        id1.starts_with("stdout-"),
        "generated ID must match 'stdout-{{...}}' pattern, got: {id1}"
    );
    assert!(
        id2.starts_with("stdout-"),
        "generated ID must match 'stdout-{{...}}' pattern, got: {id2}"
    );
}
