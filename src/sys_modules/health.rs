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
use crate::observability::metrics::MetricsCollector;
use crate::registry::registry::Registry;

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
    registry: Arc<Mutex<Registry>>,
    metrics: Option<MetricsCollector>,
    error_history: ErrorHistory,
    config: Arc<Mutex<Config>>,
}

impl HealthSummaryModule {
    pub fn new(
        registry: Arc<Mutex<Registry>>,
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
    fn description(&self) -> &str {
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
            .and_then(|v| v.as_f64())
            .unwrap_or(0.01);
        let include_healthy = inputs
            .get("include_healthy")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let reg = self.registry.lock().await;
        let module_ids = reg.list(None, None);

        let project_name = {
            let cfg = self.config.lock().await;
            cfg.get("project.name")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "apcore".to_string())
        };

        let snapshot = self.metrics.as_ref().map(|m| m.snapshot());

        let mut modules = Vec::new();
        let (mut healthy, mut degraded, mut error_count, mut unknown) = (0u32, 0u32, 0u32, 0u32);

        for mid in &module_ids {
            let (total_calls, errors) = snapshot
                .as_ref()
                .map(|s| extract_call_counts(s, mid))
                .unwrap_or((0, 0));
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

            let top_error = self.error_history.get(mid, Some(1)).first().map(|e| {
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
pub struct HealthModuleModule {
    registry: Arc<Mutex<Registry>>,
    metrics: Option<MetricsCollector>,
    error_history: ErrorHistory,
}

impl HealthModuleModule {
    pub fn new(
        registry: Arc<Mutex<Registry>>,
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
impl Module for HealthModuleModule {
    fn description(&self) -> &str {
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
        let error_limit = inputs
            .get("error_limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;

        {
            let reg = self.registry.lock().await;
            if !reg.has(module_id) {
                return Err(ModuleError::new(
                    ErrorCode::ModuleNotFound,
                    format!("Module '{}' not found", module_id),
                ));
            }
        }

        let snapshot = self.metrics.as_ref().map(|m| m.snapshot());
        let (total_calls, errors) = snapshot
            .as_ref()
            .map(|s| extract_call_counts(s, module_id))
            .unwrap_or((0, 0));
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

        Ok(json!({
            "module_id": module_id,
            "status": status,
            "total_calls": total_calls,
            "error_count": errors,
            "error_rate": error_rate,
            "recent_errors": recent_errors,
        }))
    }
}

/// Extract call counts from a MetricsCollector snapshot.
fn extract_call_counts(snapshot: &serde_json::Value, module_id: &str) -> (u64, u64) {
    let counters = match snapshot.get("counters").and_then(|c| c.as_object()) {
        Some(c) => c,
        None => return (0, 0),
    };
    let mut total: u64 = 0;
    let mut errors: u64 = 0;
    let success_key = format!("apcore_module_calls_total|module_id={module_id},status=success");
    let error_key = format!("apcore_module_calls_total|module_id={module_id},status=error");
    if let Some(v) = counters.get(&success_key).and_then(|v| v.as_u64()) {
        total += v;
    }
    if let Some(v) = counters.get(&error_key).and_then(|v| v.as_u64()) {
        total += v;
        errors = v;
    }
    (total, errors)
}
