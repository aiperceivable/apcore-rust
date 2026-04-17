// APCore Protocol — System health modules
// Spec reference: system.health.summary, system.health.module

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::config::Config;
use crate::context::Context;
use crate::errors::{ErrorCode, ModuleError};
use crate::module::Module;
use crate::observability::error_history::ErrorHistory;
use crate::observability::metrics::{estimate_p99_from_histogram, MetricsCollector};
use crate::registry::registry::Registry;

// NOTE: `registry` is now a plain `Arc<Registry>` — interior mutability via
// `parking_lot::RwLock` means no external lock is needed.

fn classify_health(error_rate: f64, total_calls: u64, threshold: f64) -> &'static str {
    if total_calls == 0 {
        return "unknown";
    }
    if error_rate < threshold {
        "healthy"
    } else if error_rate < 0.10 {
        "degraded"
    } else {
        "error"
    }
}

/// system.health.summary — Aggregated health overview of all registered modules.
pub struct HealthSummaryModule {
    registry: Arc<Registry>,
    metrics: Option<MetricsCollector>,
    error_history: ErrorHistory,
    config: Arc<Mutex<Config>>,
}

impl HealthSummaryModule {
    pub fn new(
        registry: Arc<Registry>,
        metrics: Option<MetricsCollector>,
        error_history: ErrorHistory,
        config: Arc<Mutex<Config>>,
    ) -> Self {
        Self {
            registry,
            metrics,
            error_history,
            config,
        }
    }
}

#[async_trait]
impl Module for HealthSummaryModule {
    fn description(&self) -> &'static str {
        "Aggregated health overview of all registered modules"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "error_rate_threshold": {"type": "number", "default": 0.01},
                "include_healthy": {"type": "boolean", "default": true}
            }
        })
    }

    fn output_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    async fn execute(
        &self,
        inputs: serde_json::Value,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<serde_json::Value, ModuleError> {
        let threshold = inputs
            .get("error_rate_threshold")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.01);
        let include_healthy = inputs
            .get("include_healthy")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);

        let module_ids = self.registry.list(None, None);

        let project_name = {
            let cfg = self.config.lock().await;
            cfg.get("project.name")
                .and_then(|v| v.as_str().map(std::string::ToString::to_string))
                .unwrap_or_else(|| "apcore".to_string())
        };

        let snapshot = self
            .metrics
            .as_ref()
            .map(super::super::observability::metrics::MetricsCollector::snapshot);

        let mut modules = Vec::new();
        let (mut healthy, mut degraded, mut error_count, mut unknown) = (0u32, 0u32, 0u32, 0u32);

        for mid in &module_ids {
            let (total_calls, errors) = snapshot
                .as_ref()
                .map_or((0, 0), |s| extract_call_counts(s, mid.as_str()));
            #[allow(clippy::cast_precision_loss)] // metrics ratio: precision loss acceptable
            let error_rate = if total_calls > 0 {
                errors as f64 / total_calls as f64
            } else {
                0.0
            };
            let status = classify_health(error_rate, total_calls, threshold);

            match status {
                "healthy" => healthy += 1,
                "degraded" => degraded += 1,
                "error" => error_count += 1,
                _ => unknown += 1,
            }

            if !include_healthy && status == "healthy" {
                continue;
            }

            let top_error = self
                .error_history
                .get(mid.as_str(), Some(1))
                .first()
                .map(|e| {
                    json!({
                        "code": e.error_code,
                        "message": e.message,
                        "ai_guidance": e.ai_guidance,
                        "count": e.count,
                    })
                });

            modules.push(json!({
                "module_id": mid,
                "status": status,
                "error_rate": error_rate,
                "top_error": top_error,
            }));
        }

        Ok(json!({
            "project": { "name": project_name },
            "summary": {
                "total_modules": module_ids.len(),
                "healthy": healthy,
                "degraded": degraded,
                "error": error_count,
                "unknown": unknown,
            },
            "modules": modules,
        }))
    }
}

/// system.health.module — Detailed health for a single module.
pub struct HealthModule {
    registry: Arc<Registry>,
    metrics: Option<MetricsCollector>,
    error_history: ErrorHistory,
}

impl HealthModule {
    pub fn new(
        registry: Arc<Registry>,
        metrics: Option<MetricsCollector>,
        error_history: ErrorHistory,
    ) -> Self {
        Self {
            registry,
            metrics,
            error_history,
        }
    }
}

