// APCore Protocol — Async task manager for background module execution
// Spec reference: docs/features/async-tasks.md
// Issue #34: Pluggable TaskStore, retry with exponential backoff, Reaper TTL cleanup.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use dashmap::DashMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::{watch, Semaphore};
use tokio::task::JoinHandle;
use tracing::{debug, error, warn};
use uuid::Uuid;

use crate::context::Context;
use crate::errors::{ErrorCode, ModuleError};
use crate::executor::Executor;

/// Status of an async task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    fn is_active(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }
}

/// Metadata and result tracking for a submitted async task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    pub task_id: String,
    pub module_id: String,
    pub status: TaskStatus,
    pub submitted_at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub retry_count: u32,
    #[serde(default)]
    pub max_retries: u32,
}

/// Returns the current time as seconds since the UNIX epoch.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

// ---------------------------------------------------------------------------
// TaskStore
// ---------------------------------------------------------------------------

/// Pluggable backing store for [`TaskInfo`] records.
///
/// Implementations decouple task state from in-process memory and enable
/// distributed deployments. The default backend is [`InMemoryTaskStore`].
///
/// Concrete implementations include `RedisTaskStore` and `SqlTaskStore`
/// (provided as optional add-ons in downstream crates).
#[async_trait]
pub trait TaskStore: Send + Sync {
    /// Persist a task record (create or overwrite).
    async fn save(&self, task: &TaskInfo) -> Result<(), ModuleError>;

    /// Look up a task by id; returns `None` if no record exists.
    async fn get(&self, id: &str) -> Result<Option<TaskInfo>, ModuleError>;

    /// List all tasks, optionally filtered by exact status match.
    async fn list(&self, status: Option<TaskStatus>) -> Result<Vec<TaskInfo>, ModuleError>;

    /// Remove a task record. No-op if `id` is absent.
    async fn delete(&self, id: &str) -> Result<(), ModuleError>;

    /// Return all terminal-state tasks whose `completed_at` is strictly less
    /// than `before_timestamp` (Unix seconds). Pending and Running tasks MUST
    /// NOT be returned by this method.
    async fn list_expired(&self, before_timestamp: f64) -> Result<Vec<TaskInfo>, ModuleError>;

    /// Identifier of the concrete store type, used by tooling to expose the
    /// active backend (matches the type name; e.g. `"InMemoryTaskStore"`).
    fn store_type_name(&self) -> &'static str;
}

/// Default in-memory [`TaskStore`] backed by [`DashMap`] for lock-free
/// concurrent access.
#[derive(Default)]
pub struct InMemoryTaskStore {
    tasks: DashMap<String, TaskInfo>,
}

impl InMemoryTaskStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl TaskStore for InMemoryTaskStore {
    async fn save(&self, task: &TaskInfo) -> Result<(), ModuleError> {
        self.tasks.insert(task.task_id.clone(), task.clone());
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<TaskInfo>, ModuleError> {
        Ok(self.tasks.get(id).map(|entry| entry.clone()))
    }

    async fn list(&self, status: Option<TaskStatus>) -> Result<Vec<TaskInfo>, ModuleError> {
        let mut out: Vec<TaskInfo> = self
            .tasks
            .iter()
            .filter(|entry| match status {
                Some(s) => entry.value().status == s,
                None => true,
            })
            .map(|entry| entry.value().clone())
            .collect();
        // Stable order is helpful for deterministic tests; sort by task_id.
        out.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        Ok(out)
    }

    async fn delete(&self, id: &str) -> Result<(), ModuleError> {
        self.tasks.remove(id);
        Ok(())
    }

    async fn list_expired(&self, before_timestamp: f64) -> Result<Vec<TaskInfo>, ModuleError> {
        let mut out: Vec<TaskInfo> = self
            .tasks
            .iter()
            .filter(|entry| {
                let info = entry.value();
                if !info.status.is_terminal() {
                    return false;
                }
                match info.completed_at {
                    Some(ts) => ts < before_timestamp,
                    None => false,
                }
            })
            .map(|entry| entry.value().clone())
            .collect();
        out.sort_by(|a, b| a.task_id.cmp(&b.task_id));
        Ok(out)
    }

    fn store_type_name(&self) -> &'static str {
        "InMemoryTaskStore"
    }
}

