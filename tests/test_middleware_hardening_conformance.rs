//! Cross-language conformance tests for Middleware Architecture Hardening
//! (Issue #42).
//!
//! Fixture source: apcore/conformance/fixtures/middleware_hardening.json
//! Spec reference: apcore/docs/features/middleware-system.md
//! (## Middleware Architecture Hardening)
//!
//! Each fixture case verifies one normative rule:
//! - context namespace partitioning (`_apcore.*` vs `ext.*`),
//! - the `CircuitBreakerMiddleware` state machine and event emission,
//! - the `TracingMiddleware` span lifecycle and no-op fallback,
//! - the language-specific async-handler detection rule (Rust: static).

#![allow(clippy::missing_panics_doc)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;

use apcore::context::{Context, Identity};
use apcore::errors::{ErrorCode, ModuleError};
use apcore::events::emitter::{ApCoreEvent, EventEmitter};
use apcore::events::subscribers::EventSubscriber;
use apcore::middleware::circuit_breaker::{CircuitBreakerMiddleware, CircuitBreakerState};
use apcore::middleware::context_namespace::{validate_context_key, ContextWriter};
use apcore::middleware::otel_tracing::{
    TracingMiddleware, TRACING_ATTRIBUTES_KEY, TRACING_SPAN_NAME_KEY,
};
use apcore::middleware::{Middleware, TRACING_SPAN_STATUS_KEY};

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
    let path = find_fixtures_root().join("middleware_hardening.json");
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

fn make_ctx(caller: &str) -> Context<Value> {
    let identity = Identity::new(
        "test".to_string(),
        "user".to_string(),
        vec![],
        std::collections::HashMap::new(),
    );
    let mut ctx: Context<Value> = Context::new(identity);
    ctx.caller_id = Some(caller.to_string());
    ctx
}

// Recording subscriber that captures every event by name. Used to assert
// `apcore.circuit.opened` / `apcore.circuit.closed` emission.
#[derive(Debug, Default)]
struct RecordingSubscriber {
    events: Mutex<Vec<ApCoreEvent>>,
}

impl RecordingSubscriber {
    fn captured(&self) -> Vec<ApCoreEvent> {
        self.events.lock().clone()
    }
}

#[derive(Debug)]
struct RecordingHandle {
    inner: Arc<RecordingSubscriber>,
    id: String,
}

#[async_trait]
impl EventSubscriber for RecordingHandle {
    fn subscriber_id(&self) -> &str {
        &self.id
    }
    #[allow(clippy::unnecessary_literal_bound)]
    fn event_pattern(&self) -> &str {
        "*"
    }
    async fn on_event(&self, event: &ApCoreEvent) -> Result<(), ModuleError> {
        self.inner.events.lock().push(event.clone());
        Ok(())
    }
}

fn build_emitter_with_recorder() -> (Arc<EventEmitter>, Arc<RecordingSubscriber>) {
    let recorder = Arc::new(RecordingSubscriber::default());
    let mut emitter = EventEmitter::new();
    emitter.subscribe(Box::new(RecordingHandle {
        inner: Arc::clone(&recorder),
        id: "recorder".to_string(),
    }));
    (Arc::new(emitter), recorder)
}

// ---------------------------------------------------------------------------
// Case 1 — context_namespace_apcore_prefix
// ---------------------------------------------------------------------------

#[test]
fn conformance_context_namespace_apcore_prefix() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "context_namespace_apcore_prefix");
    let writer = case["input"]["writer"].as_str().unwrap();
    let key = case["input"]["key"].as_str().unwrap();
    assert_eq!(writer, "framework");

    let check = validate_context_key(ContextWriter::Framework, key);
    assert!(check.valid);
    assert!(!check.warning);

    assert_eq!(case["expected"]["valid"].as_bool(), Some(true));
    assert_eq!(case["expected"]["warning"].as_bool(), Some(false));
}

// ---------------------------------------------------------------------------
// Case 2 — context_namespace_ext_prefix
// ---------------------------------------------------------------------------

#[test]
fn conformance_context_namespace_ext_prefix() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "context_namespace_ext_prefix");
    let writer = case["input"]["writer"].as_str().unwrap();
    let key = case["input"]["key"].as_str().unwrap();
    assert_eq!(writer, "user");

    let check = validate_context_key(ContextWriter::User, key);
    assert!(check.valid);
    assert!(!check.warning);

    assert_eq!(case["expected"]["valid"].as_bool(), Some(true));
    assert_eq!(case["expected"]["warning"].as_bool(), Some(false));
}

