// APCore Protocol — Error history tracking
// Spec reference: Error recording and history middleware

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::context::Context;
use crate::errors::{ErrorCode, ModuleError};
use crate::middleware::base::Middleware;

/// A recorded error entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEntry {
    pub timestamp: DateTime<Utc>,
    pub module_id: String,
    pub error_code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

/// Stores a history of errors for diagnostics.
#[derive(Debug, Clone)]
pub struct ErrorHistory {
    entries: Arc<Mutex<HashMap<String, Vec<ErrorEntry>>>>,
    max_entries: usize,
}

impl ErrorHistory {
    /// Create a new error history with the given capacity per module.
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            max_entries,
        }
    }

    /// Record an error for a module.
    pub fn record(&self, module_id: &str, error: &ModuleError) {
        // TODO: Implement — enforce max_entries per module
        todo!()
    }

    /// Get all recorded errors across all modules.
    pub fn get_all(&self) -> Vec<ErrorEntry> {
        // TODO: Implement
        todo!()
    }

    /// Get errors for a specific module.
    pub fn get(&self, module_id: &str) -> Vec<ErrorEntry> {
        self.entries
            .lock()
            .unwrap()
            .get(module_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Clear errors. If module_id is Some, clear only that module; otherwise clear all.
    pub fn clear(&self, module_id: Option<&str>) {
        let mut map = self.entries.lock().unwrap();
        match module_id {
            Some(id) => {
                map.remove(id);
            }
            None => map.clear(),
        }
    }
}

/// Middleware that records errors into an ErrorHistory.
#[derive(Debug)]
pub struct ErrorHistoryMiddleware {
    history: ErrorHistory,
}

impl ErrorHistoryMiddleware {
    /// Create a new error history middleware.
    pub fn new(history: ErrorHistory) -> Self {
        Self { history }
    }
}

#[async_trait]
impl Middleware for ErrorHistoryMiddleware {
    fn name(&self) -> &str {
        "error_history"
    }

    async fn before(
        &self,
        _ctx: &Context<serde_json::Value>,
        _module_name: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError> {
        Ok(input)
    }

    async fn after(
        &self,
        _ctx: &Context<serde_json::Value>,
        _module_name: &str,
        _inputs: serde_json::Value,
        output: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError> {
        Ok(output)
    }

    async fn on_error(
        &self,
        _ctx: &Context<serde_json::Value>,
        _module_name: &str,
        _inputs: serde_json::Value,
        _error: &ModuleError,
    ) -> Result<(), ModuleError> {
        // TODO: Implement — record error into history
        todo!()
    }
}
