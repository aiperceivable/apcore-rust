// APCore Protocol — Usage tracking
// Spec reference: Module usage statistics and middleware

use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::context::Context;
use crate::errors::ModuleError;
use crate::middleware::base::Middleware;
use crate::observability::metrics::estimate_p99_from_sorted;

/// A single usage record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UsageRecord {
    pub timestamp: String,
    pub caller_id: Option<String>,
    pub latency_ms: f64,
    pub success: bool,
}

/// Usage summary for a single module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageStats {
    pub module_id: String,
    pub call_count: u64,
    pub error_count: u64,
    pub avg_latency_ms: f64,
    pub unique_callers: usize,
    pub trend: String,
}

/// Maximum records per hourly bucket before oldest are evicted.
const MAX_RECORDS_PER_BUCKET: usize = 10_000;
/// Maximum hourly buckets retained per module (168 = 7 days).
const MAX_BUCKETS_PER_MODULE: usize = 168;

/// Internal module data for aggregation.
#[derive(Debug, Clone)]
struct ModuleData {
    records: HashMap<String, Vec<UsageRecord>>, // bucket_key -> records
}

impl ModuleData {
    fn new() -> Self {
        Self {
            records: HashMap::new(),
        }
    }

    /// Evict buckets exceeding the retention limit (keeps newest).
    fn evict_old_buckets(&mut self) {
        if self.records.len() > MAX_BUCKETS_PER_MODULE {
            let mut keys: Vec<String> = self.records.keys().cloned().collect();
            keys.sort();
            let to_remove = keys.len() - MAX_BUCKETS_PER_MODULE;
            for key in keys.into_iter().take(to_remove) {
                self.records.remove(&key);
            }
        }
    }
}

/// Collects usage statistics across module executions.
#[derive(Debug, Clone)]
pub struct UsageCollector {
    data: Arc<Mutex<HashMap<String, ModuleData>>>,
}

impl UsageCollector {
    /// Create a new usage collector.
    pub fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Generate hourly bucket key from current time: "YYYY-MM-DDTHH".
    fn bucket_key() -> String {
        Utc::now().format("%Y-%m-%dT%H").to_string()
    }

    /// Record a module execution.
    pub fn record(&self, module_id: &str, caller_id: Option<&str>, latency_ms: f64, success: bool) {
        let record = UsageRecord {
            timestamp: Utc::now().to_rfc3339(),
            caller_id: caller_id.map(std::string::ToString::to_string),
            latency_ms,
            success,
        };

        let bucket = Self::bucket_key();
        let mut data = self.data.lock();
        let module = data
            .entry(module_id.to_string())
            .or_insert_with(ModuleData::new);
        let bucket_records = module.records.entry(bucket).or_default();
        bucket_records.push(record);
        // Evict oldest records if bucket exceeds limit
        if bucket_records.len() > MAX_RECORDS_PER_BUCKET {
            let excess = bucket_records.len() - MAX_RECORDS_PER_BUCKET;
            bucket_records.drain(..excess);
        }
        // Evict old hourly buckets if retention exceeded
        module.evict_old_buckets();
    }

    /// Aggregate records for a single module into a UsageStats.
    fn aggregate(module_id: &str, module_data: &ModuleData) -> UsageStats {
        let mut call_count: u64 = 0;
        let mut error_count: u64 = 0;
        let mut total_latency: f64 = 0.0;
        let mut unique_callers: HashSet<String> = HashSet::new();

        for records in module_data.records.values() {
            for record in records {
                call_count += 1;
                if !record.success {
                    error_count += 1;
                }
                total_latency += record.latency_ms;
                if let Some(ref cid) = record.caller_id {
                    unique_callers.insert(cid.clone());
                }
            }
        }

        #[allow(clippy::cast_precision_loss)]
        // intentional: call counts fit in f64 for realistic usage stats
        let avg_latency_ms = if call_count > 0 {
            total_latency / call_count as f64
        } else {
            0.0
        };

        UsageStats {
            module_id: module_id.to_string(),
            call_count,
            error_count,
            avg_latency_ms,
            unique_callers: unique_callers.len(),
            trend: "stable".to_string(),
        }
    }

    /// Get usage summary for a specific module.
    pub fn get_module_summary(&self, module_id: &str) -> Option<UsageStats> {
        let data = self.data.lock();
        data.get(module_id).map(|md| Self::aggregate(module_id, md))
    }

    /// Get all usage summaries.
    pub fn get_all_summaries(&self) -> Vec<UsageStats> {
        let data = self.data.lock();
        data.iter()
            .map(|(mid, md)| Self::aggregate(mid, md))
            .collect()
    }