#[async_trait]
impl Module for HealthModule {
    fn description(&self) -> &'static str {
        "Detailed health information for a single module"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["module_id"],
            "properties": {
                "module_id": {"type": "string"},
                "error_limit": {"type": "integer", "default": 10}
            }
        })
    }

    fn output_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    async fn execute(
        &self,
        inputs: serde_json::Value,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<serde_json::Value, ModuleError> {
        let module_id = inputs
            .get("module_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ModuleError::new(ErrorCode::GeneralInvalidInput, "'module_id' is required")
            })?;
        #[allow(clippy::cast_possible_truncation)]
        // config value won't exceed platform usize limits
        let error_limit = inputs
            .get("error_limit")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(10) as usize;

        if !self.registry.has(module_id) {
            return Err(ModuleError::new(
                ErrorCode::ModuleNotFound,
                format!("Module '{module_id}' not found"),
            ));
        }

        let snapshot = self
            .metrics
            .as_ref()
            .map(super::super::observability::metrics::MetricsCollector::snapshot);
        let (total_calls, errors) = snapshot
            .as_ref()
            .map_or((0, 0), |s| extract_call_counts(s, module_id));
        #[allow(clippy::cast_precision_loss)] // metrics ratio: precision loss acceptable
        let error_rate = if total_calls > 0 {
            errors as f64 / total_calls as f64
        } else {
            0.0
        };
        let status = classify_health(error_rate, total_calls, 0.01);

        let recent_errors: Vec<serde_json::Value> = self
            .error_history
            .get(module_id, Some(error_limit))
            .into_iter()
            .map(|e| {
                json!({
                    "code": e.error_code,
                    "message": e.message,
                    "ai_guidance": e.ai_guidance,
                    "count": e.count,
                    "first_occurred": e.first_occurred.to_rfc3339(),
                    "last_occurred": e.last_occurred.to_rfc3339(),
                })
            })
            .collect();

        let (avg_latency_ms, p99_latency_ms) = snapshot
            .as_ref()
            .map_or((0.0, 0.0), |s| extract_latency_stats(s, module_id));

        Ok(json!({
            "module_id": module_id,
            "status": status,
            "total_calls": total_calls,
            "error_count": errors,
            "error_rate": error_rate,
            "avg_latency_ms": avg_latency_ms,
            "p99_latency_ms": p99_latency_ms,
            "recent_errors": recent_errors,
        }))
    }
}

/// Extract call counts from a `MetricsCollector` snapshot.
fn extract_call_counts(snapshot: &serde_json::Value, module_id: &str) -> (u64, u64) {
    let Some(counters) = snapshot.get("counters").and_then(|c| c.as_object()) else {
        return (0, 0);
    };
    let mut total: u64 = 0;
    let mut errors: u64 = 0;
    let success_key = format!("apcore_module_calls_total|module_id={module_id},status=success");
    let error_key = format!("apcore_module_calls_total|module_id={module_id},status=error");
    if let Some(v) = counters
        .get(&success_key)
        .and_then(serde_json::Value::as_u64)
    {
        total += v;
    }
    if let Some(v) = counters.get(&error_key).and_then(serde_json::Value::as_u64) {
        total += v;
        errors = v;
    }
    (total, errors)
}

/// Extract latency statistics (`avg_ms`, `p99_ms`) from a `MetricsCollector` snapshot.
///
/// Reads the histogram key `apcore_module_duration_seconds|module_id=<id>`.
/// Returns (`avg_latency_ms`, `p99_latency_ms`).
fn extract_latency_stats(snapshot: &serde_json::Value, module_id: &str) -> (f64, f64) {
    let Some(histograms) = snapshot.get("histograms").and_then(|h| h.as_object()) else {
        return (0.0, 0.0);
    };
    let hist_key = format!("apcore_module_duration_seconds|module_id={module_id}");
    let Some(data) = histograms.get(&hist_key) else {
        return (0.0, 0.0);
    };
    let sum = data
        .get("sum")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    let count = data
        .get("count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    #[allow(clippy::cast_precision_loss)] // latency avg: precision loss acceptable
    let avg_ms = if count > 0 {
        (sum / count as f64) * 1000.0
    } else {
        0.0
    };

    // Estimate p99 from histogram buckets.
    let p99_ms = if let Some(buckets) = data.get("buckets").and_then(|b| b.as_array()) {
        estimate_p99_from_histogram(buckets, count)
    } else {
        0.0
    };

    (avg_ms, p99_ms)
}
