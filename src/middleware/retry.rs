// APCore Protocol — Retry middleware
// Spec reference: Automatic retry with configurable backoff

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::context::Context;
use crate::errors::ModuleError;
use super::base::Middleware;

/// Configuration for retry behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_multiplier: f64,
    #[serde(default)]
    pub retry_on_codes: Vec<crate::errors::ErrorCode>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay_ms: 100,
            max_delay_ms: 5000,
            backoff_multiplier: 2.0,
            retry_on_codes: vec![],
        }
    }
}

/// Middleware that retries failed executions according to RetryConfig.
#[derive(Debug)]
pub struct RetryMiddleware {
    pub config: RetryConfig,
}

impl RetryMiddleware {
    /// Create a new retry middleware with the given config.
    pub fn new(config: RetryConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Middleware for RetryMiddleware {
    fn name(&self) -> &str {
        "retry"
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
        // TODO: Implement — check if retryable, apply backoff
        todo!()
    }
}
