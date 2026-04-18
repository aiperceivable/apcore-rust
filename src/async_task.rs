// APCore Protocol — Async task manager for background module execution
// Spec reference: Background execution with concurrency limiting

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tracing::error;
use uuid::Uuid;

use crate::context::Context;
use crate::errors::ModuleError;
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

/// Metadata and result tracking for a submitted async task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    pub task_id: String,
    pub module_id: String,
    pub status: TaskStatus,
    pub submitted_at: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Returns the current time as seconds since the UNIX epoch.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// Manages background execution of modules via tokio tasks.
///
/// Limits concurrency with a semaphore and tracks task lifecycle.
pub struct AsyncTaskManager {
    executor: Arc<Executor>,
    max_tasks: usize,
    tasks: Arc<Mutex<HashMap<String, TaskInfo>>>,
    handles: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
    semaphore: Arc<Semaphore>,
}

impl AsyncTaskManager {
    /// Create a new `AsyncTaskManager`.
    ///
    /// # Arguments
    ///
    /// * `executor` — The executor used to run modules.
    /// * `max_concurrent` — Maximum number of tasks running simultaneously.
    /// * `max_tasks` — Maximum number of tracked tasks (pending + active + terminal).
    pub fn new(executor: Arc<Executor>, max_concurrent: usize, max_tasks: usize) -> Self {
        Self {
            executor,
            max_tasks,
            tasks: Arc::new(Mutex::new(HashMap::new())),
            handles: Arc::new(Mutex::new(HashMap::new())),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
        }
    }

    /// Submit a module for background execution.
    ///
    /// Creates a `TaskInfo` in `Pending` state, spawns a tokio task that
    /// acquires the concurrency semaphore before calling `executor.call()`.
    ///
    /// Returns the generated task ID (UUID v4 string).
    pub fn submit(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        context: Option<Context<serde_json::Value>>,
    ) -> Result<String, ModuleError> {
        // Hold the lock across both the capacity check and the insert so
        // concurrent submit() calls cannot both observe task_count < max_tasks
        // and both insert, violating the cap (TOCTOU). Mirrors Python's
        // single-lock pattern in AsyncTaskManager.submit.
        let task_id = {
            let mut tasks = self.tasks.lock();
            if tasks.len() >= self.max_tasks {
                return Err(ModuleError::new(
                    crate::errors::ErrorCode::GeneralInternalError,
                    format!("Task limit reached ({})", self.max_tasks),
                ));
            }
            let task_id = Uuid::new_v4().to_string();
            let info = TaskInfo {
                task_id: task_id.clone(),
                module_id: module_id.to_string(),
                status: TaskStatus::Pending,
                submitted_at: now_secs(),
                started_at: None,
                completed_at: None,
                result: None,
                error: None,
            };
            tasks.insert(task_id.clone(), info);
            task_id
        };

        let tasks = Arc::clone(&self.tasks);
        let handles = Arc::clone(&self.handles);
        let semaphore = Arc::clone(&self.semaphore);
        let executor = Arc::clone(&self.executor);
        let mid = module_id.to_string();
        let tid = task_id.clone();

        let handle = tokio::spawn(async move {
            Self::run_task(
                tid.clone(),
                mid,
                inputs,
                context,
                executor,
                semaphore,
                tasks,
            )
            .await;
            handles.lock().remove(&tid);
        });

        self.handles.lock().insert(task_id.clone(), handle);

        Ok(task_id)
    }

    /// Return the `TaskInfo` for a task, or `None` if not found.
    pub fn get_status(&self, task_id: &str) -> Option<TaskInfo> {
        self.tasks.lock().get(task_id).cloned()
    }

    /// Return the result of a completed task.
    ///
    /// Returns an error if the task is not found or not in `Completed` status.
    pub fn get_result(&self, task_id: &str) -> Result<serde_json::Value, ModuleError> {
        let tasks = self.tasks.lock();
        let info = tasks.get(task_id).ok_or_else(|| {
            ModuleError::new(
                crate::errors::ErrorCode::GeneralInternalError,
                format!("Task not found: {task_id}"),
            )
        })?;
        if info.status != TaskStatus::Completed {
            return Err(ModuleError::new(
                crate::errors::ErrorCode::GeneralInternalError,
                format!("Task {task_id} is not completed (status={:?})", info.status),
            ));
        }
        Ok(info.result.clone().unwrap_or(serde_json::Value::Null))
    }

