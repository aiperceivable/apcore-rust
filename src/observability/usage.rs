// APCore Protocol — Usage tracking
// Spec reference: Module usage statistics and middleware

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::context::Context;
use crate::errors::ModuleError;
use crate::middleware::base::Middleware;

/// Usage statistics for a single module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageStats {
    pub module_name: String,
    pub total_calls: u64,
    pub success_count: u64,
    pub error_count: u64,
    pub total_duration_ms: u64,
    pub avg_duration_ms: f64,
}

/// Collects usage statistics across module executions.
#[derive(Debug, Clone)]
pub struct UsageCollector {
    stats: Arc<Mutex<HashMap<String, UsageStats>>>,
}

impl UsageCollector {
    /// Create a new usage collector.
    pub fn new() -> Self {
        Self {
            stats: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Record a successful execution.
    pub fn record_success(&self, module_name: &str, duration_ms: u64) {
        // TODO: Implement
        todo!()
    }

    /// Record a failed execution.
    pub fn record_error(&self, module_name: &str, duration_ms: u64) {
        // TODO: Implement
        todo!()
    }

    /// Get usage stats for a module.
    pub fn get_stats(&self, module_name: &str) -> Option<UsageStats> {
        self.stats.lock().unwrap().get(module_name).cloned()
    }

    /// Get all usage stats.
    pub fn get_all_stats(&self) -> Vec<UsageStats> {
        self.stats.lock().unwrap().values().cloned().collect()
    }

    /// Reset all stats.
    pub fn reset(&self) {
        self.stats.lock().unwrap().clear();
    }
}

impl Default for UsageCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Middleware that tracks usage statistics.
#[derive(Debug)]
pub struct UsageMiddleware {
    collector: UsageCollector,
}

impl UsageMiddleware {
    /// Create a new usage middleware.
    pub fn new(collector: UsageCollector) -> Self {
        Self { collector }
    }
}

#[async_trait]
impl Middleware for UsageMiddleware {
    fn name(&self) -> &str {
        "usage"
    }

    async fn before(
        &self,
        _ctx: &Context<serde_json::Value>,
        _module_name: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError> {
        // TODO: Implement — record start time
        todo!()
    }

    async fn after(
        &self,
        _ctx: &Context<serde_json::Value>,
        _module_name: &str,
        output: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError> {
        // TODO: Implement — record success
        todo!()
    }

    async fn on_error(
        &self,
        _ctx: &Context<serde_json::Value>,
        _module_name: &str,
        _error: &ModuleError,
    ) -> Result<(), ModuleError> {
        // TODO: Implement — record error
        todo!()
    }
}
