// APCore Protocol — System usage modules
// Spec reference: system.usage.summary, system.usage.module

use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::context::Context;
use crate::errors::{ErrorCode, ModuleError};
use crate::module::Module;
use crate::observability::usage::UsageCollector;
use crate::registry::registry::Registry;

/// Hour-key format matching `UsageCollector` bucket hours
/// (`YYYY-MM-DDTHH:00:00Z`).
const HOUR_KEY_FORMAT: &str = "%Y-%m-%dT%H:00:00Z";

/// Pad an hourly distribution to exactly 24 entries, zero-filling gaps.
///
/// Generates the 24 hourly keys covering the last 24 hours (`now-23h .. now`),
/// merges in any actual data buckets, sorts and dedups, then keeps the latest
/// 24. Missing hours are filled with zero counts. Spec MUST: `system.usage.module`
/// always returns 24 hourly entries. Mirrors apcore-python
/// `_pad_hourly_distribution` (usage.py:172) — sync finding A-D-13.
fn pad_hourly_distribution(hourly: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let now = Utc::now();

    // Map existing hour-key -> (call_count, error_count).
    let mut existing: std::collections::HashMap<String, (u64, u64)> =
        std::collections::HashMap::new();
    for h in hourly {
        if let Some(hour) = h.get("hour").and_then(serde_json::Value::as_str) {
            let calls = h
                .get("call_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let errors = h
                .get("error_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            existing.insert(hour.to_string(), (calls, errors));
        }
    }

    // Generate the 24 hourly keys for now-23h .. now, then union with any
    // data keys not already present, sort+dedup, and take the latest 24.
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for i in 0..24i64 {
        let hour_dt = now - Duration::hours(23 - i);
        keys.insert(hour_dt.format(HOUR_KEY_FORMAT).to_string());
    }
    for k in existing.keys() {
        keys.insert(k.clone());
    }
    let latest_24: Vec<String> = {
        let all: Vec<String> = keys.into_iter().collect();
        let start = all.len().saturating_sub(24);
        all[start..].to_vec()
    };

    latest_24
        .into_iter()
        .map(|key| {
            let (call_count, error_count) = existing.get(&key).copied().unwrap_or((0, 0));
            json!({
                "hour": key,
                "call_count": call_count,
                "error_count": error_count,
            })
        })
        .collect()
}

/// system.usage.summary — Usage overview with trend detection across all modules.
pub struct UsageSummaryModule {
    collector: UsageCollector,
}

impl UsageSummaryModule {
    #[must_use]
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
        summaries.sort_by_key(|b| std::cmp::Reverse(b.call_count));

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
pub struct UsageModule {
    registry: Arc<Registry>,
    collector: UsageCollector,
}

impl UsageModule {
    pub fn new(registry: Arc<Registry>, collector: UsageCollector) -> Self {
        Self {
            registry,
            collector,
        }
    }
}

#[async_trait]
impl Module for UsageModule {
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
        // Reject an empty module_id with InvalidInput (GENERAL_INVALID_INPUT)
        // rather than letting it fall through to ModuleNotFound, matching
        // apcore-python / apcore-typescript.
        let module_id = super::require_string(&inputs, "module_id")?;
        let module_id = module_id.as_str();
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

        // Spec MUST: always return exactly 24 hourly entries, zero-filling
        // gaps (sync finding A-D-13).
        let hourly = pad_hourly_distribution(&hourly);

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
                "hourly_distribution": hourly,
            })),
        }
    }
}