// ---------------------------------------------------------------------------
// Case 3 — context_namespace_violation
// ---------------------------------------------------------------------------

#[test]
fn conformance_context_namespace_violation() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "context_namespace_violation");
    let writer = case["input"]["writer"].as_str().unwrap();
    let key = case["input"]["key"].as_str().unwrap();
    assert_eq!(writer, "user");

    let check = validate_context_key(ContextWriter::User, key);
    assert!(!check.valid);
    assert!(check.warning);

    assert_eq!(case["expected"]["valid"].as_bool(), Some(false));
    assert_eq!(case["expected"]["warning"].as_bool(), Some(true));
}

// ---------------------------------------------------------------------------
// Case 4 — circuit_breaker_opens_at_threshold
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_circuit_breaker_opens_at_threshold() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "circuit_breaker_opens_at_threshold");
    let module_id = case["input"]["module_id"].as_str().unwrap();
    let caller_id = case["input"]["caller_id"].as_str().unwrap();
    let open_threshold = case["input"]["open_threshold"].as_f64().unwrap();
    let window_size = usize::try_from(case["input"]["window_size"].as_u64().unwrap()).unwrap();
    let errors_in_window = case["input"]["errors_in_window"].as_u64().unwrap();
    let successes_in_window = case["input"]["successes_in_window"].as_u64().unwrap();

    let (emitter, recorder) = build_emitter_with_recorder();
    let mw = CircuitBreakerMiddleware::builder()
        .open_threshold(open_threshold)
        .window_size(window_size)
        .min_samples(1)
        .emitter(Arc::clone(&emitter))
        .build();

    let mut ctx = make_ctx(caller_id);
    // Some calls will be rejected once the circuit opens; force caller bucket.
    ctx.caller_id = Some(caller_id.to_string());

    let err = ModuleError::new(ErrorCode::ModuleExecuteError, "fixture-error");

    // Interleave successes and errors so the rolling window contains exactly
    // `errors_in_window` errors and `successes_in_window` successes.
    for _ in 0..successes_in_window {
        mw.after(module_id, Value::Null, Value::Null, &ctx)
            .await
            .unwrap();
    }
    for _ in 0..errors_in_window {
        mw.on_error(module_id, Value::Null, &err, &ctx)
            .await
            .unwrap();
    }

    assert_eq!(
        mw.state(module_id, caller_id),
        CircuitBreakerState::Open,
        "expected circuit OPEN after {errors_in_window} errors / {successes_in_window} successes"
    );
    assert_eq!(case["expected"]["circuit_state"].as_str(), Some("OPEN"),);

    let captured = recorder.captured();
    assert!(
        captured
            .iter()
            .any(|e| e.event_type == "apcore.circuit.opened"),
        "expected apcore.circuit.opened to be emitted; got {:?}",
        captured.iter().map(|e| &e.event_type).collect::<Vec<_>>(),
    );
    assert_eq!(
        case["expected"]["event_emitted"].as_str(),
        Some("apcore.circuit.opened"),
    );
}

// ---------------------------------------------------------------------------
// Case 5 — circuit_breaker_short_circuits_open
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_circuit_breaker_short_circuits_open() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "circuit_breaker_short_circuits_open");
    let module_id = case["input"]["module_id"].as_str().unwrap();
    let caller_id = case["input"]["caller_id"].as_str().unwrap();
    let recovery_window_ms = case["input"]["recovery_window_ms"].as_u64().unwrap();
    let ms_since_opened = case["input"]["ms_since_opened"].as_u64().unwrap();

    let mw = CircuitBreakerMiddleware::builder()
        .recovery_window_ms(recovery_window_ms)
        .build();

    // Force OPEN with opened_at set so the recovery window has NOT elapsed.
    let opened_at = Instant::now()
        .checked_sub(Duration::from_millis(ms_since_opened))
        .expect("test clock far enough from epoch");
    mw.force_state(
        module_id,
        caller_id,
        CircuitBreakerState::Open,
        Some(opened_at),
    );

    let mut ctx = make_ctx(caller_id);
    ctx.caller_id = Some(caller_id.to_string());

    let result = mw.before(module_id, Value::Null, &ctx).await;
    assert!(
        result.is_err(),
        "before() must short-circuit while OPEN within the recovery window"
    );
    let err = result.unwrap_err();
    assert_eq!(err.code, ErrorCode::CircuitBreakerOpen);

    assert_eq!(
        case["expected"]["error"].as_str(),
        Some("CircuitBreakerOpenError"),
    );
    assert_eq!(case["expected"]["module_reached"].as_bool(), Some(false));
}

