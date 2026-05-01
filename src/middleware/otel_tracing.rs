// APCore Protocol — TracingMiddleware (OpenTelemetry-compatible, Issue #42)
// Spec reference: middleware-system.md §1.3 TracingMiddleware
//
// Creates a logical span around each module call:
//   - span name           = module_id
//   - span attributes     = { apcore.trace_id, apcore.caller_id, apcore.module_id }
//   - context.data write  = _apcore.mw.tracing.span_id
//
// Compile-time feature `opentelemetry` controls whether the middleware is
// active by default. When the feature is disabled the middleware behaves as
// a no-op (does not write to context.data and does not raise). The runtime
// `enabled` override on the builder bypasses the feature flag for tests and
// for runtime probes that want to disable tracing without recompiling.
//
// Note: this implementation does not pull in the heavyweight
// `opentelemetry-sdk`/exporter crates. It produces deterministic UUID-based
// span ids and stores the attribute set in context.data so downstream code,
// adapters, and conformance tests can verify span lifecycle without an
// actual OTLP backend wired up. A future change can layer real tracer
// integration on top of this scaffold without breaking the public surface.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::base::Middleware;
use super::context_namespace::{
    enforce_context_key, namespace_keys::TRACING_SPAN_ID, ContextWriter,
};
use crate::context::Context;
use crate::errors::ModuleError;

/// Auxiliary context-data key for the recorded span attribute set. Useful for
/// adapters, integration tests, and the cross-language conformance suite —
/// not part of the canonical spec key list.
pub const TRACING_ATTRIBUTES_KEY: &str = "_apcore.mw.tracing.attributes";
/// Auxiliary context-data key for the recorded span name (== module_id).
pub const TRACING_SPAN_NAME_KEY: &str = "_apcore.mw.tracing.span_name";
/// Auxiliary context-data key for the span lifecycle status, written by
/// `after()` (`"ok"`) and `on_error()` (`"error"`).
pub const TRACING_SPAN_STATUS_KEY: &str = "_apcore.mw.tracing.span_status";

/// Configuration for [`TracingMiddleware`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracingConfig {
    /// `service.name` resource attribute reported with each span.
    #[serde(default = "default_service_name")]
    pub service_name: String,
    /// Whether to inject a W3C `traceparent` header on outbound calls.
    ///
    /// **Note:** the field is parsed and stored, but the middleware itself
    /// has no outbound-call hook to attach the header to — propagation must
    /// happen at the adapter or executor layer that actually emits requests.
    /// Setting this on its own does not change runtime behaviour today.
    #[serde(default = "default_propagate")]
    pub propagate_traceparent: bool,
    /// Middleware ordering priority (higher runs first).
    #[serde(default = "default_priority")]
    pub priority: u16,
    /// Runtime override for the compile-time `opentelemetry` feature. When
    /// `Some(true)`, the middleware is active even if the feature is off.
    /// When `Some(false)`, the middleware is a no-op even if the feature is
    /// on. When `None`, behaviour follows the feature flag.
    #[serde(default)]
    pub enabled: Option<bool>,
}

fn default_service_name() -> String {
    "apcore".to_string()
}
fn default_propagate() -> bool {
    true
}
fn default_priority() -> u16 {
    800
}

impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            service_name: default_service_name(),
            propagate_traceparent: default_propagate(),
            priority: default_priority(),
            enabled: None,
        }
    }
}

/// Builder for [`TracingMiddleware`].
#[derive(Debug, Default)]
pub struct TracingBuilder {
    config: TracingConfig,
}

impl TracingBuilder {
    #[must_use]
    pub fn service_name(mut self, name: impl Into<String>) -> Self {
        self.config.service_name = name.into();
        self
    }

    #[must_use]
    pub fn propagate_traceparent(mut self, value: bool) -> Self {
        self.config.propagate_traceparent = value;
        self
    }

    #[must_use]
    pub fn priority(mut self, value: u16) -> Self {
        self.config.priority = value;
        self
    }

