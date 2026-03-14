// APCore Protocol — Retry middleware
// Spec reference: Automatic retry with configurable backoff

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use super::base::Middleware;
use crate::context::Context;
use crate::errors::ModuleError;

/// Configuration for retry behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    pub max_retries: u32,
    /// Backoff strategy: "exponential" or "fixed".
    #[serde(default = "default_strategy")]
    pub strategy: String,
    /// Base delay in milliseconds.
    #[serde(default = "default_base_delay")]
    pub base_delay_ms: u64,
    /// Maximum delay in milliseconds (cap for exponential backoff).
    #[serde(default = "default_max_delay")]
    pub max_delay_ms: u64,
    /// Whether to add random jitter to delays.
    #[serde(default = "default_jitter")]
    pub jitter: bool,
}

fn default_strategy() -> String {
    "exponential".to_string()
}
fn default_base_delay() -> u64 {
    100
}
fn default_max_delay() -> u64 {
    5000
}
fn default_jitter() -> bool {
    true
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            strategy: "exponential".to_string(),
            base_delay_ms: 100,
            max_delay_ms: 5000,
            jitter: true,
        }
    }
}

/// Middleware that retries failed executions according to RetryConfig.
///
/// When `on_error` is called with a retryable error (`error.retryable == Some(true)`),
/// this middleware sleeps for a calculated delay and returns `Some(inputs)` to signal
/// the pipeline to retry execution. After `max_retries` attempts or for non-retryable
/// errors, it returns `None` so the error propagates.
///
/// Retry state is tracked per-module internally via a `Mutex<HashMap>`
/// within the middleware instance.
#[derive(Debug)]
pub struct RetryMiddleware {
    pub config: RetryConfig,
    retry_counts: Mutex<HashMap<String, u32>>,
}

impl RetryMiddleware {
    /// Create a new retry middleware with the given config.
    pub fn new(config: RetryConfig) -> Self {
        Self {
            config,
            retry_counts: Mutex::new(HashMap::new()),
        }
    }

    /// Calculate delay in milliseconds for the given attempt number.
    ///
    /// - Exponential: `base_delay_ms * 2^attempt`, capped at `max_delay_ms`.
    /// - Fixed: `base_delay_ms`.
    /// - With jitter: multiply by a factor in [0.5, 1.5).
    fn calculate_delay(&self, attempt: u32) -> f64 {
        let delay = if self.config.strategy == "fixed" {
            self.config.base_delay_ms as f64
        } else {
            // Exponential backoff
            let exp_delay = self.config.base_delay_ms as f64 * 2.0_f64.powi(attempt as i32);
            exp_delay.min(self.config.max_delay_ms as f64)
        };

        if self.config.jitter {
            // Simple jitter using system time nanos as pseudo-random source.
            // Produces a factor in [0.5, 1.5) matching Python's random.uniform(0.5, 1.5).
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos();
            let factor = 0.5 + (nanos as f64 % 1_000_000.0) / 1_000_000.0;
            delay * factor
        } else {
            delay
        }
    }
}

#[async_trait]
impl Middleware for RetryMiddleware {
    fn name(&self) -> &str {
        "retry"
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
        module_id: &str,
        _inputs: serde_json::Value,
        _output: serde_json::Value,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        // Reset retry count on successful execution so retries don't persist
        // across separate call sequences.
        self.retry_counts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(module_id);
        Ok(None)
    }

    async fn on_error(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        error: &ModuleError,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        // Only retry if the error is explicitly marked as retryable.
        if error.retryable != Some(true) {
            return Ok(None);
        }

        let retry_count = {
            let counts = self.retry_counts.lock().unwrap_or_else(|e| e.into_inner());
            *counts.get(module_id).unwrap_or(&0)
        };

        if retry_count >= self.config.max_retries {
            tracing::warn!(
                "Max retries ({}) exceeded for module '{}'",
                self.config.max_retries,
                module_id,
            );
            return Ok(None);
        }

        let delay_ms = self.calculate_delay(retry_count);

        tracing::info!(
            "Retrying module '{}' (attempt {}/{}) after {:.0}ms",
            module_id,
            retry_count + 1,
            self.config.max_retries,
            delay_ms,
        );

        // Sleep for the calculated delay.
        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms as u64)).await;

        // Increment the retry count.
        {
            let mut counts = self.retry_counts.lock().unwrap_or_else(|e| e.into_inner());
            *counts.entry(module_id.to_string()).or_insert(0) += 1;
        }

        // Return the original inputs to signal retry.
        Ok(Some(inputs))
    }
}
