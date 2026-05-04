// Tests for Issue #36 — canonical event-name standardization, and
// Issue #45.2 — contextual auditing (caller_id auto-extraction).
//
// These tests assert:
//   1. The four legacy events (module_registered, module_unregistered,
//      error_threshold_exceeded, latency_threshold_exceeded) emit BOTH the
//      legacy name AND the canonical apcore.<subsystem>.<event> name.
//   2. The canonical names match the apcore.registry.* and apcore.health.*
//      glob subscription patterns.
//   3. Registry events are emitted as ApCoreEvent (not just tracing logs)
//      so subscribers can pattern-match.
//   4. update_config audit events include caller_id from the Context.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::Mutex as TokioMutex;

use apcore::context::{Context, Identity};
use apcore::events::emitter::{ApCoreEvent, EventEmitter};
use apcore::events::subscribers::EventSubscriber;
use apcore::middleware::{Middleware, PlatformNotifyMiddleware};
use apcore::module::Module;
use apcore::observability::metrics::MetricsCollector;
use apcore::sys_modules::audit::{AuditStore, InMemoryAuditStore};
use apcore::sys_modules::UpdateConfigModule;

#[derive(Debug, Default)]
struct RecordingSub {
    pattern: String,
    received: Arc<Mutex<Vec<ApCoreEvent>>>,
}

impl RecordingSub {
    fn new(pattern: &str) -> (Box<Self>, Arc<Mutex<Vec<ApCoreEvent>>>) {
        let received = Arc::new(Mutex::new(Vec::new()));
        let sub = Box::new(Self {
            pattern: pattern.to_string(),
            received: received.clone(),
        });
        (sub, received)
    }
}

#[async_trait]
impl EventSubscriber for RecordingSub {
    fn subscriber_id(&self) -> &'static str {
        "recording"
    }
    fn event_pattern(&self) -> &str {
        &self.pattern
    }
    async fn on_event(&self, event: &ApCoreEvent) -> Result<(), apcore::errors::ModuleError> {
        self.received.lock().push(event.clone());
        Ok(())
    }
}

struct DummyModule;
#[async_trait]
impl Module for DummyModule {
    fn description(&self) -> &'static str {
        "dummy"
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }
    async fn execute(
        &self,
        _inputs: Value,
        _ctx: &Context<Value>,
    ) -> Result<Value, apcore::errors::ModuleError> {
        Ok(json!({}))
    }
}

fn build_ctx_with_caller(caller_id: Option<String>, identity_id: Option<&str>) -> Context<Value> {
    let identity = identity_id
        .map(|id| Identity::new(id.to_string(), "user".to_string(), vec![], HashMap::new()));
    Context {
        trace_id: "trace-test".to_string(),
        identity,
        services: Value::Null,
        caller_id,
        data: Arc::new(parking_lot::RwLock::new(HashMap::new())),
        call_chain: vec![],
        redacted_inputs: None,
        redacted_output: None,
        cancel_token: None,
        global_deadline: None,
        executor: None,
    }
}

// ---------------------------------------------------------------------------
// Part A — Health threshold events (middleware)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_threshold_emits_canonical_and_legacy_events() {
    // Pre-populate metrics so error rate > 0.5
    let metrics = MetricsCollector::new();
    let mut labels = HashMap::new();
    labels.insert("module".to_string(), "mod.a".to_string());
    labels.insert("status".to_string(), "error".to_string());
    metrics.increment("apcore_module_calls_total", labels.clone(), 10.0);

    let mut emitter = EventEmitter::new();
    let (sub, received) = RecordingSub::new("apcore.health.*");
    let (legacy_sub, legacy_received) = RecordingSub::new("error_threshold_exceeded");
    emitter.subscribe(sub);
    emitter.subscribe(legacy_sub);

    let pn = PlatformNotifyMiddleware::new(emitter, Some(metrics), 0.1, 5000.0);

    let ctx = build_ctx_with_caller(None, None);
    let _ = pn
        .on_error(
            "mod.a",
            json!({}),
            &apcore::errors::ModuleError::new(
                apcore::errors::ErrorCode::GeneralInternalError,
                "boom",
            ),
            &ctx,
        )
        .await;

    let canonical_count = received
        .lock()
        .iter()
        .filter(|e| e.event_type == "apcore.health.error_threshold_exceeded")
        .count();
    let legacy_count = legacy_received
        .lock()
        .iter()
        .filter(|e| e.event_type == "error_threshold_exceeded")
        .count();
    assert_eq!(
        canonical_count, 1,
        "expected canonical apcore.health.error_threshold_exceeded event"
    );
    assert_eq!(
        legacy_count, 1,
        "expected legacy error_threshold_exceeded event for backward compat"
    );

    // Legacy event payload must signal deprecation.
    let legacy_evt = legacy_received
        .lock()
        .iter()
        .find(|e| e.event_type == "error_threshold_exceeded")
        .cloned()
        .unwrap();
    assert_eq!(
        legacy_evt.data.get("deprecated"),
        Some(&json!(true)),
        "legacy event must include deprecated:true marker"
    );
}

