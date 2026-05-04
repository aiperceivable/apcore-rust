// Integration tests for Pipeline StepMiddleware (Issue #33 §2.2)
// Verifies before_step / after_step / on_step_error hook ordering and recovery semantics.

use apcore::context::Context;
use apcore::errors::{ErrorCode, ModuleError};
use apcore::pipeline::{
    ExecutionStrategy, PipelineContext, PipelineEngine, PipelineState, Step, StepMiddleware,
    StepResult,
};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

struct OkStep {
    name: String,
    output: Value,
}

impl OkStep {
    fn boxed(name: &str, output: Value) -> Box<dyn Step> {
        Box::new(Self {
            name: name.to_string(),
            output,
        })
    }
}

#[async_trait]
impl Step for OkStep {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.name
    }
    fn removable(&self) -> bool {
        true
    }
    fn replaceable(&self) -> bool {
        true
    }
    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        ctx.output = Some(self.output.clone());
        Ok(StepResult::continue_step())
    }
}

struct FailingStep {
    name: String,
}

impl FailingStep {
    fn boxed(name: &str) -> Box<dyn Step> {
        Box::new(Self {
            name: name.to_string(),
        })
    }
}

#[async_trait]
impl Step for FailingStep {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.name
    }
    fn removable(&self) -> bool {
        true
    }
    fn replaceable(&self) -> bool {
        true
    }
    async fn execute(&self, _ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
        Err(ModuleError::new(ErrorCode::ModuleExecuteError, "step boom"))
    }
}

#[derive(Default)]
struct RecordingMiddleware {
    log: Arc<Mutex<Vec<String>>>,
    before_count: Arc<AtomicUsize>,
    after_count: Arc<AtomicUsize>,
    error_count: Arc<AtomicUsize>,
    recovery: Option<Value>,
}

#[async_trait]
impl StepMiddleware for RecordingMiddleware {
    async fn before_step(
        &self,
        step_name: &str,
        _state: &PipelineState<'_>,
    ) -> Result<(), ModuleError> {
        self.log.lock().push(format!("before:{step_name}"));
        self.before_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn after_step(
        &self,
        step_name: &str,
        _state: &PipelineState<'_>,
        _result: &Value,
    ) -> Result<(), ModuleError> {
        self.log.lock().push(format!("after:{step_name}"));
        self.after_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn on_step_error(
        &self,
        step_name: &str,
        _state: &PipelineState<'_>,
        _error: &ModuleError,
    ) -> Result<Option<Value>, ModuleError> {
        self.log.lock().push(format!("error:{step_name}"));
        self.error_count.fetch_add(1, Ordering::SeqCst);
        Ok(self.recovery.clone())
    }
}

fn ctx() -> PipelineContext {
    PipelineContext::new(
        "mod.x",
        serde_json::json!({}),
        Context::<Value>::anonymous(),
        "test",
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn step_middleware_invoked_before_and_after_each_step() {
    let mw = Arc::new(RecordingMiddleware::default());
    let log = Arc::clone(&mw.log);

    let mut strategy = ExecutionStrategy::new(
        "test",
        vec![
            OkStep::boxed("a", serde_json::json!(1)),
            OkStep::boxed("b", serde_json::json!(2)),
        ],
    )
    .unwrap();
    strategy.add_step_middleware(mw.clone() as Arc<dyn StepMiddleware>);

    let mut pctx = ctx();
    let (output, trace) = PipelineEngine::run(&strategy, &mut pctx).await.unwrap();

    assert!(trace.success);
    assert_eq!(output, Some(serde_json::json!(2)));
    assert_eq!(mw.before_count.load(Ordering::SeqCst), 2);
    assert_eq!(mw.after_count.load(Ordering::SeqCst), 2);
    assert_eq!(mw.error_count.load(Ordering::SeqCst), 0);

    let log = log.lock();
    assert_eq!(
        *log,
        vec![
            "before:a".to_string(),
            "after:a".to_string(),
            "before:b".to_string(),
            "after:b".to_string(),
        ]
    );
}

#[tokio::test]
async fn step_middleware_on_step_error_propagates_without_recovery() {
    let mw = Arc::new(RecordingMiddleware::default());

    let mut strategy = ExecutionStrategy::new(
        "test",
        vec![
            OkStep::boxed("ok", serde_json::json!("ok")),
            FailingStep::boxed("bad"),
            OkStep::boxed("never", serde_json::json!("never")),
        ],
    )
    .unwrap();
    strategy.add_step_middleware(mw.clone() as Arc<dyn StepMiddleware>);

    let mut pctx = ctx();
    let result = PipelineEngine::run(&strategy, &mut pctx).await;

    assert!(result.is_err());
    assert_eq!(mw.before_count.load(Ordering::SeqCst), 2);
    assert_eq!(mw.after_count.load(Ordering::SeqCst), 1);
    assert_eq!(mw.error_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn step_middleware_on_step_error_can_recover_with_value() {
    let mw = Arc::new(RecordingMiddleware {
        recovery: Some(serde_json::json!("recovered")),
        ..Default::default()
    });

    let mut strategy = ExecutionStrategy::new(
        "test",
        vec![
            FailingStep::boxed("bad"),
            OkStep::boxed("after", serde_json::json!("after")),
        ],
    )
    .unwrap();
    strategy.add_step_middleware(mw.clone() as Arc<dyn StepMiddleware>);

    let mut pctx = ctx();
    let (output, trace) = PipelineEngine::run(&strategy, &mut pctx).await.unwrap();

    assert!(trace.success);
    // Recovery from failing step then "after" runs cleanly.
    assert_eq!(output, Some(serde_json::json!("after")));
    assert_eq!(mw.error_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn step_middleware_multiple_run_in_registration_order() {
    let mw1 = Arc::new(RecordingMiddleware::default());
    let mw2 = Arc::new(RecordingMiddleware::default());
    let log1 = Arc::clone(&mw1.log);
    let log2 = Arc::clone(&mw2.log);

    let mut strategy =
        ExecutionStrategy::new("test", vec![OkStep::boxed("only", serde_json::json!(1))]).unwrap();
    strategy.add_step_middleware(mw1.clone() as Arc<dyn StepMiddleware>);
    strategy.add_step_middleware(mw2.clone() as Arc<dyn StepMiddleware>);

    let mut pctx = ctx();
    PipelineEngine::run(&strategy, &mut pctx).await.unwrap();

    assert_eq!(*log1.lock(), vec!["before:only", "after:only"]);
    assert_eq!(*log2.lock(), vec!["before:only", "after:only"]);
}