    /// Cancel a running or pending task.
    ///
    /// Aborts the tokio task and marks it as `Cancelled`.
    ///
    /// Returns `true` if the task was successfully cancelled.
    pub fn cancel(&self, task_id: &str) -> bool {
        let should_cancel = {
            let tasks = self.tasks.lock();
            match tasks.get(task_id) {
                Some(info) => matches!(info.status, TaskStatus::Pending | TaskStatus::Running),
                None => false,
            }
        };

        if !should_cancel {
            return false;
        }

        // Abort the tokio task if it exists
        if let Some(handle) = self.handles.lock().remove(task_id) {
            handle.abort();
        }

        // Force status to Cancelled if still active
        let mut tasks = self.tasks.lock();
        if let Some(info) = tasks.get_mut(task_id) {
            if matches!(info.status, TaskStatus::Pending | TaskStatus::Running) {
                info.status = TaskStatus::Cancelled;
                info.completed_at = Some(now_secs());
            }
        }

        true
    }

    /// Cancel all pending and running tasks.
    pub fn shutdown(&self) {
        let task_ids: Vec<String> = {
            let tasks = self.tasks.lock();
            tasks
                .iter()
                .filter(|(_, info)| {
                    matches!(info.status, TaskStatus::Pending | TaskStatus::Running)
                })
                .map(|(id, _)| id.clone())
                .collect()
        };

        for task_id in task_ids {
            self.cancel(&task_id);
        }
    }

    /// Return all tasks, optionally filtered by status.
    pub fn list_tasks(&self, status: Option<TaskStatus>) -> Vec<TaskInfo> {
        let tasks = self.tasks.lock();
        match status {
            None => tasks.values().cloned().collect(),
            Some(s) => tasks
                .values()
                .filter(|info| info.status == s)
                .cloned()
                .collect(),
        }
    }

    /// Remove terminal-state tasks older than `max_age_seconds`.
    ///
    /// Terminal states: `Completed`, `Failed`, `Cancelled`.
    ///
    /// Returns the number of tasks removed.
    pub fn cleanup(&self, max_age_seconds: f64) -> usize {
        let now = now_secs();
        let mut tasks = self.tasks.lock();

        let to_remove: Vec<String> = tasks
            .iter()
            .filter(|(_, info)| {
                matches!(
                    info.status,
                    TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
                )
            })
            .filter(|(_, info)| {
                let ref_time = info.completed_at.unwrap_or(info.submitted_at);
                (now - ref_time) >= max_age_seconds
            })
            .map(|(id, _)| id.clone())
            .collect();

        let count = to_remove.len();
        for id in &to_remove {
            tasks.remove(id);
        }
        // Also clean up any stale handles (should already be removed, but be safe)
        let mut handles = self.handles.lock();
        for id in &to_remove {
            handles.remove(id);
        }

        count
    }

    /// Maximum number of currently tracked tasks (all states).
    pub fn task_count(&self) -> usize {
        self.tasks.lock().len()
    }