#[tokio::test]
async fn latency_threshold_emits_canonical_and_legacy_events() {
    let metrics = MetricsCollector::new();
    let mut labels = HashMap::new();
    labels.insert("module_id".to_string(), "mod.b".to_string());
    // Push a high-latency observation.
    metrics.observe("apcore_module_duration_seconds", labels, 10.0);

    let mut emitter = EventEmitter::new();
    let (sub, received) = RecordingSub::new("apcore.health.*");
    let (legacy_sub, legacy_received) = RecordingSub::new("latency_threshold_exceeded");
    emitter.subscribe(sub);
    emitter.subscribe(legacy_sub);

    let pn = PlatformNotifyMiddleware::new(emitter, Some(metrics), 0.1, 1000.0);

    let ctx = build_ctx_with_caller(None, None);
    let _ = pn.after("mod.b", json!({}), json!({}), &ctx).await;

    let canonical = received
        .lock()
        .iter()
        .filter(|e| e.event_type == "apcore.health.latency_threshold_exceeded")
        .count();
    let legacy = legacy_received
        .lock()
        .iter()
        .filter(|e| e.event_type == "latency_threshold_exceeded")
        .count();
    assert_eq!(canonical, 1, "expected canonical latency event");
    assert_eq!(
        legacy, 1,
        "expected legacy latency event for backward compat"
    );
}

// ---------------------------------------------------------------------------
// Part A — Registry events as ApCoreEvent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn registry_register_emits_apcore_event_canonical_and_legacy() {
    use apcore::config::Config;
    use apcore::executor::Executor;
    use apcore::registry::registry::Registry;
    use apcore::sys_modules::{register_sys_modules_with_options, SysModulesOptions};

    let registry = Arc::new(Registry::new());
    let mut config = Config::default();
    config.set("sys_modules.enabled", json!(true));
    config.set("sys_modules.events.enabled", json!(true));
    let executor = Executor::new(Arc::clone(&registry), Config::default());

    let ctx_result = register_sys_modules_with_options(
        Arc::clone(&registry),
        &executor,
        &config,
        None,
        SysModulesOptions::default(),
    )
    .expect("register_sys_modules");

    // Subscribe to both canonical glob and legacy literal name.
    let (canonical_sub, canonical_received) = RecordingSub::new("apcore.registry.*");
    let (legacy_sub, legacy_received) = RecordingSub::new("module_registered");
    {
        let mut emitter = ctx_result.emitter.lock().await;
        emitter.subscribe(canonical_sub);
        emitter.subscribe(legacy_sub);
    }

    // Register a fresh user module that wasn't registered before subscriber attach.
    registry
        .register_module("user.dummy", Box::new(DummyModule))
        .expect("register user module");

    // Allow async dispatch to drain (callbacks run on tokio::spawn).
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let canonical = canonical_received
        .lock()
        .iter()
        .filter(|e| e.event_type == "apcore.registry.module_registered")
        .count();
    let legacy = legacy_received
        .lock()
        .iter()
        .filter(|e| e.event_type == "module_registered")
        .count();
    assert!(
        canonical >= 1,
        "expected at least one canonical apcore.registry.module_registered event"
    );
    assert!(
        legacy >= 1,
        "expected at least one legacy module_registered event"
    );
}

