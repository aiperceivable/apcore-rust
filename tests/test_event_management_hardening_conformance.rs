//! Cross-language conformance tests for Event Management Hardening (Issue #36).
//!
//! Fixture source: apcore/conformance/fixtures/event_management_hardening.json
//! Spec reference: apcore/docs/features/event-system.md (## Event Management Hardening)
//!
//! Each fixture case verifies one normative rule of the cross-language
//! SubscriberFactory parity, the new built-in `file` / `stdout` / `filter`
//! subscribers, and the per-subscriber circuit-breaker state machine.

#![allow(clippy::missing_panics_doc)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
// parking_lot::Mutex is preferred over std::sync::Mutex in async tests because
// it sidesteps clippy's `await_holding_lock` warning while still being safe to
// hold during await points (we only need the lock for registry hygiene, never
// for cross-await coordination).
use parking_lot::Mutex;
use regex::Regex;
use serde_json::{json, Value};

use apcore::errors::{ErrorCode, ModuleError};
use apcore::events::circuit_breaker::{CircuitBreakerWrapper, CircuitEventSink, CircuitState};
use apcore::events::emitter::ApCoreEvent;
use apcore::events::subscribers::{
    create_subscriber, register_subscriber_type, reset_subscriber_registry, EventSubscriber,
};

// ---------------------------------------------------------------------------
// Fixture loading (mirrors other conformance tests)
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
         Fix one of:\n\
         1. Set APCORE_SPEC_REPO to the apcore spec repo path\n\
         2. Clone apcore as a sibling: git clone <apcore-url> {}\n",
        manifest_dir.parent().unwrap().join("apcore").display()
    );
}

fn load_fixture() -> Value {
    let path = find_fixtures_root().join("event_management_hardening.json");
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
        .unwrap_or_else(|| panic!("fixture case '{id}' not present"))
}

fn parse_event(value: &Value) -> ApCoreEvent {
    ApCoreEvent {
        event_type: value["event_type"].as_str().unwrap().to_string(),
        timestamp: value["timestamp"].as_str().unwrap().to_string(),
        data: value["data"].clone(),
        module_id: value
            .get("module_id")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        severity: value["severity"].as_str().unwrap().to_string(),
    }
}

// ---------------------------------------------------------------------------
// Test helpers — recording subscriber and circuit-event sink
// ---------------------------------------------------------------------------

/// Tests that mutate the global subscriber-factory registry hold this lock to
/// avoid cross-test races. Mirrors the convention used in
/// `src/events/subscribers.rs`.
fn registry_lock() -> &'static Mutex<()> {
    static LOCK: Mutex<()> = Mutex::new(());
    &LOCK
}

#[derive(Debug, Default)]
struct CallRecord {
    received: Mutex<Vec<String>>,
}

impl CallRecord {
    fn count(&self) -> usize {
        self.received.lock().len()
    }
}

#[derive(Debug)]
struct RecordingSubscriber {
    id: String,
    record: Arc<CallRecord>,
}

#[async_trait]
impl EventSubscriber for RecordingSubscriber {
    fn subscriber_id(&self) -> &str {
        &self.id
    }
    #[allow(clippy::unnecessary_literal_bound)]
    fn event_pattern(&self) -> &str {
        "*"
    }
    async fn on_event(&self, event: &ApCoreEvent) -> Result<(), ModuleError> {
        self.record.received.lock().push(event.event_type.clone());
        Ok(())
    }
}

#[derive(Debug, Default)]
struct CapturingSink {
    events: Mutex<Vec<ApCoreEvent>>,
}

impl CapturingSink {
    fn captured(&self) -> Vec<ApCoreEvent> {
        self.events.lock().clone()
    }
}

impl CircuitEventSink for CapturingSink {
    fn emit_circuit_event(&self, event: ApCoreEvent) {
        self.events.lock().push(event);
    }
}

#[derive(Debug)]
struct AlwaysFail {
    id: String,
    calls: Arc<AtomicU32>,
}

#[async_trait]
impl EventSubscriber for AlwaysFail {
    fn subscriber_id(&self) -> &str {
        &self.id
    }
    #[allow(clippy::unnecessary_literal_bound)]
    fn event_pattern(&self) -> &str {
        "*"
    }
    async fn on_event(&self, _event: &ApCoreEvent) -> Result<(), ModuleError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(ModuleError::new(
            ErrorCode::GeneralInternalError,
            "intentional fixture failure",
        ))
    }
}