// ---------------------------------------------------------------------------
// RetryConfig
// ---------------------------------------------------------------------------

/// Retry policy applied per task on failure.
#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    /// Maximum number of retry attempts after the initial execution. `0` disables retry.
    pub max_retries: u32,
    /// Base delay between retries in milliseconds.
    pub retry_delay_ms: u64,
    /// Multiplicative factor applied at each attempt (`1.0` = constant, `2.0` = exponential doubling).
    pub backoff_multiplier: f64,
    /// Upper bound on the computed retry delay, in milliseconds.
    pub max_retry_delay_ms: u64,
}

impl Default for RetryConfig {
    /// Spec-aligned default (D-14): `max_retries = 0` matches both
    /// `apcore-python` and `apcore-typescript`. Retries are explicitly opt-in;
    /// this prevents the SDK from silently re-running failed tasks behind the
    /// caller's back. The 1s base delay / 2.0× backoff / 60s ceiling stay
    /// useful when callers DO opt into retries, so they're preserved.
    fn default() -> Self {
        Self {
            max_retries: 0,
            retry_delay_ms: 1000,
            backoff_multiplier: 2.0,
            max_retry_delay_ms: 60_000,
        }
    }
}

impl RetryConfig {
    /// Compute the retry delay for the given attempt index (`0`-based).
    ///
    /// Formula: `min(retry_delay_ms * (backoff_multiplier ^ attempt), max_retry_delay_ms)`.
    ///
    /// Cross-language: this is the canonical name across `apcore-python` and
    /// `apcore-typescript` (sync alignment D-08). The legacy
    /// [`Self::delay_for_attempt`] alias delegates to this method and is
    /// `#[deprecated]`; it will be removed in the next minor version.
    #[must_use]
    pub fn compute_delay_ms(&self, attempt: u32) -> u64 {
        // Use f64 arithmetic to apply the backoff factor, then clamp to the cap.
        // The `as f64` casts are intentional: retry delays are bounded small
        // integers in practice (sub-second to minutes), so precision loss is a
        // non-issue.
        #[allow(clippy::cast_precision_loss)]
        let base = self.retry_delay_ms as f64;
        // `powf` accepts an f64 exponent and avoids the i32 cast lint while
        // producing the same result for non-negative integer attempt values.
        let raw = base * self.backoff_multiplier.powf(f64::from(attempt));
        #[allow(clippy::cast_precision_loss)]
        let cap = self.max_retry_delay_ms as f64;
        let capped = raw.min(cap);
        // Saturate negative or NaN values to zero, then truncate.
        if !capped.is_finite() || capped <= 0.0 {
            return 0;
        }
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let out = capped as u64;
        out
    }

    /// Deprecated alias for [`Self::compute_delay_ms`] (sync alignment D-08).
    ///
    /// Kept for one minor version to allow callers a graceful migration.
    #[must_use]
    #[deprecated(since = "0.21.0", note = "use compute_delay_ms")]
    pub fn delay_for_attempt(&self, attempt: u32) -> u64 {
        self.compute_delay_ms(attempt)
    }
}

// ---------------------------------------------------------------------------
// Reaper
// ---------------------------------------------------------------------------

/// Configuration for the [`AsyncTaskManager`] background reaper.
#[derive(Debug, Clone, Copy)]
pub struct ReaperConfig {
    /// Age threshold (seconds) before a terminal task becomes eligible for deletion.
    pub ttl_seconds: f64,
    /// Sweep interval (milliseconds) between reaper runs.
    pub sweep_interval_ms: u64,
}

impl Default for ReaperConfig {
    fn default() -> Self {
        Self {
            ttl_seconds: 3600.0,
            sweep_interval_ms: 300_000,
        }
    }
}

/// Handle returned by [`AsyncTaskManager::start_reaper`] to control the
/// background reaper task. Drop the handle to detach (the reaper continues to
/// run); call [`ReaperHandle::stop`] to gracefully signal cancellation and
/// await termination.
pub struct ReaperHandle {
    handle: JoinHandle<()>,
    stop_tx: watch::Sender<bool>,
}

