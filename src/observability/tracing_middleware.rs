// APCore Protocol — Tracing middleware
// Spec reference: Automatic span creation around module execution

use async_trait::async_trait;

use crate::context::Context;
use crate::errors::ModuleError;
use crate::middleware::base::Middleware;
use super::span::SpanExporter;

/// Strategy for deciding which spans to sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamplingStrategy {
    /// Sample all spans.
    Always,
    /// Sample spans at the configured rate.
    Probabilistic,
    /// Never sample spans.
    Never,
}

/// Middleware that creates tracing spans around module execution.
#[derive(Debug)]
pub struct TracingMiddleware {
    exporter: Box<dyn SpanExporter>,
    pub sampling_strategy: SamplingStrategy,
    pub sampling_rate: f64,
}

impl TracingMiddleware {
    /// Create a new tracing middleware with the given exporter.
    pub fn new(exporter: Box<dyn SpanExporter>) -> Self {
        Self {
            exporter,
            sampling_strategy: SamplingStrategy::Always,
            sampling_rate: 1.0,
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
            sampling_rate: rate,
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
        _ctx: &Context<serde_json::Value>,
        _module_name: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError> {
        // TODO: Implement — create and start a span
        todo!()
    }

    async fn after(
        &self,
        _ctx: &Context<serde_json::Value>,
        _module_name: &str,
        _inputs: serde_json::Value,
        output: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError> {
        // TODO: Implement — end span, export
        todo!()
    }

    async fn on_error(
        &self,
        _ctx: &Context<serde_json::Value>,
        _module_name: &str,
        _inputs: serde_json::Value,
        _error: &ModuleError,
    ) -> Result<(), ModuleError> {
        // TODO: Implement — record error on span, export
        todo!()
    }
}
