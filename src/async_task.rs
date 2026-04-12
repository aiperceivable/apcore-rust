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
        let task_count = self.tasks.lock().len();
        if task_count >= self.max_tasks {
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

        self.tasks.lock().insert(task_id.clone(), info);

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
