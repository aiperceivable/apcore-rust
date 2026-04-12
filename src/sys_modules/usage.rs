// APCore Protocol — System usage modules
// Spec reference: system.usage.summary, system.usage.module

use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;

use crate::context::Context;
use crate::errors::{ErrorCode, ModuleError};
use crate::module::Module;
use crate::observability::usage::UsageCollector;
use crate::registry::registry::Registry;

/// system.usage.summary — Usage overview with trend detection across all modules.
pub struct UsageSummaryModule {
    collector: UsageCollector,
}

impl UsageSummaryModule {
    pub fn new(collector: UsageCollector) -> Self {
        Self { collector }
    }
}

#[async_trait]
impl Module for UsageSummaryModule {
    fn description(&self) -> &'static str {
        "Usage overview with trend detection across all modules"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "period": {"type": "string", "default": "24h"}
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
        let period = inputs
            .get("period")
            .and_then(|v| v.as_str())
            .unwrap_or("24h");

        let mut summaries = self.collector.get_all_summaries();
        // Sort by call_count descending per spec.
        summaries.sort_by(|a, b| b.call_count.cmp(&a.call_count));

        let total_calls: u64 = summaries.iter().map(|s| s.call_count).sum();
        let total_errors: u64 = summaries.iter().map(|s| s.error_count).sum();

        let modules: Vec<serde_json::Value> = summaries
            .into_iter()
            .map(|s| {
                json!({
                    "module_id": s.module_id,
                    "call_count": s.call_count,
                    "error_count": s.error_count,
                    "avg_latency_ms": s.avg_latency_ms,
                    "unique_callers": s.unique_callers,
                    "trend": s.trend,
                })
            })
            .collect();

        Ok(json!({
            "period": period,
            "total_calls": total_calls,
            "total_errors": total_errors,
            "modules": modules,
        }))
    }
}

/// system.usage.module — Detailed usage for a single module.
pub struct UsageModuleModule {
    registry: Arc<Registry>,
    collector: UsageCollector,
}

impl UsageModuleModule {
    pub fn new(registry: Arc<Registry>, collector: UsageCollector) -> Self {
        Self {
            registry,
            collector,
        }
    }
}

#[async_trait]
impl Module for UsageModuleModule {
    fn description(&self) -> &'static str {
        "Detailed usage for a single module with caller breakdown"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "required": ["module_id"],
            "properties": {
                "module_id": {"type": "string"},
                "period": {"type": "string", "default": "24h"}
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
        let period = inputs
            .get("period")
            .and_then(|v| v.as_str())
            .unwrap_or("24h");

        if !self.registry.has(module_id) {
            return Err(ModuleError::new(
                ErrorCode::ModuleNotFound,
                format!("Module '{module_id}' not found"),
            ));
        }

        let stats = self.collector.get_module_summary(module_id);
        let p99 = self.collector.get_p99_latency_ms(module_id);
        let callers: Vec<serde_json::Value> = self
            .collector
            .get_caller_breakdown(module_id)
            .into_iter()
            .map(|c| {
                json!({
                    "caller_id": c.caller_id,
                    "call_count": c.call_count,
                    "error_count": c.error_count,
                    "avg_latency_ms": c.avg_latency_ms,
                })
            })
            .collect();
        let hourly: Vec<serde_json::Value> = self
            .collector
            .get_hourly_distribution(module_id)
            .into_iter()
            .map(|h| {
                json!({
                    "hour": h.hour,
                    "call_count": h.call_count,
                    "error_count": h.error_count,
                })
            })
            .collect();

        match stats {
            Some(s) => Ok(json!({
                "module_id": module_id,
                "period": period,
                "call_count": s.call_count,
                "error_count": s.error_count,
                "avg_latency_ms": s.avg_latency_ms,
                "p99_latency_ms": p99,
                "trend": s.trend,
                "callers": callers,
                "hourly_distribution": hourly,
            })),
            None => Ok(json!({
                "module_id": module_id,
                "period": period,
                "call_count": 0,
                "error_count": 0,
                "avg_latency_ms": 0.0,
                "p99_latency_ms": 0.0,
                "trend": "inactive",
                "callers": [],
                "hourly_distribution": [],
            })),
        }
    }
}
