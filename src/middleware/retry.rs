// APCore Protocol — Retry middleware
// Spec reference: Automatic retry with configurable backoff

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;

use super::base::Middleware;
use crate::context::Context;
use crate::errors::ModuleError;

/// Configuration for retry behavior.
///
/// Marked `#[non_exhaustive]` (issue #24) so future spec extensions can add
/// fields without breaking downstream struct-literal construction. Construct
/// via `..Default::default()` or a builder pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
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

/// Middleware that retries failed executions according to `RetryConfig`.
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
    #[must_use]
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
        #[allow(clippy::cast_precision_loss)]
        // intentional: delay values are small (ms) and won't lose meaningful precision
        let delay = if self.config.strategy == "fixed" {
            self.config.base_delay_ms as f64
        } else {
            // Exponential backoff
            #[allow(clippy::cast_possible_wrap)] // attempt won't exceed i32::MAX in practice
            let exp_delay = self.config.base_delay_ms as f64 * 2.0_f64.powi(attempt as i32);
            #[allow(clippy::cast_precision_loss)]
            // intentional: delay values are small (ms) and won't lose meaningful precision
            let max = self.config.max_delay_ms as f64;
            exp_delay.min(max)
        };

        if self.config.jitter {
            // Simple jitter using system time nanos as pseudo-random source.
            // Produces a factor in [0.5, 1.5) matching Python's random.uniform(0.5, 1.5).
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos();
            let factor = 0.5 + (f64::from(nanos) % 1_000_000.0) / 1_000_000.0;
            delay * factor
        } else {
            delay
        }
    }
}

#[async_trait]
impl Middleware for RetryMiddleware {
    fn name(&self) -> &'static str {
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
        self.retry_counts.lock().remove(module_id);
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
            let counts = self.retry_counts.lock();
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
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        // delay_ms is non-negative and won't exceed u64::MAX
        let delay_ms_u64 = delay_ms as u64;
        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms_u64)).await;

        // Increment the retry count.
        {
            let mut counts = self.retry_counts.lock();
            *counts.entry(module_id.to_string()).or_insert(0) += 1;
        }

        // Return the original inputs to signal retry.
        Ok(Some(inputs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::ErrorCode;
    use serde_json::json;

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.strategy, "exponential");
        assert_eq!(config.base_delay_ms, 100);
        assert_eq!(config.max_delay_ms, 5000);
        assert!(config.jitter);
    }

    #[test]
    fn test_retry_config_serde_defaults() {
        let config: RetryConfig = serde_json::from_str(r#"{"max_retries": 5}"#).unwrap();
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.strategy, "exponential");
        assert_eq!(config.base_delay_ms, 100);
    }

    #[test]
    fn test_calculate_delay_fixed_no_jitter() {
        let mw = RetryMiddleware::new(RetryConfig {
            max_retries: 3,
            strategy: "fixed".to_string(),
            base_delay_ms: 200,
            max_delay_ms: 5000,
            jitter: false,
        });
        assert!((mw.calculate_delay(0) - 200.0).abs() < f64::EPSILON);
        assert!((mw.calculate_delay(1) - 200.0).abs() < f64::EPSILON);
        assert!((mw.calculate_delay(5) - 200.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_calculate_delay_exponential_no_jitter() {
        let mw = RetryMiddleware::new(RetryConfig {
            max_retries: 5,
            strategy: "exponential".to_string(),
            base_delay_ms: 100,
            max_delay_ms: 5000,
            jitter: false,
        });
        assert!((mw.calculate_delay(0) - 100.0).abs() < f64::EPSILON);
        assert!((mw.calculate_delay(1) - 200.0).abs() < f64::EPSILON);
        assert!((mw.calculate_delay(2) - 400.0).abs() < f64::EPSILON);
        assert!((mw.calculate_delay(3) - 800.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_calculate_delay_exponential_capped() {
        let mw = RetryMiddleware::new(RetryConfig {
            max_retries: 10,
            strategy: "exponential".to_string(),
            base_delay_ms: 100,
            max_delay_ms: 500,
            jitter: false,
        });
        assert!((mw.calculate_delay(4) - 500.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_calculate_delay_with_jitter_in_range() {
        let mw = RetryMiddleware::new(RetryConfig {
            max_retries: 3,
            strategy: "fixed".to_string(),
            base_delay_ms: 1000,
            max_delay_ms: 5000,
            jitter: true,
        });
        for _ in 0..20 {
            let delay = mw.calculate_delay(0);
            assert!(delay >= 500.0, "delay {delay} should be >= 500");
            assert!(delay < 1500.0, "delay {delay} should be < 1500");
        }
    }

    #[test]
    fn test_retry_middleware_name() {
        let mw = RetryMiddleware::new(RetryConfig::default());
        assert_eq!(mw.name(), "retry");
    }

    #[tokio::test]
    async fn test_before_returns_none() {
        let mw = RetryMiddleware::new(RetryConfig::default());
        let ctx = Context::<serde_json::Value>::anonymous();
        let result = mw.before("test.mod", json!({"a": 1}), &ctx).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_after_resets_retry_count() {
        let mw = RetryMiddleware::new(RetryConfig::default());
        mw.retry_counts.lock().insert("mod.a".to_string(), 2);
        let ctx = Context::<serde_json::Value>::anonymous();
        mw.after("mod.a", json!({}), json!({}), &ctx).await.unwrap();
        assert!(mw.retry_counts.lock().get("mod.a").is_none());
    }

    #[tokio::test]
    async fn test_on_error_non_retryable_returns_none() {
        let mw = RetryMiddleware::new(RetryConfig {
            max_retries: 3,
            strategy: "fixed".to_string(),
            base_delay_ms: 1,
            max_delay_ms: 1,
            jitter: false,
        });
        let ctx = Context::<serde_json::Value>::anonymous();
        let err = ModuleError::new(ErrorCode::ModuleExecuteError, "fail");
        let result = mw.on_error("mod.a", json!({}), &err, &ctx).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_on_error_retryable_returns_inputs() {
        let mw = RetryMiddleware::new(RetryConfig {
            max_retries: 3,
            strategy: "fixed".to_string(),
            base_delay_ms: 1,
            max_delay_ms: 1,
            jitter: false,
        });
        let ctx = Context::<serde_json::Value>::anonymous();
        let err = ModuleError::new(ErrorCode::ModuleExecuteError, "fail").with_retryable(true);
        let inputs = json!({"key": "val"});
        let result = mw
            .on_error("mod.a", inputs.clone(), &err, &ctx)
            .await
            .unwrap();
        assert_eq!(result, Some(inputs));
        assert_eq!(*mw.retry_counts.lock().get("mod.a").unwrap(), 1);
    }

    #[tokio::test]
    async fn test_on_error_exceeds_max_retries() {
        let mw = RetryMiddleware::new(RetryConfig {
            max_retries: 2,
            strategy: "fixed".to_string(),
            base_delay_ms: 1,
            max_delay_ms: 1,
            jitter: false,
        });
        mw.retry_counts.lock().insert("mod.a".to_string(), 2);
        let ctx = Context::<serde_json::Value>::anonymous();
        let err = ModuleError::new(ErrorCode::ModuleExecuteError, "fail").with_retryable(true);
        let result = mw.on_error("mod.a", json!({}), &err, &ctx).await.unwrap();
        assert_eq!(result, None);
    }
}