// ---------------------------------------------------------------------------
// Case 6 — circuit_breaker_half_open_probe
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_circuit_breaker_half_open_probe() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "circuit_breaker_half_open_probe");
    let module_id = case["input"]["module_id"].as_str().unwrap();
    let caller_id = case["input"]["caller_id"].as_str().unwrap();
    let recovery_window_ms = case["input"]["recovery_window_ms"].as_u64().unwrap();
    let ms_since_opened = case["input"]["ms_since_opened"].as_u64().unwrap();
    assert!(
        ms_since_opened > recovery_window_ms,
        "fixture invariant: ms_since_opened > recovery_window_ms"
    );

    let mw = CircuitBreakerMiddleware::builder()
        .recovery_window_ms(recovery_window_ms)
        .build();

    let opened_at = Instant::now()
        .checked_sub(Duration::from_millis(ms_since_opened))
        .expect("test clock far enough from epoch");
    mw.force_state(
        module_id,
        caller_id,
        CircuitBreakerState::Open,
        Some(opened_at),
    );

    let mut ctx = make_ctx(caller_id);
    ctx.caller_id = Some(caller_id.to_string());

    // First call after recovery window → transitions to HALF_OPEN, allowed.
    let probe = mw.before(module_id, Value::Null, &ctx).await;
    assert!(probe.is_ok(), "first probe call must be allowed");
    assert_eq!(
        mw.state(module_id, caller_id),
        CircuitBreakerState::HalfOpen,
    );
    assert_eq!(
        case["expected"]["circuit_state"].as_str(),
        Some("HALF_OPEN"),
    );
    assert_eq!(case["expected"]["probe_call_allowed"].as_bool(), Some(true));

    // Second concurrent probe must be rejected (max_concurrent_probes = 1).
    let second = mw.before(module_id, Value::Null, &ctx).await;
    assert!(second.is_err());
    assert_eq!(second.unwrap_err().code, ErrorCode::CircuitBreakerOpen,);
    assert_eq!(case["expected"]["max_concurrent_probes"].as_u64(), Some(1),);
}

// ---------------------------------------------------------------------------
// Case 7 — circuit_breaker_closes_on_success
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_circuit_breaker_closes_on_success() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "circuit_breaker_closes_on_success");
    let module_id = case["input"]["module_id"].as_str().unwrap();
    let caller_id = case["input"]["caller_id"].as_str().unwrap();

    let (emitter, recorder) = build_emitter_with_recorder();
    let mw = CircuitBreakerMiddleware::builder()
        .emitter(Arc::clone(&emitter))
        .build();

    mw.force_state(
        module_id,
        caller_id,
        CircuitBreakerState::HalfOpen,
        Some(Instant::now()),
    );

    let mut ctx = make_ctx(caller_id);
    ctx.caller_id = Some(caller_id.to_string());

    // Take the probe slot, then complete successfully.
    mw.before(module_id, Value::Null, &ctx).await.unwrap();
    mw.after(module_id, Value::Null, Value::Null, &ctx)
        .await
        .unwrap();

    assert_eq!(mw.state(module_id, caller_id), CircuitBreakerState::Closed,);
    assert_eq!(case["expected"]["circuit_state"].as_str(), Some("CLOSED"),);

    let captured = recorder.captured();
    assert!(
        captured
            .iter()
            .any(|e| e.event_type == "apcore.circuit.closed"),
        "expected apcore.circuit.closed to be emitted; got {:?}",
        captured.iter().map(|e| &e.event_type).collect::<Vec<_>>(),
    );
    assert_eq!(
        case["expected"]["event_emitted"].as_str(),
        Some("apcore.circuit.closed"),
    );
}

