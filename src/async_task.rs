// APCore Protocol — Async task management
// Spec reference: Task status tracking and async execution

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::errors::ModuleError;

/// Status of an async task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Information about an async task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: Uuid,
    pub module_name: String,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Manages asynchronous task execution and tracking.
#[derive(Debug)]
pub struct AsyncTaskManager {
    tasks: HashMap<Uuid, TaskInfo>,
}

impl AsyncTaskManager {
    /// Create a new task manager.
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    /// Submit a new async task.
    ///
    /// Creates a task entry with Pending status and returns the UUID.
    /// Note: actual async execution would require an executor reference;
    /// for now this only creates the task record.
    pub async fn submit(
        &mut self,
        module_name: &str,
        _input: serde_json::Value,
    ) -> Result<Uuid, ModuleError> {
        let id = Uuid::new_v4();
        let task = TaskInfo {
            id,
            module_name: module_name.to_string(),
            status: TaskStatus::Pending,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            result: None,
            error: None,
            metadata: HashMap::new(),
        };
        self.tasks.insert(id, task);
        Ok(id)
    }

    /// Get the status of a task by ID.
    pub fn get_task(&self, task_id: &Uuid) -> Option<&TaskInfo> {
        self.tasks.get(task_id)
    }

    /// Cancel a running task.
    pub async fn cancel(&mut self, task_id: &Uuid) -> Result<(), ModuleError> {
        let task = self.tasks.get_mut(task_id).ok_or_else(|| {
            ModuleError::new(
                crate::errors::ErrorCode::ModuleNotFound,
                format!("Task '{}' not found", task_id),
            )
        })?;

        match task.status {
            TaskStatus::Pending | TaskStatus::Running => {
                task.status = TaskStatus::Cancelled;
                task.completed_at = Some(Utc::now());
                Ok(())
            }
            _ => Err(ModuleError::new(
                crate::errors::ErrorCode::GeneralInvalidInput,
                format!(
                    "Task '{}' cannot be cancelled (status: {:?})",
                    task_id, task.status
                ),
            )),
        }
    }

    /// List all tasks, optionally filtered by status.
    pub fn list_tasks(&self, status: Option<TaskStatus>) -> Vec<&TaskInfo> {
        self.tasks
            .values()
            .filter(|task| match status {
                Some(s) => task.status == s,
                None => true,
            })
            .collect()
    }

    /// Wait for a task to complete and return its result.
    pub async fn await_task(&self, task_id: &Uuid) -> Result<serde_json::Value, ModuleError> {
        let task = self.tasks.get(task_id).ok_or_else(|| {
            ModuleError::new(
                crate::errors::ErrorCode::ModuleNotFound,
                format!("Task '{}' not found", task_id),
            )
        })?;

        match task.status {
            TaskStatus::Completed => Ok(task.result.clone().unwrap_or(serde_json::Value::Null)),
            TaskStatus::Failed => Err(ModuleError::new(
                crate::errors::ErrorCode::ModuleExecuteError,
                task.error
                    .clone()
                    .unwrap_or_else(|| format!("Task '{}' failed", task_id)),
            )),
            TaskStatus::Cancelled => Err(ModuleError::new(
                crate::errors::ErrorCode::ExecutionCancelled,
                format!("Task '{}' was cancelled", task_id),
            )),
            _ => Err(ModuleError::new(
                crate::errors::ErrorCode::GeneralInvalidInput,
                format!(
                    "Task '{}' is not yet completed (status: {:?})",
                    task_id, task.status
                ),
            )),
        }
    }
}

impl Default for AsyncTaskManager {
    fn default() -> Self {
        Self::new()
    }
}
