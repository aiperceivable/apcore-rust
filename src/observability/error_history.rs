// APCore Protocol — Error history tracking
// Spec reference: Error recording and history middleware

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::context::Context;
use crate::errors::ModuleError;
use crate::middleware::base::Middleware;

/// A recorded error entry with deduplication support.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEntry {
    pub module_id: String,
    pub error_code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_guidance: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub count: u64,
    pub first_occurred: DateTime<Utc>,
    pub last_occurred: DateTime<Utc>,
}

/// Stores a history of errors for diagnostics.
#[derive(Debug, Clone)]
pub struct ErrorHistory {
    entries: Arc<Mutex<HashMap<String, VecDeque<ErrorEntry>>>>,
    max_entries_per_module: usize,
    max_total_entries: usize,
}

impl ErrorHistory {
    /// Create a new error history with the given capacity per module.
    pub fn new(max_entries_per_module: usize) -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            max_entries_per_module,
            max_total_entries: max_entries_per_module * 100,
        }
    }

    /// Create with explicit per-module and total limits.
    pub fn with_limits(max_entries_per_module: usize, max_total_entries: usize) -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            max_entries_per_module,
            max_total_entries,
        }
    }

    /// Record an error for a module. Deduplicates by (error_code, message).
    pub fn record(&self, module_id: &str, error: &ModuleError) {
        let mut map = self.entries.lock().unwrap();
        let error_code = format!("{:?}", error.code);
        let now = Utc::now();

        let module_entries = map.entry(module_id.to_string()).or_default();

        // Check for existing entry with same code and message (deduplication)
        let existing = module_entries
            .iter_mut()
            .find(|e| e.error_code == error_code && e.message == error.message);

        if let Some(entry) = existing {
            entry.count += 1;
            entry.last_occurred = now;
            entry.timestamp = now;
        } else {
            let entry = ErrorEntry {
                module_id: module_id.to_string(),
                error_code,
                message: error.message.clone(),
                ai_guidance: error.ai_guidance.clone(),
                timestamp: now,
                count: 1,
                first_occurred: now,
                last_occurred: now,
            };
            module_entries.push_back(entry);

            // Evict per-module if over limit
            while module_entries.len() > self.max_entries_per_module {
                module_entries.pop_front();
            }
        }

        // Evict total entries if over limit
        let mut total: usize = map.values().map(|v| v.len()).sum();
        while total > self.max_total_entries {
            // Find the module with the oldest entry and remove it
            let mut oldest_module = None;
            let mut oldest_time = None;
            for (mid, entries) in map.iter() {
                if let Some(front) = entries.front() {
                    if oldest_time.is_none() || front.first_occurred < oldest_time.unwrap() {
                        oldest_time = Some(front.first_occurred);
                        oldest_module = Some(mid.clone());
                    }
                }
            }
            if let Some(mid) = oldest_module {
                if let Some(entries) = map.get_mut(&mid) {
                    entries.pop_front();
                    if entries.is_empty() {
                        map.remove(&mid);
                    }
                }
                total -= 1;
            } else {
                break;
            }
        }
    }

    /// Get errors for a specific module, newest first.
    pub fn get(&self, module_id: &str, limit: Option<usize>) -> Vec<ErrorEntry> {
        let map = self.entries.lock().unwrap();
        match map.get(module_id) {
            Some(entries) => {
                let mut result: Vec<ErrorEntry> = entries.iter().cloned().collect();
                result.sort_by(|a, b| b.last_occurred.cmp(&a.last_occurred));
                if let Some(lim) = limit {
                    result.truncate(lim);
                }
                result
            }
            None => Vec::new(),
        }
    }

    /// Get all recorded errors across all modules, sorted by last_occurred desc.
    pub fn get_all(&self, limit: Option<usize>) -> Vec<ErrorEntry> {
        let map = self.entries.lock().unwrap();
        let mut all: Vec<ErrorEntry> = map
            .values()
            .flat_map(|entries| entries.iter().cloned())
            .collect();
        all.sort_by(|a, b| b.last_occurred.cmp(&a.last_occurred));
        if let Some(lim) = limit {
            all.truncate(lim);
        }
        all
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

    /// Get a reference to the underlying error history.
    pub fn history(&self) -> &ErrorHistory {
        &self.history
    }
}

#[async_trait]
impl Middleware for ErrorHistoryMiddleware {
    fn name(&self) -> &str {
        "error_history"
    }

    async fn before(
        &self,
        _module_id: &str,
        _inputs: serde_json::Value,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        Ok(None)
    }

    async fn after(
        &self,
        _module_id: &str,
        _inputs: serde_json::Value,
        _output: serde_json::Value,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        Ok(None)
    }

    async fn on_error(
        &self,
        module_id: &str,
        _inputs: serde_json::Value,
        error: &ModuleError,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        self.history.record(module_id, error);
        Ok(None)
    }
}