    /// Force the middleware on or off, bypassing the compile-time
    /// `opentelemetry` feature flag.
    #[must_use]
    pub fn enabled(mut self, value: bool) -> Self {
        self.config.enabled = Some(value);
        self
    }

    #[must_use]
    pub fn build(self) -> TracingMiddleware {
        TracingMiddleware::with_config(self.config)
    }
}

/// OpenTelemetry-compatible tracing middleware.
#[derive(Debug)]
pub struct TracingMiddleware {
    config: TracingConfig,
}

impl TracingMiddleware {
    /// Builder for the most common construction path.
    #[must_use]
    pub fn builder() -> TracingBuilder {
        TracingBuilder::default()
    }

    /// Construct the middleware with explicit config. The runtime `enabled`
    /// override on the config takes precedence over the compile-time feature.
    #[must_use]
    pub fn with_config(config: TracingConfig) -> Self {
        Self { config }
    }

    /// Whether the middleware is currently active. Reflects the runtime
    /// `enabled` override if set, otherwise the compile-time `opentelemetry`
    /// feature flag.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        if let Some(v) = self.config.enabled {
            return v;
        }
        cfg!(feature = "opentelemetry")
    }

    fn caller_of(ctx: &Context<Value>) -> String {
        ctx.caller_id.clone().unwrap_or_default()
    }

    fn build_attributes(module_id: &str, ctx: &Context<Value>) -> Value {
        serde_json::json!({
            "apcore.trace_id": ctx.trace_id,
            "apcore.caller_id": Self::caller_of(ctx),
            "apcore.module_id": module_id,
        })
    }

    fn write_key(ctx: &Context<Value>, key: &str, value: Value) {
        let _ = enforce_context_key(ContextWriter::Framework, key);
        let mut data = ctx.data.write();
        data.insert(key.to_string(), value);
    }
}

