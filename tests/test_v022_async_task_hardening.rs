//! Regression tests for v0.22 async-task hardening — A-D-AT-01, A-D-AT-05.
//!
//! Covers:
//! - A-D-AT-01: `max_tasks` capacity counts only active tasks (`Pending` +
//!   `Running`), not terminal-state records still pending TTL cleanup.
//! - A-D-AT-05: `start_reaper` is single-instance — a second call without
//!   `stop()` returns `ErrorCode::ReaperAlreadyRunning`.

use std::sync::Arc;
use std::time::Duration;

use apcore::async_task::{
    AsyncTaskManager, InMemoryTaskStore, ReaperConfig, TaskInfo, TaskStatus, TaskStore,
};
use apcore::config::Config;
use apcore::context::Context;
use apcore::errors::{ErrorCode, ModuleError};
use apcore::module::Module;
use apcore::registry::registry::Registry;
use apcore::Executor;
use async_trait::async_trait;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct NoopModule;

#[async_trait]
impl Module for NoopModule {
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn description(&self) -> &'static str {
        "noop"
    }
    async fn execute(&self, _inputs: Value, _ctx: &Context<Value>) -> Result<Value, ModuleError> {
        Ok(json!({"ok": true}))
    }
}

fn make_manager_with_store(max_tasks: usize, store: Arc<dyn TaskStore>) -> AsyncTaskManager {
    let registry = Arc::new(Registry::new());
    registry
        .register_module("noop.module", Box::new(NoopModule))
        .unwrap();
    let executor = Arc::new(Executor::new(registry, Arc::new(Config::default())));
    AsyncTaskManager::with_store(executor, /*max_concurrent=*/ 16, max_tasks, store)
}

fn make_manager(max_tasks: usize) -> AsyncTaskManager {
    make_manager_with_store(max_tasks, Arc::new(InMemoryTaskStore::new()))
}

// ---------------------------------------------------------------------------
// A-D-AT-01: max_tasks counts active statuses only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn max_tasks_counts_only_active_statuses() {
    let store = Arc::new(InMemoryTaskStore::new());

    // Pre-populate the store with `max_tasks` terminal-state records — these
    // are exactly the records that pluggable storage and TTL-based cleanup
    // are designed to retain. They MUST NOT consume the active budget.
    for i in 0..3 {
        let mut info = TaskInfo::default();
        info.task_id = format!("done-{i}");
        info.module_id = "noop.module".to_string();
        info.status = TaskStatus::Completed;
        info.completed_at = Some(0.0);
        info.started_at = Some(0.0);
        info.result = Some(json!({}));
        store.save(&info).await.unwrap();
    }

    let mgr = make_manager_with_store(/*max_tasks=*/ 3, store.clone() as Arc<dyn TaskStore>);

    // Even with 3 terminal records present, a new submission MUST succeed
    // because 0 active tasks < max_tasks=3.
    let result = mgr.submit("noop.module", json!({}), None).await;
    assert!(
        result.is_ok(),
        "submit must not be rejected by terminal-state records (closes A-D-AT-01); got {:?}",
        result.err()
    );
}

#[tokio::test]
async fn max_tasks_still_rejects_when_active_budget_exhausted() {
    // Sanity check: the active-count fix MUST NOT regress the original
    // protection — if active >= max_tasks, submit still fails.
    let store = Arc::new(InMemoryTaskStore::new());
    for i in 0..2 {
        let mut info = TaskInfo::default();
        info.task_id = format!("running-{i}");
        info.module_id = "noop.module".to_string();
        info.status = TaskStatus::Running;
        info.started_at = Some(0.0);
        store.save(&info).await.unwrap();
    }

    let mgr = make_manager_with_store(/*max_tasks=*/ 2, store as Arc<dyn TaskStore>);
    let err = mgr
        .submit("noop.module", json!({}), None)
        .await
        .expect_err("submit must fail when active >= max_tasks");
    assert_eq!(err.code, ErrorCode::TaskLimitExceeded);
}

// ---------------------------------------------------------------------------
// A-D-AT-05: start_reaper is single-instance
// ---------------------------------------------------------------------------

#[tokio::test]
async fn start_reaper_rejects_concurrent_start() {
    let mgr = make_manager(/*max_tasks=*/ 100);
    let mut cfg = ReaperConfig::default();
    cfg.ttl_seconds = 60.0;
    cfg.sweep_interval_ms = 5_000;

    let first = mgr
        .start_reaper(cfg)
        .expect("first start_reaper must succeed");

    let err = mgr
        .start_reaper(cfg)
        .expect_err("second start_reaper must fail while first is live");
    assert_eq!(err.code, ErrorCode::ReaperAlreadyRunning);

    // After stop() releases the flag, a fresh reaper can be started again.
    first.stop().await;
    let third = mgr
        .start_reaper(cfg)
        .expect("start_reaper must succeed after stop()");
    third.stop().await;
}

#[tokio::test]
async fn dropped_reaper_handle_releases_running_flag() {
    // Even when a caller drops the handle without calling stop(), the
    // Drop impl MUST release the running flag so subsequent calls succeed.
    let mgr = make_manager(/*max_tasks=*/ 100);
    let mut cfg = ReaperConfig::default();
    cfg.ttl_seconds = 60.0;
    cfg.sweep_interval_ms = 5_000;
    {
        let _detached = mgr.start_reaper(cfg).unwrap();
        // _detached drops here.
    }
    // Give the runtime a moment to surface the drop; spawning a new reaper
    // MUST not see the prior flag.
    tokio::time::sleep(Duration::from_millis(10)).await;
    let handle = mgr
        .start_reaper(cfg)
        .expect("drop must release running flag");
    handle.stop().await;
}