#[derive(Debug)]
struct AlwaysOk {
    id: String,
    calls: Arc<AtomicU32>,
}

#[async_trait]
impl EventSubscriber for AlwaysOk {
    fn subscriber_id(&self) -> &str {
        &self.id
    }
    #[allow(clippy::unnecessary_literal_bound)]
    fn event_pattern(&self) -> &str {
        "*"
    }
    async fn on_event(&self, _event: &ApCoreEvent) -> Result<(), ModuleError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Case 1 — subscriber_factory_registered_type
// ---------------------------------------------------------------------------

#[test]
fn conformance_subscriber_factory_registered_type() {
    let _guard = registry_lock().lock();
    reset_subscriber_registry();

    let fixture = load_fixture();
    let case = fixture_case(&fixture, "subscriber_factory_registered_type");
    let subscriber_config = &case["input"]["subscriber_config"];
    let registered_types: Vec<String> = case["input"]["registered_types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(registered_types, vec!["slack".to_string()]);

    register_subscriber_type(
        "slack",
        Box::new(|config| {
            let webhook_url = config
                .get("webhook_url")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let id = format!("slack-{webhook_url}");
            Ok(Box::new(RecordingSubscriber {
                id,
                record: Arc::new(CallRecord::default()),
            }) as Box<dyn EventSubscriber>)
        }),
    );

    let sub = create_subscriber(subscriber_config).expect("slack subscriber must be created");
    assert!(sub.subscriber_id().starts_with("slack-"));
    assert_eq!(case["expected"]["subscriber_created"].as_bool(), Some(true));
    assert_eq!(case["expected"]["subscriber_type"].as_str(), Some("slack"));

    reset_subscriber_registry();
}

// ---------------------------------------------------------------------------
// Case 2 — builtin_stdout_type
// ---------------------------------------------------------------------------

#[test]
fn conformance_builtin_stdout_type() {
    let _guard = registry_lock().lock();
    reset_subscriber_registry();

    let fixture = load_fixture();
    let case = fixture_case(&fixture, "builtin_stdout_type");
    let subscriber_config = &case["input"]["subscriber_config"];

    // No registration call — the built-in must be available out of the box.
    let sub = create_subscriber(subscriber_config).expect("stdout built-in must be available");
    assert!(sub.subscriber_id().starts_with("stdout-"));
    assert_eq!(
        case["expected"]["requires_registration"].as_bool(),
        Some(false)
    );
    assert_eq!(case["expected"]["subscriber_type"].as_str(), Some("stdout"));
}

// ---------------------------------------------------------------------------
// Case 3 — builtin_file_type
// ---------------------------------------------------------------------------

#[test]
fn conformance_builtin_file_type() {
    let _guard = registry_lock().lock();
    reset_subscriber_registry();

    let fixture = load_fixture();
    let case = fixture_case(&fixture, "builtin_file_type");
    let subscriber_config = &case["input"]["subscriber_config"];

    let sub = create_subscriber(subscriber_config).expect("file built-in must be available");
    assert!(sub.subscriber_id().starts_with("file-"));
    assert_eq!(
        case["expected"]["requires_registration"].as_bool(),
        Some(false)
    );
    assert_eq!(case["expected"]["subscriber_type"].as_str(), Some("file"));
}

// ---------------------------------------------------------------------------
// Case 4 — builtin_filter_passes_matching
// ---------------------------------------------------------------------------

/// Build a `filter` subscriber from a fixture config but inject a recording
/// delegate so we can observe whether the event reached the inner sink.
/// The fixture references `delegate_type: "webhook"`, but we register a stub
/// under that name during the test to avoid real HTTP traffic.
fn install_recording_webhook(record: Arc<CallRecord>) {
    register_subscriber_type(
        "webhook",
        Box::new(move |_config| {
            let id = format!("webhook-{}", uuid::Uuid::new_v4());
            Ok(Box::new(RecordingSubscriber {
                id,
                record: record.clone(),
            }) as Box<dyn EventSubscriber>)
        }),
    );
}

#[tokio::test]
async fn conformance_builtin_filter_passes_matching() {
    // Lock is held only while we mutate the global registry and create the
    // subscriber instance. Once `sub` owns its (already-built) delegate, the
    // registry can be safely reset and the lock dropped before we await.
    let record = Arc::new(CallRecord::default());
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "builtin_filter_passes_matching");
    let subscriber_config = case["input"]["subscriber_config"].clone();
    let event = parse_event(&case["input"]["event"]);

    let sub = {
        let _guard = registry_lock().lock();
        reset_subscriber_registry();
        install_recording_webhook(record.clone());
        let s = create_subscriber(&subscriber_config).expect("filter built-in must be available");
        reset_subscriber_registry();
        s
    };

    sub.on_event(&event).await.unwrap();

    assert_eq!(case["expected"]["delivery_attempted"].as_bool(), Some(true));
    assert_eq!(case["expected"]["discarded"].as_bool(), Some(false));
    assert_eq!(
        record.count(),
        1,
        "delegate must receive the matching event"
    );
}

// ---------------------------------------------------------------------------
// Case 5 — builtin_filter_discards_nonmatching
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_builtin_filter_discards_nonmatching() {
    let record = Arc::new(CallRecord::default());
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "builtin_filter_discards_nonmatching");
    let subscriber_config = case["input"]["subscriber_config"].clone();
    let event = parse_event(&case["input"]["event"]);