impl ReaperHandle {
    /// Signal the reaper to stop and await its clean shutdown.
    pub async fn stop(self) {
        // Receivers may have already been dropped — ignore the send error.
        let _ = self.stop_tx.send(true);
        // The reaper observes the signal and exits its loop; await the join.
        if let Err(err) = self.handle.await {
            if !err.is_cancelled() {
                warn!("reaper task join failed: {err}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// AsyncTaskManager
// ---------------------------------------------------------------------------

/// Manages background execution of modules via tokio tasks.
///
/// Bounds concurrency with a semaphore, persists task state through a
/// pluggable [`TaskStore`], and supports retry with exponential backoff and
/// an opt-in [`ReaperHandle`] for TTL-based cleanup.
pub struct AsyncTaskManager {
    executor: Arc<Executor>,
    max_tasks: usize,
    store: Arc<dyn TaskStore>,
    handles: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
    semaphore: Arc<Semaphore>,
    // Serialises the capacity-check + save sequence in `submit_with_retry`.
    // Without this guard, two concurrent submits can both observe
    // `store.list().len() < max_tasks` and both `save()` — exceeding the cap.
    // The lock is held only for the synchronous `block_on_local` poll; we never
    // hold it across a real .await (the in-memory store resolves immediately,
    // and a yielding store would already panic in `block_on_local`).
    admission_lock: Arc<Mutex<()>>,
}

impl AsyncTaskManager {
    /// Create a new manager backed by the default [`InMemoryTaskStore`].
    ///
    /// # Arguments
    ///
    /// * `executor` — Executor used to invoke modules.
    /// * `max_concurrent` — Maximum simultaneously running tasks (semaphore size).
    /// * `max_tasks` — Maximum tracked task records (rejects further submits).
    pub fn new(executor: Arc<Executor>, max_concurrent: usize, max_tasks: usize) -> Self {
        Self::with_store(
            executor,
            max_concurrent,
            max_tasks,
            Arc::new(InMemoryTaskStore::new()),
        )
    }

    /// Create a manager with a caller-provided [`TaskStore`] implementation.
    pub fn with_store(
        executor: Arc<Executor>,
        max_concurrent: usize,
        max_tasks: usize,
        store: Arc<dyn TaskStore>,
    ) -> Self {
        Self {
            executor,
            max_tasks,
            store,
            handles: Arc::new(Mutex::new(HashMap::new())),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            admission_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Identifier of the underlying store backend.
    pub fn store_type_name(&self) -> &'static str {
        self.store.store_type_name()
    }

    /// Borrow the underlying [`TaskStore`] handle (for direct interaction or
    /// custom maintenance routines).
    pub fn store(&self) -> Arc<dyn TaskStore> {
        Arc::clone(&self.store)
    }

    /// Submit a module call for background execution. See [`Self::submit_with_retry`].
    pub fn submit(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        context: Option<Context<serde_json::Value>>,
    ) -> Result<String, ModuleError> {
        self.submit_with_retry(module_id, inputs, context, None)
    }

    /// Submit a module call with optional retry policy.
    ///
    /// Returns the generated task ID (UUID v4). Spawns a background tokio task
    /// that will acquire a concurrency permit before invoking the executor.
    /// On failure, when `retry` is supplied, the task is rescheduled after
    /// the policy-derived backoff delay until `max_retries` is exhausted.
    pub fn submit_with_retry(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        context: Option<Context<serde_json::Value>>,
        retry: Option<RetryConfig>,
    ) -> Result<String, ModuleError> {
        // Construct the initial TaskInfo and reserve a slot in the store.
        // Concurrent submits are serialised through `admission_lock` so the
        // capacity check and subsequent save below are atomic.
        let task_id = Uuid::new_v4().to_string();
        let max_retries = retry.as_ref().map_or(0, |r| r.max_retries);
        let info = TaskInfo {
            task_id: task_id.clone(),
            module_id: module_id.to_string(),
            status: TaskStatus::Pending,
            submitted_at: now_secs(),
            started_at: None,
            completed_at: None,
            result: None,
            error: None,
            retry_count: 0,
            max_retries,
        };

        // Capacity check + persist must be atomic with respect to other
        // concurrent submits, otherwise two submitters can both pass the
        // `len < max_tasks` check before either saves. Hold `admission_lock`
        // around the whole sequence. The lock is sync (parking_lot) and is
        // only held for the single immediate poll of `block_on_local` — no
        // real await happens while the guard is alive.
        let store = Arc::clone(&self.store);
        let max_tasks = self.max_tasks;
        let info_clone = info.clone();
        {
            let _admit = self.admission_lock.lock();
            block_on_local(async move {
                check_capacity_and_save(&*store, max_tasks, &info_clone).await
            })?;
        }

        let handles = Arc::clone(&self.handles);
        let semaphore = Arc::clone(&self.semaphore);
        let executor = Arc::clone(&self.executor);
        let store_for_run = Arc::clone(&self.store);
        let mid = module_id.to_string();
        let tid = task_id.clone();

        let handle = tokio::spawn(async move {
            run_task(
                tid.clone(),
                mid,
                inputs,
                context,
                retry,
                executor,
                semaphore,
                store_for_run,
            )
            .await;
            handles.lock().remove(&tid);
        });

        self.handles.lock().insert(task_id.clone(), handle);

        Ok(task_id)
    }

    /// Return the current snapshot of a task, or `None` if unknown.
    ///
    /// This is the synchronous wrapper used by the existing public API. For
    /// network-backed stores, prefer [`Self::get_status_async`].
    pub fn get_status(&self, task_id: &str) -> Option<TaskInfo> {
        block_on_local(self.store.get(task_id)).ok().flatten()
    }

    /// Async variant of [`Self::get_status`] for network-backed stores.
    pub async fn get_status_async(&self, task_id: &str) -> Option<TaskInfo> {
        self.store.get(task_id).await.ok().flatten()
    }

    /// Return the result of a completed task or an error if not found / not completed.
    pub fn get_result(&self, task_id: &str) -> Result<serde_json::Value, ModuleError> {
        block_on_local(self.get_result_async(task_id))
    }

    /// Async variant of [`Self::get_result`] for network-backed stores.
    pub async fn get_result_async(&self, task_id: &str) -> Result<serde_json::Value, ModuleError> {
        let info = self.store.get(task_id).await?.ok_or_else(|| {
            ModuleError::new(
                ErrorCode::GeneralInternalError,
                format!("Task not found: {task_id}"),
            )
        })?;
        if info.status != TaskStatus::Completed {
            return Err(ModuleError::new(
                ErrorCode::GeneralInternalError,
                format!("Task {task_id} is not completed (status={:?})", info.status),
            ));
        }
        Ok(info.result.unwrap_or(serde_json::Value::Null))
    }

    /// Cancel a pending or running task. Returns `true` if cancellation was applied.
    pub fn cancel(&self, task_id: &str) -> bool {
        let store = Arc::clone(&self.store);
        let Some(info) = block_on_local(store.get(task_id)).ok().flatten() else {
            return false;
        };
        if !info.status.is_active() {
            return false;
        }

        if let Some(handle) = self.handles.lock().remove(task_id) {
            handle.abort();
        }

        // Transition to Cancelled if still active.
        let mut updated = info;
        if updated.status.is_active() {
            updated.status = TaskStatus::Cancelled;
            updated.completed_at = Some(now_secs());
            let _ = block_on_local(self.store.save(&updated));
        }
        true
    }

    /// Cancel all pending and running tasks.
    pub fn shutdown(&self) {
        let task_ids: Vec<String> = block_on_local(self.store.list(None))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|info| info.status.is_active().then_some(info.task_id))
            .collect();

        for task_id in task_ids {
            self.cancel(&task_id);
        }
    }

    /// Return all tasks, optionally filtered by status. Synchronous wrapper.
    pub fn list_tasks(&self, status: Option<TaskStatus>) -> Vec<TaskInfo> {
        block_on_local(self.store.list(status)).unwrap_or_default()
    }

    /// Remove terminal-state tasks older than `max_age_seconds`. Returns the
    /// count of removed tasks.
    pub fn cleanup(&self, max_age_seconds: f64) -> usize {
        let now = now_secs();
        let to_remove: Vec<String> = block_on_local(self.store.list(None))
            .unwrap_or_default()
            .into_iter()
            .filter(|info| info.status.is_terminal())
            .filter(|info| {
                let ref_time = info.completed_at.unwrap_or(info.submitted_at);
                (now - ref_time) >= max_age_seconds
            })
            .map(|info| info.task_id)
            .collect();

        let count = to_remove.len();
        for id in &to_remove {
            let _ = block_on_local(self.store.delete(id));
            self.handles.lock().remove(id);
        }
        count
    }

    /// Total tracked task count across all states.
    pub fn task_count(&self) -> usize {
        block_on_local(self.store.list(None))
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// Start the opt-in background reaper. Returns a [`ReaperHandle`] used to
    /// stop the reaper gracefully. Multiple concurrent reapers can be started;
    /// callers SHOULD guard against this and keep at most one handle live.
    pub fn start_reaper(&self, config: ReaperConfig) -> ReaperHandle {
        let store = Arc::clone(&self.store);
        let (stop_tx, mut stop_rx) = watch::channel(false);
        let interval = Duration::from_millis(config.sweep_interval_ms);
        let ttl = config.ttl_seconds;

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    changed = stop_rx.changed() => {
                        if changed.is_ok() && *stop_rx.borrow() {
                            debug!("reaper received stop signal");
                            return;
                        }
                    }
                    () = tokio::time::sleep(interval) => {
                        let before = now_secs() - ttl;
                        match store.list_expired(before).await {
                            Ok(expired) => {
                                let count = expired.len();
                                for info in &expired {
                                    if let Err(err) = store.delete(&info.task_id).await {
                                        warn!(task_id = %info.task_id, "reaper delete failed: {err}");
                                    }
                                }
                                if count > 0 {
                                    debug!("reaper deleted {count} expired tasks");
                                }
                            }
                            Err(err) => {
                                warn!("reaper list_expired failed: {err}");
                            }
                        }
                    }
                }
            }
        });

        ReaperHandle { handle, stop_tx }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Capacity-check then persist a new task record.
async fn check_capacity_and_save(
    store: &dyn TaskStore,
    max_tasks: usize,
    info: &TaskInfo,
) -> Result<(), ModuleError> {
    let current = store.list(None).await?;
    if current.len() >= max_tasks {
        return Err(ModuleError::new(
            ErrorCode::TaskLimitExceeded,
            format!("Task limit reached ({max_tasks})"),
        ));
    }
    store.save(info).await?;
    Ok(())
}

/// Run a single task with optional retry/backoff. Persists state transitions
/// via the [`TaskStore`].
#[allow(clippy::too_many_arguments)]
async fn run_task(
    task_id: String,
    module_id: String,
    inputs: serde_json::Value,
    context: Option<Context<serde_json::Value>>,
    retry: Option<RetryConfig>,
    executor: Arc<Executor>,
    semaphore: Arc<Semaphore>,
    store: Arc<dyn TaskStore>,
) {
    let max_retries = retry.as_ref().map_or(0, |r| r.max_retries);

    loop {
        // Acquire concurrency permit. A closed semaphore is treated as
        // cancellation (matches the legacy `_run` behaviour).
        let Ok(permit) = semaphore.acquire().await else {
            mark_cancelled(&store, &task_id).await;
            return;
        };

        // Re-fetch and short-circuit if cancellation happened during the wait.
        let Ok(Some(mut info)) = store.get(&task_id).await else {
            return;
        };
        if info.status == TaskStatus::Cancelled {
            return;
        }
        info.status = TaskStatus::Running;
        if info.started_at.is_none() {
            info.started_at = Some(now_secs());
        }
        if let Err(err) = store.save(&info).await {
            error!(task_id = %task_id, "store.save(running) failed: {err}");
            return;
        }

        // Execute the module.
        let result = executor
            .call(&module_id, inputs.clone(), context.as_ref(), None)
            .await;
        drop(permit);

        // Re-fetch in case cancel() raced with the executor call.
        let Ok(Some(mut info)) = store.get(&task_id).await else {
            return;
        };
        if info.status == TaskStatus::Cancelled {
            return;
        }

        match result {
            Ok(output) => {
                info.status = TaskStatus::Completed;
                info.completed_at = Some(now_secs());
                info.result = Some(output);
                save_terminal_if_not_cancelled(&store, &task_id, &info).await;
                return;
            }
            Err(err) => {
                if let Some(cfg) = retry.as_ref() {
                    if info.retry_count < max_retries {
                        let delay_ms = cfg.compute_delay_ms(info.retry_count);
                        info.retry_count += 1;
                        info.status = TaskStatus::Pending;
                        // `started_at` is intentionally NOT reset across retries:
                        // it captures the wall-clock of the first execution and
                        // matches the Python reference behaviour so cross-language
                        // TaskInfo snapshots remain comparable mid-retry.
                        let _ = store.save(&info).await;
                        debug!(
                            task_id = %task_id,
                            attempt = info.retry_count,
                            delay_ms,
                            "scheduling retry"
                        );
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        continue;
                    }
                }
                info.status = TaskStatus::Failed;
                info.completed_at = Some(now_secs());
                info.error = Some(err.to_string());
                save_terminal_if_not_cancelled(&store, &task_id, &info).await;
                error!(task_id = %task_id, "task failed: {err}");
                return;
            }
        }
    }
}

/// Re-fetches the task immediately before writing a terminal status. If a
/// concurrent `cancel()` flipped the task to Cancelled while the executor was
/// finishing, leave that state intact — do not overwrite it with
/// Completed/Failed. The intermediate `re-fetch + check + save` is not
/// atomic at the store level, so a perfectly-timed cancel can still slip in
/// between the `get` and the `save`; closing that final window would require
/// CAS support in the `TaskStore` trait. The window left here is small and
/// the cost of slipping (one extra terminal write that the next cancel will
/// no-op against) is non-corrupting.
pub(crate) async fn save_terminal_if_not_cancelled(
    store: &Arc<dyn TaskStore>,
    task_id: &str,
    info: &TaskInfo,
) {
    if let Ok(Some(current)) = store.get(task_id).await {
        if current.status == TaskStatus::Cancelled {
            return;
        }
    }
    let _ = store.save(info).await;
}

async fn mark_cancelled(store: &Arc<dyn TaskStore>, task_id: &str) {
    if let Ok(Some(mut info)) = store.get(task_id).await {
        if info.status.is_active() {
            info.status = TaskStatus::Cancelled;
            info.completed_at = Some(now_secs());
            let _ = store.save(&info).await;
        }
    }
}

/// Drive a future to completion synchronously by polling it once with a
/// no-op waker. This is intentional: the manager exposes a synchronous
/// facade on top of an async [`TaskStore`], and the supported in-process
/// stores ([`InMemoryTaskStore`]) resolve without yielding so a single poll
/// is sufficient. For network-backed stores callers MUST use the `_async`
/// variants instead.
///
/// If a custom store actually yields, this panics — surfacing the misuse
/// rather than silently deadlocking the calling thread.
fn block_on_local<F, T>(fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    use std::pin::pin;
    use std::ptr;
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    // No-op waker: synchronous stores never wake themselves; if they did, we
    // would not be in this code path.
    const VTABLE: RawWakerVTable = RawWakerVTable::new(
        |_| RawWaker::new(ptr::null(), &VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );
    // SAFETY: the vtable above performs no operations on the data pointer,
    // so a null pointer is safe for every callback.
    let waker = unsafe { Waker::from_raw(RawWaker::new(ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = pin!(fut);
    match fut.as_mut().poll(&mut cx) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!(
            "block_on_local: TaskStore future yielded — use the _async variants for non-blocking stores"
        ),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Executor;
    use crate::registry::registry::Registry;

    fn make_executor() -> Arc<Executor> {
        let registry = Arc::new(Registry::default());
        let config = Arc::new(crate::config::Config::default());
        Arc::new(Executor::new(registry, config))
    }

    #[test]
    fn retry_delay_grows_exponentially_and_caps() {
        let cfg = RetryConfig {
            max_retries: 5,
            retry_delay_ms: 1000,
            backoff_multiplier: 2.0,
            max_retry_delay_ms: 30_000,
        };
        assert_eq!(cfg.compute_delay_ms(0), 1000);
        assert_eq!(cfg.compute_delay_ms(1), 2000);
        assert_eq!(cfg.compute_delay_ms(2), 4000);
        assert_eq!(cfg.compute_delay_ms(3), 8000);
        assert_eq!(cfg.compute_delay_ms(4), 16_000);
        assert_eq!(cfg.compute_delay_ms(5), 30_000);
    }

    #[tokio::test]
    async fn default_store_is_in_memory() {
        let mgr = AsyncTaskManager::new(make_executor(), 4, 100);
        assert_eq!(mgr.store_type_name(), "InMemoryTaskStore");
    }

    #[tokio::test]
    async fn in_memory_store_save_and_get_round_trip() {
        let store = InMemoryTaskStore::new();
        let info = TaskInfo {
            task_id: "abc".into(),
            module_id: "data.process".into(),
            status: TaskStatus::Completed,
            submitted_at: 1.0,
            started_at: Some(2.0),
            completed_at: Some(3.0),
            result: Some(serde_json::json!({"ok": true})),
            error: None,
            retry_count: 0,
            max_retries: 0,
        };
        store.save(&info).await.unwrap();
        let got = store.get("abc").await.unwrap().unwrap();
        assert_eq!(got.task_id, "abc");
        assert_eq!(got.status, TaskStatus::Completed);
    }

    #[tokio::test]
    async fn in_memory_store_list_filters_by_status() {
        let store = InMemoryTaskStore::new();
        for (id, status) in [
            ("c1", TaskStatus::Completed),
            ("c2", TaskStatus::Running),
            ("c3", TaskStatus::Failed),
        ] {
            store
                .save(&TaskInfo {
                    task_id: id.into(),
                    module_id: "m".into(),
                    status,
                    submitted_at: 0.0,
                    started_at: None,
                    completed_at: None,
                    result: None,
                    error: None,
                    retry_count: 0,
                    max_retries: 0,
                })
                .await
                .unwrap();
        }
        let completed = store.list(Some(TaskStatus::Completed)).await.unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].task_id, "c1");
    }

    #[tokio::test]
    async fn in_memory_store_list_expired_skips_active_tasks() {
        let store = InMemoryTaskStore::new();
        store
            .save(&TaskInfo {
                task_id: "old-completed".into(),
                module_id: "m".into(),
                status: TaskStatus::Completed,
                submitted_at: 0.0,
                started_at: Some(0.0),
                completed_at: Some(100.0),
                result: None,
                error: None,
                retry_count: 0,
                max_retries: 0,
            })
            .await
            .unwrap();
        store
            .save(&TaskInfo {
                task_id: "old-running".into(),
                module_id: "m".into(),
                status: TaskStatus::Running,
                submitted_at: 0.0,
                started_at: Some(0.0),
                completed_at: None,
                result: None,
                error: None,
                retry_count: 0,
                max_retries: 0,
            })
            .await
            .unwrap();
        let expired = store.list_expired(1000.0).await.unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].task_id, "old-completed");
    }

    /// Regression: when a `cancel()` flips the task to Cancelled in the
    /// store between the run-task post-execution re-fetch and its terminal
    /// `save()`, the helper MUST refrain from overwriting the Cancelled
    /// status with Completed/Failed.
    #[tokio::test]
    async fn save_terminal_does_not_overwrite_cancelled() {
        let store: Arc<dyn TaskStore> = Arc::new(InMemoryTaskStore::new());
        let task_id = "race-task";

        // Seed the store with the cancelled state (as `cancel()` would have
        // written between re-fetch and the terminal save).
        store
            .save(&TaskInfo {
                task_id: task_id.into(),
                module_id: "m".into(),
                status: TaskStatus::Cancelled,
                submitted_at: 0.0,
                started_at: Some(0.0),
                completed_at: Some(1.0),
                result: None,
                error: None,
                retry_count: 0,
                max_retries: 0,
            })
            .await
            .unwrap();

        // The terminal info `run_task` would otherwise write — Completed
        // with a payload. The helper must observe the Cancelled state in
        // the store and skip the save.
        let terminal = TaskInfo {
            task_id: task_id.into(),
            module_id: "m".into(),
            status: TaskStatus::Completed,
            submitted_at: 0.0,
            started_at: Some(0.0),
            completed_at: Some(2.0),
            result: Some(serde_json::json!({"value": 42})),
            error: None,
            retry_count: 0,
            max_retries: 0,
        };
        save_terminal_if_not_cancelled(&store, task_id, &terminal).await;

        let after = store.get(task_id).await.unwrap().expect("task present");
        assert_eq!(
            after.status,
            TaskStatus::Cancelled,
            "terminal save MUST NOT overwrite a concurrent cancellation"
        );
        assert!(
            after.result.is_none(),
            "the cancel-time TaskInfo had no result; the overwriting Completed payload must not leak through"
        );
    }
}