    /// Internal coroutine that executes a module under the concurrency semaphore.
    async fn run_task(
        task_id: String,
        module_id: String,
        inputs: serde_json::Value,
        context: Option<Context<serde_json::Value>>,
        executor: Arc<Executor>,
        semaphore: Arc<Semaphore>,
        tasks: Arc<Mutex<HashMap<String, TaskInfo>>>,
    ) {
        // Acquire a permit from the semaphore (limits concurrency).
        let Ok(_permit) = semaphore.acquire().await else {
            // Semaphore closed — treat as cancellation
            let mut guard = tasks.lock();
            if let Some(info) = guard.get_mut(&task_id) {
                info.status = TaskStatus::Cancelled;
                info.completed_at = Some(now_secs());
            }
            return;
        };

        // Mark as running
        {
            let mut guard = tasks.lock();
            if let Some(info) = guard.get_mut(&task_id) {
                // If already cancelled while waiting for permit, bail out
                if info.status == TaskStatus::Cancelled {
                    return;
                }
                info.status = TaskStatus::Running;
                info.started_at = Some(now_secs());
            }
        }

        // Execute the module
        let result = executor
            .call(&module_id, inputs, context.as_ref(), None)
            .await;

        // Update task status based on result
        let mut guard = tasks.lock();
        if let Some(info) = guard.get_mut(&task_id) {
            // Don't overwrite a cancellation that happened during execution
            if info.status == TaskStatus::Cancelled {
                return;
            }

            match result {
                Ok(output) => {
                    info.status = TaskStatus::Completed;
                    info.completed_at = Some(now_secs());
                    info.result = Some(output);
                }
                Err(err) => {
                    info.status = TaskStatus::Failed;
                    info.completed_at = Some(now_secs());
                    info.error = Some(err.to_string());
                    error!("Task {} failed: {}", task_id, err);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Executor;
    use crate::registry::registry::Registry;
    use std::sync::Arc;

    fn make_executor() -> Arc<Executor> {
        let registry = Arc::new(Registry::default());
        let config = Arc::new(crate::config::Config::default());
        Arc::new(Executor::new(registry, config))
    }

    #[test]
    fn new_creates_empty_task_list() {
        let exec = make_executor();
        let mgr = AsyncTaskManager::new(exec, 4, 100);
        assert_eq!(mgr.task_count(), 0);
        assert!(mgr.list_tasks(None).is_empty());
    }

    #[test]
    fn get_status_returns_none_for_unknown_task() {
        let exec = make_executor();
        let mgr = AsyncTaskManager::new(exec, 4, 100);
        assert!(mgr.get_status("nonexistent-task-id").is_none());
    }

    #[test]
    fn get_result_errors_for_unknown_task() {
        let exec = make_executor();
        let mgr = AsyncTaskManager::new(exec, 4, 100);
        assert!(mgr.get_result("nonexistent-task-id").is_err());
    }

    #[test]
    fn cancel_returns_false_for_unknown_task() {
        let exec = make_executor();
        let mgr = AsyncTaskManager::new(exec, 4, 100);
        assert!(!mgr.cancel("nonexistent-task-id"));
    }

    #[tokio::test]
    async fn submit_returns_task_id_and_records_pending() {
        let exec = make_executor();
        let mgr = AsyncTaskManager::new(exec, 4, 100);
        let task_id = mgr
            .submit("some.module", serde_json::json!({}), None)
            .expect("submit should succeed");
        assert!(!task_id.is_empty());
        // Task should be tracked (may have transitioned to Running/Failed by now)
        assert!(mgr.get_status(&task_id).is_some());
    }

    #[tokio::test]
    async fn submit_rejected_when_at_capacity() {
        let exec = make_executor();
        let mgr = AsyncTaskManager::new(exec, 4, 2); // max 2 tasks
                                                     // Spawn 2 tasks to fill the limit
        let _ = mgr.submit("a.module", serde_json::json!({}), None);
        let _ = mgr.submit("b.module", serde_json::json!({}), None);
        // Third submit should fail
        let result = mgr.submit("c.module", serde_json::json!({}), None);
        assert!(result.is_err(), "Should reject when task limit is reached");
    }

    #[tokio::test]
    async fn list_tasks_filtered_by_status() {
        let exec = make_executor();
        let mgr = AsyncTaskManager::new(exec, 0, 100); // max_concurrent=0 keeps tasks pending
                                                       // Submit a task; with 0 concurrency slots it stays Pending until the semaphore opens
        let _ = mgr.submit("some.module", serde_json::json!({}), None);
        // list_tasks(Some(Pending)) should contain it; other statuses should be empty
        let completed = mgr.list_tasks(Some(TaskStatus::Completed));
        let cancelled = mgr.list_tasks(Some(TaskStatus::Cancelled));
        // The task was submitted; it may be Pending or Running depending on scheduling,
        // but it should NOT be Completed or Cancelled yet
        assert!(completed.is_empty(), "no completed tasks yet");
        assert!(cancelled.is_empty(), "no cancelled tasks yet");
    }

    #[tokio::test]
    async fn cancel_changes_status_to_cancelled() {
        let exec = make_executor();
        let mgr = AsyncTaskManager::new(Arc::clone(&exec), 0, 100); // 0 concurrency — tasks stay Pending
        let task_id = mgr
            .submit("some.module", serde_json::json!({}), None)
            .unwrap();
        let cancelled = mgr.cancel(&task_id);
        assert!(cancelled, "cancel should return true for a Pending task");
        let info = mgr.get_status(&task_id).expect("task should still exist");
        assert_eq!(info.status, TaskStatus::Cancelled);
    }

    #[tokio::test]
    async fn cleanup_removes_terminal_tasks_past_max_age() {
        let exec = make_executor();
        let mgr = AsyncTaskManager::new(exec, 0, 100);
        let task_id = mgr.submit("m", serde_json::json!({}), None).unwrap();
        // Cancel it so it reaches a terminal state
        mgr.cancel(&task_id);
        // Cleanup with max_age = -1 (everything is "old enough")
        let removed = mgr.cleanup(-1.0);
        assert_eq!(removed, 1, "one terminal task should be removed");
        assert!(mgr.get_status(&task_id).is_none(), "task should be gone");
    }

    #[tokio::test]
    async fn cleanup_keeps_tasks_within_max_age() {
        let exec = make_executor();
        let mgr = AsyncTaskManager::new(exec, 0, 100);
        let task_id = mgr.submit("m", serde_json::json!({}), None).unwrap();
        mgr.cancel(&task_id);
        // Cleanup with very large max_age — nothing should be removed
        let removed = mgr.cleanup(9_999_999.0);
        assert_eq!(removed, 0, "task within max_age should not be removed");
        assert!(
            mgr.get_status(&task_id).is_some(),
            "task should still exist"
        );
    }

    #[tokio::test]
    async fn shutdown_cancels_all_pending_tasks() {
        let exec = make_executor();
        let mgr = AsyncTaskManager::new(exec, 0, 100); // 0 concurrency keeps tasks Pending
        let id1 = mgr.submit("m1", serde_json::json!({}), None).unwrap();
        let id2 = mgr.submit("m2", serde_json::json!({}), None).unwrap();
        mgr.shutdown();
        let s1 = mgr.get_status(&id1).unwrap().status;
        let s2 = mgr.get_status(&id2).unwrap().status;
        assert_eq!(s1, TaskStatus::Cancelled);
        assert_eq!(s2, TaskStatus::Cancelled);
    }

    #[tokio::test]
    async fn submit_respects_max_tasks_under_concurrent_load() {
        // Regression: a TOCTOU between the capacity check and the insert in
        // submit() allowed two concurrent callers to both observe
        // task_count < max_tasks and both insert, exceeding the cap.
        let exec = make_executor();
        let mgr = Arc::new(AsyncTaskManager::new(exec, 4, 1));

        let mgr_a = Arc::clone(&mgr);
        let mgr_b = Arc::clone(&mgr);

        // Spawn two tasks concurrently targeting a max_tasks=1 manager.
        let (res_a, res_b) = tokio::join!(
            tokio::task::spawn_blocking(move || {
                mgr_a.submit("nonexistent.module", serde_json::json!({}), None)
            }),
            tokio::task::spawn_blocking(move || {
                mgr_b.submit("nonexistent.module", serde_json::json!({}), None)
            }),
        );

        let ok_count = [res_a.unwrap(), res_b.unwrap()]
            .iter()
            .filter(|r| r.is_ok())
            .count();

        assert_eq!(
            ok_count, 1,
            "exactly one submit must succeed when max_tasks=1 and two concurrent submits race"
        );
        assert!(
            mgr.task_count() <= 1,
            "task count must never exceed max_tasks after concurrent submits"
        );
    }

    #[test]
    fn task_info_serializes_and_deserializes() {
        let info = TaskInfo {
            task_id: "abc".to_string(),
            module_id: "m.foo".to_string(),
            status: TaskStatus::Completed,
            submitted_at: 1_000_000.0,
            started_at: Some(1_000_001.0),
            completed_at: Some(1_000_002.0),
            result: Some(serde_json::json!({"x": 1})),
            error: None,
        };
        let json = serde_json::to_string(&info).expect("serialization should succeed");
        let restored: TaskInfo =
            serde_json::from_str(&json).expect("deserialization should succeed");
        assert_eq!(restored.task_id, "abc");
        assert_eq!(restored.status, TaskStatus::Completed);
        assert_eq!(restored.result, Some(serde_json::json!({"x": 1})));
    }
}
