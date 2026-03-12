// APCore Protocol — Error history tracking
// Spec reference: Error recording and history middleware

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

use crate::context::Context;
use crate::errors::{ErrorCode, ModuleError};
use crate::middleware::base::Middleware;

/// A recorded error entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEntry {
    pub timestamp: DateTime<Utc>,
    pub module_name: String,
    pub error_code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
}

/// Stores a history of errors for diagnostics.
#[derive(Debug, Clone)]
pub struct ErrorHistory {
    entries: Arc<Mutex<Vec<ErrorEntry>>>,
    max_entries: usize,
}

impl ErrorHistory {
    /// Create a new error history with the given capacity.
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Arc::new(Mutex::new(vec![])),
            max_entries,
        }
    }

    /// Record an error.
    pub fn record(&self, entry: ErrorEntry) {
        // TODO: Implement — enforce max_entries
        todo!()
    }

    /// Get all recorded errors.
    pub fn get_entries(&self) -> Vec<ErrorEntry> {
        self.entries.lock().unwrap().clone()
    }

    /// Get errors filtered by error code.
    pub fn get_by_code(&self, code: ErrorCode) -> Vec<ErrorEntry> {
        // TODO: Implement
        todo!()
    }

    /// Clear the history.
    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
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
        output: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError> {
        Ok(output)
    }

    async fn on_error(
        &self,
        _ctx: &Context<serde_json::Value>,
        _module_name: &str,
        _error: &ModuleError,
    ) -> Result<(), ModuleError> {
        // TODO: Implement — record error into history
        todo!()
    }
}