    let sub = {
        let _guard = registry_lock().lock();
        reset_subscriber_registry();
        install_recording_webhook(record.clone());
        let s = create_subscriber(&subscriber_config).expect("filter built-in must be available");
        reset_subscriber_registry();
        s
    };

    sub.on_event(&event).await.unwrap();

    assert_eq!(
        case["expected"]["delivery_attempted"].as_bool(),
        Some(false)
    );
    assert_eq!(case["expected"]["discarded"].as_bool(), Some(true));
    assert_eq!(
        record.count(),
        0,
        "delegate must NOT receive non-matching events"
    );
}

// ---------------------------------------------------------------------------
// Case 6 — circuit_open_after_threshold
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_circuit_open_after_threshold() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "circuit_open_after_threshold");
    let cb_cfg = &case["input"]["circuit_breaker_config"];
    let timeout_ms = cb_cfg["timeout_ms"].as_u64().unwrap();
    let open_threshold: u32 = cb_cfg["open_threshold"]
        .as_u64()
        .unwrap()
        .try_into()
        .unwrap();
    let recovery_window_ms = cb_cfg["recovery_window_ms"].as_u64().unwrap();
    let attempts = case["input"]["failure_sequence"].as_array().unwrap().len();

    let calls = Arc::new(AtomicU32::new(0));
    let sink = Arc::new(CapturingSink::default());

    let wrapper = CircuitBreakerWrapper::new(
        Box::new(AlwaysFail {
            id: "webhook-x".into(),
            calls: calls.clone(),
        }),
        Some(sink.clone()),
    )
    .with_timeout_ms(timeout_ms)
    .with_open_threshold(open_threshold)
    .with_recovery_window_ms(recovery_window_ms)
    .with_subscriber_type_name("webhook");

    let event = ApCoreEvent::new("test.event", json!({}));
    for _ in 0..attempts {
        wrapper.on_event(&event).await.unwrap();
    }

    let expected_state = case["expected"]["circuit_state"].as_str().unwrap();
    let expected_failures: u32 = case["expected"]["consecutive_failures"]
        .as_u64()
        .unwrap()
        .try_into()
        .unwrap();
    let expected_event = case["expected"]["event_emitted"].as_str().unwrap();

    assert_eq!(wrapper.state().as_str(), expected_state);
    assert_eq!(wrapper.consecutive_failures(), expected_failures);
    let captured = sink.captured();
    assert!(
        captured.iter().any(|e| e.event_type == expected_event),
        "expected {expected_event} to be emitted, got: {:?}",
        captured.iter().map(|e| &e.event_type).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Case 7 — circuit_discards_in_open_state
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_circuit_discards_in_open_state() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "circuit_discards_in_open_state");
    let event = parse_event(&case["input"]["event"]);

    let calls = Arc::new(AtomicU32::new(0));
    let sink = Arc::new(CapturingSink::default());
    let wrapper = CircuitBreakerWrapper::new(
        Box::new(AlwaysFail {
            id: "webhook-x".into(),
            calls: calls.clone(),
        }),
        Some(sink.clone()),
    )
    // Long recovery window so the wrapper stays in OPEN for the duration of the test.
    .with_recovery_window_ms(60_000_000);
    wrapper.force_state(CircuitState::Open);
    wrapper.force_last_failure_at(Some(Utc::now()));

    wrapper.on_event(&event).await.unwrap();

    assert_eq!(
        case["expected"]["delivery_attempted"].as_bool(),
        Some(false)
    );
    assert_eq!(
        wrapper.state().as_str(),
        case["expected"]["circuit_state"].as_str().unwrap()
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "delegate.deliver() MUST NOT be called in OPEN state"
    );
}