#[async_trait]
impl Middleware for TracingMiddleware {
    fn name(&self) -> &'static str {
        "tracing"
    }

    fn priority(&self) -> u16 {
        self.config.priority
    }

    async fn before(
        &self,
        module_id: &str,
        _inputs: Value,
        ctx: &Context<Value>,
    ) -> Result<Option<Value>, ModuleError> {
        if !self.is_enabled() {
            return Ok(None);
        }

        // 32-char lowercase hex span id, aligned with W3C span-id width
        // (which is 16 chars) — we use the full UUID hex to remain unique
        // across the rolling window.
        let span_id = uuid::Uuid::new_v4().simple().to_string();
        let attributes = Self::build_attributes(module_id, ctx);

        // Plain structured log line — a `tracing::info_span!` here would
        // create a span object that is dropped without entering, so it would
        // not actually scope downstream events. Real OTel exporter
        // integration (when the `opentelemetry` feature is layered up to a
        // full SDK) will replace this with a proper `Tracer::start` call.
        tracing::debug!(
            service_name = %self.config.service_name,
            apcore.trace_id = %ctx.trace_id,
            apcore.caller_id = %Self::caller_of(ctx),
            apcore.module_id = %module_id,
            apcore.span_id = %span_id,
            "apcore.module_call span started"
        );

        Self::write_key(ctx, TRACING_SPAN_ID, Value::String(span_id));
        Self::write_key(
            ctx,
            TRACING_SPAN_NAME_KEY,
            Value::String(module_id.to_string()),
        );
        Self::write_key(ctx, TRACING_ATTRIBUTES_KEY, attributes);

        Ok(None)
    }

    async fn after(
        &self,
        _module_id: &str,
        _inputs: Value,
        _output: Value,
        ctx: &Context<Value>,
    ) -> Result<Option<Value>, ModuleError> {
        if !self.is_enabled() {
            return Ok(None);
        }
        Self::write_key(
            ctx,
            TRACING_SPAN_STATUS_KEY,
            Value::String("ok".to_string()),
        );
        Ok(None)
    }

    async fn on_error(
        &self,
        _module_id: &str,
        _inputs: Value,
        _error: &ModuleError,
        ctx: &Context<Value>,
    ) -> Result<Option<Value>, ModuleError> {
        if !self.is_enabled() {
            return Ok(None);
        }
        Self::write_key(
            ctx,
            TRACING_SPAN_STATUS_KEY,
            Value::String("error".to_string()),
        );
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Context, Identity};
    use crate::errors::ErrorCode;

    fn make_ctx(caller: &str) -> Context<Value> {
        let identity = Identity::new(
            "test".to_string(),
            "user".to_string(),
            vec![],
            std::collections::HashMap::new(),
        );
        let mut ctx: Context<Value> = Context::new(identity);
        ctx.caller_id = Some(caller.to_string());
        ctx
    }

    #[tokio::test]
    async fn enabled_writes_span_id_and_attributes() {
        let mw = TracingMiddleware::builder().enabled(true).build();
        let ctx = make_ctx("orchestrator.notifications");
        mw.before("executor.email.send_email", Value::Null, &ctx)
            .await
            .unwrap();

        let data = ctx.data.read();
        let span_id = data
            .get(TRACING_SPAN_ID)
            .and_then(|v| v.as_str())
            .expect("span_id must be written");
        assert!(!span_id.is_empty());

        let attrs = data
            .get(TRACING_ATTRIBUTES_KEY)
            .and_then(|v| v.as_object())
            .expect("attributes must be written");
        assert_eq!(
            attrs.get("apcore.module_id").and_then(|v| v.as_str()),
            Some("executor.email.send_email")
        );
        assert_eq!(
            attrs.get("apcore.caller_id").and_then(|v| v.as_str()),
            Some("orchestrator.notifications")
        );
        assert_eq!(
            attrs
                .get("apcore.trace_id")
                .and_then(|v| v.as_str())
                .map(str::len),
            Some(32)
        );

        assert_eq!(
            data.get(TRACING_SPAN_NAME_KEY).and_then(|v| v.as_str()),
            Some("executor.email.send_email")
        );
    }

    #[tokio::test]
    async fn disabled_is_silent_noop() {
        let mw = TracingMiddleware::builder().enabled(false).build();
        let ctx = make_ctx("orch");
        mw.before("mod.a", Value::Null, &ctx).await.unwrap();
        let data = ctx.data.read();
        assert!(data.get(TRACING_SPAN_ID).is_none());
        assert!(data.get(TRACING_ATTRIBUTES_KEY).is_none());
    }

    #[tokio::test]
    async fn after_records_ok_status() {
        let mw = TracingMiddleware::builder().enabled(true).build();
        let ctx = make_ctx("orch");
        mw.after("mod.a", Value::Null, Value::Null, &ctx)
            .await
            .unwrap();
        let data = ctx.data.read();
        assert_eq!(
            data.get(TRACING_SPAN_STATUS_KEY).and_then(|v| v.as_str()),
            Some("ok")
        );
    }

    #[tokio::test]
    async fn on_error_records_error_status() {
        let mw = TracingMiddleware::builder().enabled(true).build();
        let ctx = make_ctx("orch");
        let err = ModuleError::new(ErrorCode::ModuleExecuteError, "boom");
        mw.on_error("mod.a", Value::Null, &err, &ctx).await.unwrap();
        let data = ctx.data.read();
        assert_eq!(
            data.get(TRACING_SPAN_STATUS_KEY).and_then(|v| v.as_str()),
            Some("error")
        );
    }

    #[tokio::test]
    async fn disabled_on_error_does_not_panic() {
        let mw = TracingMiddleware::builder().enabled(false).build();
        let ctx = make_ctx("orch");
        let err = ModuleError::new(ErrorCode::ModuleExecuteError, "boom");
        let result = mw.on_error("mod.a", Value::Null, &err, &ctx).await;
        assert!(result.is_ok());
        let data = ctx.data.read();
        assert!(data.get(TRACING_SPAN_STATUS_KEY).is_none());
    }
}
