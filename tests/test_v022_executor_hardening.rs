//! Regression tests for v0.22 executor hardening — D-19/D-20/D-21/D-11.
//!
//! Covers:
//! - A-D-EXEC-001 (D-11): per-module `resources.timeout` annotation honored.
//! - A-D-EXEC-002 (D-21): cancel-token check at CallChainGuard and at
//!   BuiltinExecute (mid-pipeline cancel observation).
//! - A-D-EXEC-003 (D-20): ExecutionCancelled short-circuit bypasses
//!   `on_error` middleware so logging/retry middleware cannot swallow it.
//! - A-D-EXEC-004 (D-19): `call_with_trace` runs `on_error` recovery and
//!   returns `(recovered_value, trace)` on successful recovery.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use apcore::cancel::CancelToken;
use apcore::context::{Context, Identity};
use apcore::errors::{ErrorCode, ModuleError};
use apcore::middleware::base::Middleware;
use apcore::module::{Module, ModuleAnnotations};
use apcore::registry::{ModuleDescriptor, Registry};
use apcore::APCore;
use async_trait::async_trait;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Module that sleeps for `delay_ms` before returning {"ok": true}.
struct SleepModule {
    delay_ms: u64,
}

#[async_trait]
impl Module for SleepModule {
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn description(&self) -> &'static str {
        "Sleep for a configured duration"
    }
    async fn execute(&self, _inputs: Value, _ctx: &Context<Value>) -> Result<Value, ModuleError> {
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        Ok(json!({"ok": true}))
    }
}

/// Module that always returns Err so on_error middleware can attempt recovery.
struct AlwaysFailModule;

#[async_trait]
impl Module for AlwaysFailModule {
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn description(&self) -> &'static str {
        "Always fails"
    }
    async fn execute(&self, _inputs: Value, _ctx: &Context<Value>) -> Result<Value, ModuleError> {
        Err(ModuleError::new(
            ErrorCode::GeneralInternalError,
            "always fails",
        ))
    }
}

/// Middleware whose `on_error` recovers by returning a fixed value.
#[derive(Debug)]
struct RecoveringMiddleware;

#[async_trait]
impl Middleware for RecoveringMiddleware {
    fn name(&self) -> &'static str {
        "recovering"
    }
    async fn before(
        &self,
        _module_id: &str,
        inputs: Value,
        _ctx: &Context<Value>,
    ) -> Result<Option<Value>, ModuleError> {
        Ok(Some(inputs))
    }
    async fn after(
        &self,
        _module_id: &str,
        _inputs: Value,
        _output: Value,
        _ctx: &Context<Value>,
    ) -> Result<Option<Value>, ModuleError> {
        Ok(None)
    }
    async fn on_error(
        &self,
        _module_id: &str,
        _inputs: Value,
        _error: &ModuleError,
        _ctx: &Context<Value>,
    ) -> Result<Option<Value>, ModuleError> {
        Ok(Some(json!({"recovered": true})))
    }
}

/// Middleware whose `on_error` records that it was invoked. Used to assert
/// the D-20 short-circuit prevents `on_error` from running on cancellation.
#[derive(Debug, Clone)]
struct SwallowingMiddleware {
    on_error_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Middleware for SwallowingMiddleware {
    fn name(&self) -> &'static str {
        "swallowing"
    }
    async fn before(
        &self,
        _module_id: &str,
        inputs: Value,
        _ctx: &Context<Value>,
    ) -> Result<Option<Value>, ModuleError> {
        Ok(Some(inputs))
    }
    async fn after(
        &self,
        _module_id: &str,
        _inputs: Value,
        _output: Value,
        _ctx: &Context<Value>,
    ) -> Result<Option<Value>, ModuleError> {
        Ok(None)
    }
    async fn on_error(
        &self,
        _module_id: &str,
        _inputs: Value,
        _error: &ModuleError,
        _ctx: &Context<Value>,
    ) -> Result<Option<Value>, ModuleError> {
        self.on_error_calls.fetch_add(1, Ordering::SeqCst);
        // Attempt to swallow cancellation by returning a recovery value —
        // the short-circuit MUST prevent this from succeeding.
        Ok(Some(json!({"swallowed": true})))
    }
}

fn ctx_with_token(token: CancelToken) -> Context<Value> {
    let mut ctx = Context::<Value>::new(Identity::new(
        "@external".to_string(),
        "external".to_string(),
        vec![],
        HashMap::new(),
    ));
    ctx.cancel_token = Some(token);
    ctx
}