// ---------------------------------------------------------------------------
// Case 8 — circuit_half_open_after_window
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_circuit_half_open_after_window() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "circuit_half_open_after_window");
    let cb_cfg = &case["input"]["circuit_breaker_config"];
    let recovery_window_ms = cb_cfg["recovery_window_ms"].as_u64().unwrap();

    let last_failure_at: DateTime<Utc> = case["input"]["last_failure_at"]
        .as_str()
        .unwrap()
        .parse()
        .expect("RFC3339 last_failure_at");
    let current_time: DateTime<Utc> = case["input"]["current_time"]
        .as_str()
        .unwrap()
        .parse()
        .expect("RFC3339 current_time");

    let calls = Arc::new(AtomicU32::new(0));
    let frozen_now = current_time;
    let wrapper = CircuitBreakerWrapper::new(
        Box::new(AlwaysOk {
            id: "webhook-x".into(),
            calls: calls.clone(),
        }),
        None,
    )
    .with_recovery_window_ms(recovery_window_ms)
    .with_clock(move || frozen_now);

    wrapper.force_state(CircuitState::Open);
    wrapper.force_last_failure_at(Some(last_failure_at));

    wrapper.check_recovery();

    let expected = case["expected"]["circuit_state"].as_str().unwrap();
    assert_eq!(wrapper.state().as_str(), expected);
}

// ---------------------------------------------------------------------------
// Case 9 — circuit_closes_on_success
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_circuit_closes_on_success() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "circuit_closes_on_success");

    let calls = Arc::new(AtomicU32::new(0));
    let sink = Arc::new(CapturingSink::default());
    let wrapper = CircuitBreakerWrapper::new(
        Box::new(AlwaysOk {
            id: "webhook-x".into(),
            calls: calls.clone(),
        }),
        Some(sink.clone()),
    )
    .with_subscriber_type_name("webhook");
    wrapper.force_state(CircuitState::HalfOpen);
    wrapper.force_consecutive_failures(7);

    let event = ApCoreEvent::new("test.event", json!({}));
    wrapper.on_event(&event).await.unwrap();

    let expected_state = case["expected"]["circuit_state"].as_str().unwrap();
    let expected_failures: u32 = case["expected"]["consecutive_failures"]
        .as_u64()
        .unwrap()
        .try_into()
        .unwrap();
    let expected_event = case["expected"]["event_emitted"].as_str().unwrap();

    assert_eq!(wrapper.state().as_str(), expected_state);
    assert_eq!(wrapper.consecutive_failures(), expected_failures);
    let captured = sink.captured();
    assert!(
        captured.iter().any(|e| e.event_type == expected_event),
        "expected {expected_event} to be emitted, got: {:?}",
        captured.iter().map(|e| &e.event_type).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Case 10 — event_naming_canonical
// ---------------------------------------------------------------------------

#[test]
fn conformance_event_naming_canonical() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "event_naming_canonical");

    let pattern = case["expected"]["pattern"].as_str().unwrap();
    let re = Regex::new(pattern).expect("fixture pattern must be a valid regex");

    let events: Vec<&str> = case["input"]["events_to_check"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();

    for event in &events {
        assert!(
            re.is_match(event),
            "event name '{event}' does not match canonical pattern {pattern}"
        );
    }
    assert_eq!(case["expected"]["all_match_pattern"].as_bool(), Some(true));
}