#[tokio::test]
async fn registry_unregister_emits_apcore_event_canonical_and_legacy() {
    use apcore::config::Config;
    use apcore::executor::Executor;
    use apcore::registry::registry::Registry;
    use apcore::sys_modules::{register_sys_modules_with_options, SysModulesOptions};

    let registry = Arc::new(Registry::new());
    let mut config = Config::default();
    config.set("sys_modules.enabled", json!(true));
    config.set("sys_modules.events.enabled", json!(true));
    let executor = Executor::new(Arc::clone(&registry), Config::default());

    let ctx_result = register_sys_modules_with_options(
        Arc::clone(&registry),
        &executor,
        &config,
        None,
        SysModulesOptions::default(),
    )
    .expect("register_sys_modules");

    let (canonical_sub, canonical_received) = RecordingSub::new("apcore.registry.*");
    let (legacy_sub, legacy_received) = RecordingSub::new("module_unregistered");
    {
        let mut emitter = ctx_result.emitter.lock().await;
        emitter.subscribe(canonical_sub);
        emitter.subscribe(legacy_sub);
    }

    registry
        .register_module("user.bye", Box::new(DummyModule))
        .expect("register user module");
    let _ = registry.unregister("user.bye");

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let canonical = canonical_received
        .lock()
        .iter()
        .filter(|e| e.event_type == "apcore.registry.module_unregistered")
        .count();
    let legacy = legacy_received
        .lock()
        .iter()
        .filter(|e| e.event_type == "module_unregistered")
        .count();
    assert!(
        canonical >= 1,
        "expected canonical apcore.registry.module_unregistered event"
    );
    assert!(legacy >= 1, "expected legacy module_unregistered event");
}

// ---------------------------------------------------------------------------
// Part B — Contextual auditing (caller_id auto-extraction)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_config_audit_event_includes_caller_id_from_context() {
    use apcore::config::Config;

    let config = Arc::new(TokioMutex::new(Config::default()));
    let emitter = Arc::new(TokioMutex::new(EventEmitter::new()));
    let store: Arc<dyn AuditStore> = Arc::new(InMemoryAuditStore::new());
    let store_clone_for_assert = Arc::clone(&store);

    let module = UpdateConfigModule::new(Arc::clone(&config), Arc::clone(&emitter))
        .with_audit_store(Some(Arc::clone(&store)));

    // Subscribe so we can also assert the emitted event payload.
    let (sub, received) = RecordingSub::new("apcore.config.updated");
    {
        let mut em = emitter.lock().await;
        em.subscribe(sub);
    }

    let ctx = build_ctx_with_caller(Some("api.gateway".to_string()), Some("user-123"));

    let inputs = json!({
        "key": "feature.flag",
        "value": true,
        "reason": "test",
    });
    module.execute(inputs, &ctx).await.expect("update_config");

    // The emitted event payload MUST carry caller_id from the Context.
    let evts = received.lock().clone();
    assert!(!evts.is_empty(), "expected apcore.config.updated event");
    let evt = &evts[0];
    assert_eq!(
        evt.data.get("caller_id"),
        Some(&json!("api.gateway")),
        "audit event must include caller_id from context"
    );
    assert_eq!(
        evt.data.get("actor_id"),
        Some(&json!("user-123")),
        "audit event must include actor_id from identity"
    );

    // Even when caller_id is absent, default to "@external".
    let store_for_default = Arc::clone(&store_clone_for_assert);
    let module2 = UpdateConfigModule::new(Arc::clone(&config), Arc::clone(&emitter))
        .with_audit_store(Some(store_for_default));

    let ctx_anon = build_ctx_with_caller(None, None);
    let inputs2 = json!({
        "key": "another.flag",
        "value": false,
        "reason": "test2",
    });
    module2
        .execute(inputs2, &ctx_anon)
        .await
        .expect("update_config2");

    let later_events = received.lock().clone();
    let anon_event = later_events
        .iter()
        .find(|e| e.data.get("key").and_then(|v| v.as_str()) == Some("another.flag"))
        .expect("second event present");
    assert_eq!(
        anon_event.data.get("caller_id"),
        Some(&json!("@external")),
        "default caller_id should be @external when context has none"
    );
}