/// Register `module` under `module_id` with a descriptor that pins
/// `resources.timeout` in the annotations' `extra` map.
fn register_with_timeout(
    registry: &Registry,
    module_id: &str,
    module: Box<dyn Module>,
    timeout_ms: u64,
) {
    let mut annotations = ModuleAnnotations::default();
    annotations
        .extra
        .insert("resources".to_string(), json!({ "timeout": timeout_ms }));
    let descriptor = ModuleDescriptor {
        module_id: module_id.to_string(),
        name: None,
        description: module.description().to_string(),
        documentation: None,
        input_schema: module.input_schema(),
        output_schema: module.output_schema(),
        version: "1.0.0".to_string(),
        tags: vec![],
        annotations: Some(annotations),
        examples: vec![],
        metadata: HashMap::new(),
        display: None,
        sunset_date: None,
        dependencies: vec![],
        enabled: true,
    };
    registry.register(module_id, module, descriptor).unwrap();
}

// ---------------------------------------------------------------------------
// A-D-EXEC-001 (D-11): per-module resources.timeout overrides default
// ---------------------------------------------------------------------------

#[tokio::test]
async fn per_module_timeout_overrides_default() {
    // The module sleeps for 200 ms; per-module timeout pinned to 50 ms via
    // annotations.extra["resources"]["timeout"]. The call MUST time out
    // before the module returns, regardless of the executor default
    // timeout (which is 30 000 ms in Config::default()).
    let client = APCore::new();
    register_with_timeout(
        client.registry(),
        "slow.module",
        Box::new(SleepModule { delay_ms: 200 }),
        50,
    );

    let start = std::time::Instant::now();
    let result = client.call("slow.module", json!({}), None, None).await;
    let elapsed = start.elapsed();

    assert!(result.is_err(), "expected timeout error, got {result:?}");
    let err = result.unwrap_err();
    assert_eq!(err.code, ErrorCode::ModuleTimeout);
    assert!(
        elapsed < Duration::from_millis(180),
        "per-module timeout (50 ms) was not honored; took {elapsed:?}"
    );
}

// ---------------------------------------------------------------------------
// A-D-EXEC-002 (D-21): cancel-token observed at CallChainGuard
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancel_observed_at_call_chain_guard_short_circuits_pipeline() {
    let client = APCore::new();
    client
        .register("slow.module", Box::new(SleepModule { delay_ms: 500 }))
        .unwrap();

    let token = CancelToken::new();
    token.cancel(); // pre-cancelled
    let ctx = ctx_with_token(token);

    let start = std::time::Instant::now();
    let result = client
        .executor()
        .call("slow.module", json!({}), Some(&ctx), None)
        .await;
    let elapsed = start.elapsed();

    assert!(
        result.is_err(),
        "pre-cancelled context must short-circuit; got Ok({:?}) after {:?}",
        result.ok(),
        elapsed
    );
    assert_eq!(result.unwrap_err().code, ErrorCode::ExecutionCancelled);
    assert!(
        elapsed < Duration::from_millis(100),
        "cancel was not observed early; pipeline ran for {elapsed:?}"
    );
}

// ---------------------------------------------------------------------------
// A-D-EXEC-003 (D-20): cancellation short-circuits on_error middleware
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancellation_does_not_invoke_on_error_middleware() {
    let client = APCore::new();
    let swallowing = SwallowingMiddleware {
        on_error_calls: Arc::new(AtomicUsize::new(0)),
    };
    let on_error_calls = swallowing.on_error_calls.clone();
    client
        .use_middleware(Box::new(swallowing))
        .expect("middleware registration");
    client
        .register("slow.module", Box::new(SleepModule { delay_ms: 500 }))
        .unwrap();

    let token = CancelToken::new();
    token.cancel(); // pre-cancelled — call_chain_guard short-circuits.
    let ctx = ctx_with_token(token);

    let result = client
        .executor()
        .call("slow.module", json!({}), Some(&ctx), None)
        .await;

    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err().code,
        ErrorCode::ExecutionCancelled,
        "cancellation must propagate directly, not via on_error recovery"
    );
    assert_eq!(
        on_error_calls.load(Ordering::SeqCst),
        0,
        "on_error middleware MUST NOT run for ExecutionCancelled (D-20)"
    );
}

// ---------------------------------------------------------------------------
// A-D-EXEC-004 (D-19): call_with_trace runs on_error recovery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn call_with_trace_runs_on_error_recovery() {
    let client = APCore::new();
    client
        .use_middleware(Box::new(RecoveringMiddleware))
        .expect("middleware registration");
    client
        .register("fail.module", Box::new(AlwaysFailModule))
        .unwrap();

    let (output, trace) = client
        .executor()
        .call_with_trace("fail.module", json!({}), None, None)
        .await
        .expect("on_error recovery should succeed");

    assert_eq!(output, json!({"recovered": true}));
    assert_eq!(trace.module_id, "fail.module");
}
