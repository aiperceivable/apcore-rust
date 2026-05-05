// APCore Protocol — Middleware module
// Spec reference: Middleware pipeline

pub mod adapters;
pub mod base;
pub mod circuit_breaker;
pub mod context_namespace;
pub mod logging;
pub mod manager;
pub mod otel_tracing;
pub mod retry;
pub mod yaml_config;

pub use adapters::{AfterAdapter, AfterMiddleware, BeforeAdapter, BeforeMiddleware};
pub use base::{Middleware, OnErrorOutcome, RetrySignal};
pub use circuit_breaker::{
    CircuitBreakerBuilder, CircuitBreakerConfig, CircuitBreakerMiddleware, CircuitBreakerState,
    DEFAULT_MIN_SAMPLES as CB_DEFAULT_MIN_SAMPLES,
    DEFAULT_OPEN_THRESHOLD as CB_DEFAULT_OPEN_THRESHOLD,
    DEFAULT_RECOVERY_WINDOW_MS as CB_DEFAULT_RECOVERY_WINDOW_MS,
    DEFAULT_WINDOW_SIZE as CB_DEFAULT_WINDOW_SIZE,
};
pub use context_namespace::{
    enforce_context_key, namespace_keys, validate_context_key, ContextWriter, NamespaceCheck,
    APCORE_KEY_PREFIX, EXT_KEY_PREFIX,
};
pub use logging::LoggingMiddleware;
pub use manager::MiddlewareManager;
pub use otel_tracing::{
    TracingBuilder as OtelTracingBuilder, TracingConfig as OtelTracingConfig,
    TracingMiddleware as OtelTracingMiddleware, TRACING_ATTRIBUTES_KEY, TRACING_SPAN_NAME_KEY,
    TRACING_SPAN_STATUS_KEY,
};
pub use retry::{RetryConfig, RetryMiddleware};
pub use yaml_config::{
    CircuitBreakerMiddlewareConfig, CustomMiddlewareConfig, CustomMiddlewareFactory,
    LoggingMiddlewareConfig, MiddlewareChainConfig, MiddlewareConfig, MiddlewareFactory,
    TracingMiddlewareConfig,
};

use std::collections::{HashMap, HashSet};

use parking_lot::Mutex;

use async_trait::async_trait;

use crate::context::Context;
use crate::errors::ModuleError;
use crate::events::emitter::{ApCoreEvent, EventEmitter};
use crate::observability::metrics::{estimate_p99_from_histogram, MetricsCollector};

/// Platform notification middleware — monitors error rates and latency,
/// emits threshold events with hysteresis.
///
/// Emits `apcore.health.error_threshold_exceeded` when a module's error rate
/// crosses the configured threshold, `apcore.health.latency_threshold_exceeded`
/// when p99 latency exceeds the limit, and `apcore.health.recovered` when a
/// previously alerted module recovers below `threshold * 0.5`.
///
/// **Issue #36 — canonical event-name standardization:** every threshold
/// event is dual-emitted under both the canonical `apcore.health.*` name and
/// its legacy bare-name alias (e.g. `error_threshold_exceeded`) so that
/// existing subscribers continue to fire while consumers migrate to the
/// canonical names. The legacy event payload carries a `deprecated: true`
/// marker.
///
/// Hysteresis prevents repeated alerts until recovery is observed.
#[derive(Debug)]
pub struct PlatformNotifyMiddleware {
    emitter: EventEmitter,
    metrics_collector: Option<MetricsCollector>,
    error_rate_threshold: f64,
    latency_p99_threshold_ms: f64,
    /// Tracks which alert types are active per module to implement hysteresis.
    /// Key: `module_id`, Value: set of alert type strings ("`error_rate`", "latency").
    alerted: Mutex<HashMap<String, HashSet<String>>>,
}

impl PlatformNotifyMiddleware {
    /// Create a new platform notify middleware.
    ///
    /// # Arguments
    /// * `emitter` — `EventEmitter` to emit threshold events to.
    /// * `metrics_collector` — Optional `MetricsCollector` to read error rates
    ///   and latency from. If None, all checks return 0.
    /// * `error_rate_threshold` — Error rate (0.0-1.0) above which to alert.
    /// * `latency_p99_threshold_ms` — p99 latency in ms above which to alert.
    #[must_use]
    pub fn new(
        emitter: EventEmitter,
        metrics_collector: Option<MetricsCollector>,
        error_rate_threshold: f64,
        latency_p99_threshold_ms: f64,
    ) -> Self {
        Self {
            emitter,
            metrics_collector,
            error_rate_threshold,
            latency_p99_threshold_ms,
            alerted: Mutex::new(HashMap::new()),
        }
    }

    /// Create with default thresholds (10% error rate, 5000ms p99 latency).
    #[must_use]
    pub fn with_defaults(
        emitter: EventEmitter,
        metrics_collector: Option<MetricsCollector>,
    ) -> Self {
        Self::new(emitter, metrics_collector, 0.1, 5000.0)
    }

    /// Compute error rate for a module from `MetricsCollector` snapshot.
    fn compute_error_rate(&self, module_id: &str) -> f64 {
        if self.metrics_collector.is_none() {
            return 0.0;
        }
        let collector = self.metrics_collector.as_ref().unwrap();
        let snap = collector.snapshot();

        // snapshot() returns {"counters": {...}, "histograms": {...}}
        let Some(counters) = snap.get("counters") else {
            return 0.0;
        };

        // Look for counters matching apcore_module_calls_total with module label.
        // Keys are formatted as "name|key=value,key=value".
        let total_key = format!("apcore_module_calls_total|module={module_id},status=success");
        let error_key = format!("apcore_module_calls_total|module={module_id},status=error");

        let success = counters
            .get(&total_key)
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let errors = counters
            .get(&error_key)
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let total = success + errors;

        if total == 0.0 {
            return 0.0;
        }
        errors / total
    }