// ---------------------------------------------------------------------------
// Case 8 — tracing_span_created
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_tracing_span_created() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "tracing_span_created");
    let module_id = case["input"]["module_id"].as_str().unwrap();
    let caller_id = case["input"]["caller_id"].as_str().unwrap();
    let trace_id_in = case["input"]["trace_id"].as_str().unwrap();
    assert_eq!(case["input"]["otel_available"].as_bool(), Some(true));

    // The runtime `enabled(true)` override mirrors "OpenTelemetry SDK is
    // available" for this language. Real OTel exporter wiring is layered
    // above this scaffold via the `opentelemetry` cargo feature.
    let mw = TracingMiddleware::builder().enabled(true).build();

    let mut ctx = make_ctx(caller_id);
    ctx.trace_id = trace_id_in.to_string();
    ctx.caller_id = Some(caller_id.to_string());

    mw.before(module_id, Value::Null, &ctx).await.unwrap();

    let data = ctx.data.read();
    let expected_key = case["expected"]["context_key"].as_str().unwrap();
    let span_id = data
        .get(expected_key)
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("expected key '{expected_key}' to be present in context.data"));
    assert!(!span_id.is_empty(), "span_id must be non-empty");
    assert_eq!(
        case["expected"]["span_id_stored_in_context"].as_bool(),
        Some(true),
    );

    let span_name = data
        .get(TRACING_SPAN_NAME_KEY)
        .and_then(|v| v.as_str())
        .expect("span name must be recorded");
    assert_eq!(span_name, module_id);
    assert_eq!(case["expected"]["span_name"].as_str(), Some(module_id));

    let attributes = data
        .get(TRACING_ATTRIBUTES_KEY)
        .and_then(|v| v.as_object())
        .expect("attributes must be recorded");
    let expected_attrs = case["expected"]["span_attributes"].as_object().unwrap();
    for (k, v) in expected_attrs {
        let actual = attributes
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("attribute '{k}' missing from span"));
        assert_eq!(actual, v.as_str().unwrap(), "attribute '{k}' mismatch");
    }
    assert_eq!(case["expected"]["span_created"].as_bool(), Some(true));
}

// ---------------------------------------------------------------------------
// Case 9 — tracing_noop_without_otel
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conformance_tracing_noop_without_otel() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "tracing_noop_without_otel");
    let module_id = case["input"]["module_id"].as_str().unwrap();
    let caller_id = case["input"]["caller_id"].as_str().unwrap();
    assert_eq!(case["input"]["otel_available"].as_bool(), Some(false));

    let mw = TracingMiddleware::builder().enabled(false).build();

    let mut ctx = make_ctx(caller_id);
    ctx.caller_id = Some(caller_id.to_string());

    let before = mw.before(module_id, Value::Null, &ctx).await;
    let after = mw.after(module_id, Value::Null, Value::Null, &ctx).await;
    let on_err = mw
        .on_error(
            module_id,
            Value::Null,
            &ModuleError::new(ErrorCode::ModuleExecuteError, "x"),
            &ctx,
        )
        .await;

    assert!(before.is_ok());
    assert!(after.is_ok());
    assert!(on_err.is_ok());
    assert_eq!(case["expected"]["error_raised"].as_bool(), Some(false));

    let data = ctx.data.read();
    assert!(
        data.get(apcore::middleware::namespace_keys::TRACING_SPAN_ID)
            .is_none(),
        "span_id must NOT be written when otel is unavailable"
    );
    assert!(data.get(TRACING_SPAN_STATUS_KEY).is_none());
    assert_eq!(case["expected"]["span_created"].as_bool(), Some(false));
    assert_eq!(
        case["expected"]["execution_continues"].as_bool(),
        Some(true),
    );
}

// ---------------------------------------------------------------------------
// Case 10 — async_detection_coroutine_function
//
// Rust enforces this rule statically — async fns differ from sync fns at the
// type level (they return `impl Future`). There is no runtime detection
// path to test, so we assert the fixture's premises are language-correct
// and let the static type system carry the conformance guarantee.
// ---------------------------------------------------------------------------

#[test]
fn conformance_async_detection_coroutine_function() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "async_detection_coroutine_function");
    // The case carries Python detection guidance; Rust satisfies this rule
    // structurally via `async_trait` and the `Future` trait. The fixture's
    // `is_async` field describes the expected truth value when the language
    // does perform detection — for Rust, the equivalent compile-time fact
    // is that any handler reaching the middleware pipeline is already typed
    // as async (see `Middleware::before/after/on_error` signatures).
    assert_eq!(case["expected"]["is_async"].as_bool(), Some(true));
    assert_eq!(
        case["expected"]["incorrect_method_result"]["result_on_uncalled_function"].as_bool(),
        Some(false),
    );

    // Compile-time witness: the trait methods are async. If this file
    // compiles, the rule is satisfied.
    assert_async_signatures::<TracingMiddleware>();
    assert_async_signatures::<CircuitBreakerMiddleware>();
}

fn assert_async_signatures<M: Middleware>() {}
