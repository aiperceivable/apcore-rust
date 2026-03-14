// APCore Protocol — Tracing middleware
// Spec reference: Automatic span creation around module execution

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

use super::span::{Span, SpanExporter, SpanStatus};
use crate::context::Context;
use crate::errors::ModuleError;
use crate::middleware::base::Middleware;

/// Strategy for deciding which spans to sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamplingStrategy {
    /// Sample all spans (Python: "full").
    Always,
    /// Sample spans at the configured rate (Python: "proportional").
    Probabilistic,
    /// Sample errors always, success at rate (Python: "error_first").
    ErrorFirst,
    /// Never sample spans (Python: "off").
    Never,
}

/// Middleware that creates tracing spans around module execution.
///
/// WARNING: The internal span stack is not safe for concurrent use on
/// the same middleware instance. Use separate instances per concurrent pipeline.
#[derive(Debug)]
pub struct TracingMiddleware {
    exporter: Box<dyn SpanExporter>,
    pub sampling_strategy: SamplingStrategy,
    pub sampling_rate: f64,
    spans: Mutex<HashMap<String, Span>>,
}

impl TracingMiddleware {
    /// Create a new tracing middleware with the given exporter (Always sampling).
    pub fn new(exporter: Box<dyn SpanExporter>) -> Self {
        Self {
            exporter,
            sampling_strategy: SamplingStrategy::Always,
            sampling_rate: 1.0,
            spans: Mutex::new(HashMap::new()),
        }
    }

    /// Create with explicit sampling configuration.
    pub fn with_sampling(
        exporter: Box<dyn SpanExporter>,
        strategy: SamplingStrategy,
        rate: f64,
    ) -> Self {
        Self {
            exporter,
            sampling_strategy: strategy,
            sampling_rate: rate.clamp(0.0, 1.0),
            spans: Mutex::new(HashMap::new()),
        }
    }

    /// Determine if this request should be sampled.
    fn should_sample(&self, ctx: &Context<serde_json::Value>) -> bool {
        // If parent context has a sampling decision, inherit it
        if let Some(sampled) = ctx.data.get("_apcore.tracing.sampled") {
            if let Some(b) = sampled.as_bool() {
                return b;
            }
        }

        match self.sampling_strategy {
            SamplingStrategy::Always => true,
            SamplingStrategy::Never => false,
            SamplingStrategy::Probabilistic | SamplingStrategy::ErrorFirst => {
                // Use a simple random check based on rate
                // Since we don't have `rand`, use uuid's randomness
                let random_val = uuid::Uuid::new_v4().as_u128() as f64 / u128::MAX as f64;
                random_val < self.sampling_rate
            }
        }
    }
}

#[async_trait]
impl Middleware for TracingMiddleware {
    fn name(&self) -> &str {
        "tracing"
    }

    async fn before(
        &self,
        module_id: &str,
        _inputs: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        let mut span = Span::new(module_id, &ctx.trace_id);

        // Set parent span id if there's an active span in context
        if let Some(parent_span_id) = ctx.data.get("_apcore.tracing.parent_span_id") {
            if let Some(pid) = parent_span_id.as_str() {
                span.parent_span_id = Some(pid.to_string());
            }
        }

        span.set_attribute("module.name".to_string(), serde_json::json!(module_id));
        if let Some(ref caller_id) = ctx.caller_id {
            span.set_attribute("caller.id".to_string(), serde_json::json!(caller_id));
        }

        // Store span keyed by trace_id for concurrency safety
        {
            let mut spans = self.spans.lock().unwrap_or_else(|e| e.into_inner());
            spans.insert(ctx.trace_id.clone(), span);
        }

        Ok(None)
    }

    async fn after(
        &self,
        _module_id: &str,
        _inputs: serde_json::Value,
        _output: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        let span = {
            let mut spans = self.spans.lock().unwrap_or_else(|e| e.into_inner());
            spans.remove(&ctx.trace_id)
        };

        if let Some(mut span) = span {
            span.status = SpanStatus::Ok;
            span.end();

            if self.should_sample(ctx) {
                let _ = self.exporter.export(&span).await;
            }
        }

        Ok(None)
    }

    async fn on_error(
        &self,
        _module_id: &str,
        _inputs: serde_json::Value,
        error: &ModuleError,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        let span = {
            let mut spans = self.spans.lock().unwrap_or_else(|e| e.into_inner());
            spans.remove(&ctx.trace_id)
        };

        if let Some(mut span) = span {
            span.status = SpanStatus::Error;
            span.set_attribute(
                "error.message".to_string(),
                serde_json::json!(error.message),
            );
            span.set_attribute(
                "error.code".to_string(),
                serde_json::json!(format!("{:?}", error.code)),
            );
            span.add_event("exception");
            span.end();

            // For ErrorFirst strategy, always export errors regardless of sampling decision
            let should_export = match self.sampling_strategy {
                SamplingStrategy::ErrorFirst => true,
                _ => self.should_sample(ctx),
            };

            if should_export {
                let _ = self.exporter.export(&span).await;
            }
        }

        Ok(None)
    }
}