    /// Check error rate threshold; returns the canonical + legacy event pair
    /// to emit if threshold exceeded (with hysteresis).
    ///
    /// Issue #36 — canonical events use the `apcore.<subsystem>.<event>` form;
    /// the legacy bare-name event is dual-emitted with a `deprecated: true`
    /// marker so existing subscribers continue to fire during migration.
    fn check_error_rate_threshold(&self, module_id: &str) -> Vec<ApCoreEvent> {
        let error_rate = self.compute_error_rate(module_id);
        let mut alerted = self.alerted.lock();
        let module_alerts = alerted.entry(module_id.to_string()).or_default();

        if error_rate >= self.error_rate_threshold && !module_alerts.contains("error_rate") {
            let canonical = ApCoreEvent::with_module(
                "apcore.health.error_threshold_exceeded",
                serde_json::json!({
                    "error_rate": error_rate,
                    "threshold": self.error_rate_threshold,
                }),
                module_id,
                "error",
            );
            // Legacy alias — kept for one major release per Issue #36
            // deprecation policy.
            let legacy = ApCoreEvent::with_module(
                "error_threshold_exceeded",
                serde_json::json!({
                    "error_rate": error_rate,
                    "threshold": self.error_rate_threshold,
                    "deprecated": true,
                    "canonical_event": "apcore.health.error_threshold_exceeded",
                }),
                module_id,
                "error",
            );
            module_alerts.insert("error_rate".to_string());
            vec![canonical, legacy]
        } else {
            vec![]
        }
    }

    /// Check latency threshold; returns canonical + legacy events if p99
    /// exceeds the configured limit (Issue #36 — dual emission).
    fn check_latency_threshold(&self, module_id: &str) -> Vec<ApCoreEvent> {
        let Some(collector) = self.metrics_collector.as_ref() else {
            return vec![];
        };
        let snap = collector.snapshot();

        let Some(histograms) = snap.get("histograms").and_then(|v| v.as_object()) else {
            return vec![];
        };
        let hist_key = format!("apcore_module_duration_seconds|module_id={module_id}");
        let Some(data) = histograms.get(&hist_key) else {
            return vec![];
        };

        let count = data
            .get("count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if count == 0 {
            return vec![];
        }

        // Estimate p99 from cumulative histogram buckets.
        let Some(buckets) = data.get("buckets").and_then(|v| v.as_array()) else {
            return vec![];
        };
        let p99_ms = estimate_p99_from_histogram(buckets, count);

        let mut alerted = self.alerted.lock();
        let module_alerts = alerted.entry(module_id.to_string()).or_default();
        if p99_ms >= self.latency_p99_threshold_ms && !module_alerts.contains("latency") {
            let canonical = ApCoreEvent::with_module(
                "apcore.health.latency_threshold_exceeded",
                serde_json::json!({
                    "p99_latency_ms": p99_ms,
                    "threshold": self.latency_p99_threshold_ms,
                }),
                module_id,
                "warn",
            );
            let legacy = ApCoreEvent::with_module(
                "latency_threshold_exceeded",
                serde_json::json!({
                    "p99_latency_ms": p99_ms,
                    "threshold": self.latency_p99_threshold_ms,
                    "deprecated": true,
                    "canonical_event": "apcore.health.latency_threshold_exceeded",
                }),
                module_id,
                "warn",
            );
            module_alerts.insert("latency".to_string());
            return vec![canonical, legacy];
        }
        vec![]
    }

    /// Check if error rate has recovered; returns an event to emit if recovered.
    fn check_error_recovery(&self, module_id: &str) -> Option<ApCoreEvent> {
        let error_rate = self.compute_error_rate(module_id);
        let mut alerted = self.alerted.lock();

        let has_alert = alerted
            .get(module_id)
            .is_some_and(|s| s.contains("error_rate"));

        if !has_alert {
            return None;
        }

        if error_rate < self.error_rate_threshold * 0.5 {
            let event = ApCoreEvent::with_module(
                "apcore.health.recovered",
                serde_json::json!({
                    "status": "recovered",
                    "error_rate": error_rate,
                }),
                module_id,
                "info",
            );
            if let Some(module_alerts) = alerted.get_mut(module_id) {
                module_alerts.remove("error_rate");
            }
            Some(event)
        } else {
            None
        }
    }
}

#[async_trait]
impl Middleware for PlatformNotifyMiddleware {
    fn name(&self) -> &'static str {
        "platform_notify"
    }

    async fn before(
        &self,
        _module_id: &str,
        _inputs: serde_json::Value,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        // No-op before hook — matching Python reference.
        Ok(None)
    }

    async fn after(
        &self,
        module_id: &str,
        _inputs: serde_json::Value,
        _output: serde_json::Value,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        // Check latency threshold and error-rate recovery after execution.
        for event in self.check_latency_threshold(module_id) {
            self.emitter.emit(&event).await;
        }
        if let Some(event) = self.check_error_recovery(module_id) {
            self.emitter.emit(&event).await;
        }
        Ok(None)
    }

    async fn on_error(
        &self,
        module_id: &str,
        _inputs: serde_json::Value,
        _error: &ModuleError,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        // Check error rate threshold on error.
        for event in self.check_error_rate_threshold(module_id) {
            self.emitter.emit(&event).await;
        }
        Ok(None)
    }
}