    /// Get per-caller breakdown for a module.
    pub(crate) fn get_caller_breakdown(&self, module_id: &str) -> Vec<CallerStats> {
        let data = self.data.lock();
        let Some(module_data) = data.get(module_id) else {
            return Vec::new();
        };
        let mut callers: HashMap<String, (u64, u64, f64)> = HashMap::new(); // (calls, errors, total_lat)
        for records in module_data.records.values() {
            for rec in records {
                let cid = rec.caller_id.as_deref().unwrap_or("unknown").to_string();
                let entry = callers.entry(cid).or_insert((0, 0, 0.0));
                entry.0 += 1;
                if !rec.success {
                    entry.1 += 1;
                }
                entry.2 += rec.latency_ms;
            }
        }
        callers
            .into_iter()
            .map(|(cid, (calls, errs, total_lat))| CallerStats {
                caller_id: cid,
                call_count: calls,
                error_count: errs,
                avg_latency_ms: if calls > 0 {
                    #[allow(clippy::cast_precision_loss)] // metrics avg: precision loss acceptable
                    let avg = total_lat / calls as f64;
                    avg
                } else {
                    0.0
                },
            })
            .collect()
    }

    /// Get hourly distribution for a module (sorted by hour ascending).
    pub(crate) fn get_hourly_distribution(&self, module_id: &str) -> Vec<HourlyBucket> {
        let data = self.data.lock();
        let Some(module_data) = data.get(module_id) else {
            return Vec::new();
        };
        let mut buckets: Vec<HourlyBucket> = module_data
            .records
            .iter()
            .map(|(hour, records)| {
                let call_count = records.len() as u64;
                let error_count = records.iter().filter(|r| !r.success).count() as u64;
                HourlyBucket {
                    hour: format!("{hour}:00:00Z"),
                    call_count,
                    error_count,
                }
            })
            .collect();
        buckets.sort_by(|a, b| a.hour.cmp(&b.hour));
        buckets
    }

    /// Compute p99 latency (ms) for a module from stored records.
    pub fn get_p99_latency_ms(&self, module_id: &str) -> f64 {
        let data = self.data.lock();
        let Some(module_data) = data.get(module_id) else {
            return 0.0;
        };
        let mut latencies: Vec<f64> = module_data
            .records
            .values()
            .flat_map(|recs| recs.iter().map(|r| r.latency_ms))
            .collect();
        if latencies.is_empty() {
            return 0.0;
        }
        latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        estimate_p99_from_sorted(&latencies)
    }

    /// Reset all stats.
    pub fn reset(&self) {
        self.data.lock().clear();
    }
}

/// Per-caller usage statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CallerStats {
    pub caller_id: String,
    pub call_count: u64,
    pub error_count: u64,
    pub avg_latency_ms: f64,
}

/// Hourly usage bucket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HourlyBucket {
    pub hour: String,
    pub call_count: u64,
    pub error_count: u64,
}

impl Default for UsageCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Middleware that tracks usage statistics.
///
/// WARNING: The internal start-time stack is not safe for concurrent use on
/// the same middleware instance. Use separate instances per concurrent pipeline.
#[derive(Debug)]
pub struct UsageMiddleware {
    collector: UsageCollector,
    starts: Mutex<HashMap<String, std::time::Instant>>,
}

impl UsageMiddleware {
    /// Create a new usage middleware.
    pub fn new(collector: UsageCollector) -> Self {
        Self {
            collector,
            starts: Mutex::new(HashMap::new()),
        }
    }

    /// Get a reference to the underlying collector.
    pub fn collector(&self) -> &UsageCollector {
        &self.collector
    }
}

#[async_trait]
impl Middleware for UsageMiddleware {
    fn name(&self) -> &'static str {
        "usage"
    }

    async fn before(
        &self,
        _module_id: &str,
        _inputs: serde_json::Value,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        let mut starts = self.starts.lock();
        starts.insert(_ctx.trace_id.clone(), std::time::Instant::now());
        Ok(None)
    }

    async fn after(
        &self,
        module_id: &str,
        _inputs: serde_json::Value,
        _output: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        let latency_ms = {
            let mut starts = self.starts.lock();
            starts
                .remove(&ctx.trace_id)
                .map_or(0.0, |s| s.elapsed().as_secs_f64() * 1000.0)
        };

        self.collector
            .record(module_id, ctx.caller_id.as_deref(), latency_ms, true);

        Ok(None)
    }

    async fn on_error(
        &self,
        module_id: &str,
        _inputs: serde_json::Value,
        _error: &ModuleError,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        let latency_ms = {
            let mut starts = self.starts.lock();
            starts
                .remove(&ctx.trace_id)
                .map_or(0.0, |s| s.elapsed().as_secs_f64() * 1000.0)
        };

        self.collector
            .record(module_id, ctx.caller_id.as_deref(), latency_ms, false);

        Ok(None)
    }
}
