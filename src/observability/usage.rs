// APCore Protocol — Usage tracking
// Spec reference: Module usage statistics and middleware
// Sync findings:
//   * D-27 — `record()` honors an optional explicit timestamp;
//            trend is computed from samples (current vs previous period
//            counts); summary accepts a period filter.
//   * Issue #43 §1 — optional `StorageBackend` for cross-process persistence.

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::context::Context;
use crate::errors::ModuleError;
use crate::middleware::base::Middleware;
use crate::observability::metrics::estimate_p99_from_sorted;
use crate::observability::storage::StorageBackend;

/// A single usage record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UsageRecord {
    pub timestamp: DateTime<Utc>,
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
    /// D-27: derived from sample counts (current vs previous period).
    /// Possible values: `"stable"`, `"rising"`, `"declining"`, `"new"`,
    /// `"inactive"` — matches Python/TS exactly.
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
    /// Issue #43 §1: optional `StorageBackend` for persistence beyond process
    /// lifetime. Each `record()` writes a serialized `UsageRecord` to the
    /// backend under namespace `"usage"`; the in-memory aggregation still
    /// drives the synchronous accessors so reads remain fast.
    storage_backend: Option<Arc<dyn StorageBackend>>,
}

impl UsageCollector {
    /// Create a new usage collector.
    #[must_use]
    pub fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new(HashMap::new())),
            storage_backend: None,
        }
    }

    /// Create with an optional `StorageBackend` (Issue #43 §1).
    #[must_use]
    pub fn with_storage_backend(storage_backend: Option<Arc<dyn StorageBackend>>) -> Self {
        Self {
            data: Arc::new(Mutex::new(HashMap::new())),
            storage_backend,
        }
    }

    /// Generate hourly bucket key from a timestamp: "YYYY-MM-DDTHH".
    fn bucket_key(ts: DateTime<Utc>) -> String {
        ts.format("%Y-%m-%dT%H").to_string()
    }

    /// Record a module execution at the current time.
    pub fn record(&self, module_id: &str, caller_id: Option<&str>, latency_ms: f64, success: bool) {
        self.record_at(module_id, caller_id, latency_ms, success, Utc::now());
    }

    /// Record a module execution at an explicit timestamp (D-27).
    ///
    /// Used by tests to drive trend computation deterministically and by
    /// integrations that replay historical events. The timestamp is also the
    /// hourly bucket key, so back-dated records sort correctly into past
    /// buckets.
    pub fn record_at(
        &self,
        module_id: &str,
        caller_id: Option<&str>,
        latency_ms: f64,
        success: bool,
        at: DateTime<Utc>,
    ) {
        let record = UsageRecord {
            timestamp: at,
            caller_id: caller_id.map(std::string::ToString::to_string),
            latency_ms,
            success,
        };

        let bucket = Self::bucket_key(at);
        {
            let mut data = self.data.lock();
            let module = data
                .entry(module_id.to_string())
                .or_insert_with(ModuleData::new);
            let bucket_records = module.records.entry(bucket).or_default();
            bucket_records.push(record.clone());
            if bucket_records.len() > MAX_RECORDS_PER_BUCKET {
                let excess = bucket_records.len() - MAX_RECORDS_PER_BUCKET;
                bucket_records.drain(..excess);
            }
            module.evict_old_buckets();
        }

        // Forward to the optional storage backend (Issue #43 §1). Best-effort:
        // when no tokio runtime is active or serialization fails the call is
        // dropped silently. We deliberately do NOT block the in-memory path
        // on backend latency.
        if let Some(backend) = self.storage_backend.clone() {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                let module_id = module_id.to_string();
                let key = format!("{module_id}:{}", at.timestamp_nanos_opt().unwrap_or(0));
                handle.spawn(async move {
                    if let Ok(value) = serde_json::to_value(&record) {
                        let _ = backend.save("usage", &key, value).await;
                    }
                });
            }
        }
    }

    /// Aggregate a slice of records into a `UsageStats` (helper, no lock).
    fn aggregate_records(module_id: &str, records: &[UsageRecord], trend: String) -> UsageStats {
        let mut call_count: u64 = 0;
        let mut error_count: u64 = 0;
        let mut total_latency: f64 = 0.0;
        let mut unique_callers: HashSet<String> = HashSet::new();
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
        #[allow(clippy::cast_precision_loss)]
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
            trend,
        }
    }

    /// Collect records for a module within `[start, end]`. Caller holds the lock.
    fn collect_records_in_window(
        module_data: &ModuleData,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Vec<UsageRecord> {
        let start_key = Self::bucket_key(start);
        let end_key = Self::bucket_key(end);
        let mut out: Vec<UsageRecord> = Vec::new();
        for (bk, recs) in &module_data.records {
            // Hourly bucket keys are lexicographically ordered, so we can prune
            // buckets outside the window without parsing them.
            if bk.as_str() < start_key.as_str() || bk.as_str() > end_key.as_str() {
                continue;
            }
            for r in recs {
                if r.timestamp >= start && r.timestamp <= end {
                    out.push(r.clone());
                }
            }
        }
        out
    }

    /// Compute the trend label by comparing sample counts (D-27).
    ///
    /// Mirrors Python `_compute_trend` exactly:
    ///   - both 0 → `"stable"`
    ///   - current 0 (with previous > 0) → `"inactive"`
    ///   - previous 0 (with current > 0) → `"new"`
    ///   - ratio > 1.2 → `"rising"`
    ///   - ratio < 0.8 → `"declining"`
    ///   - else → `"stable"`
    #[must_use]
    fn compute_trend(current: usize, previous: usize) -> &'static str {
        if current == 0 && previous == 0 {
            return "stable";
        }
        if current == 0 {
            return "inactive";
        }
        if previous == 0 {
            return "new";
        }
        #[allow(clippy::cast_precision_loss)]
        let ratio = current as f64 / previous as f64;
        if ratio > 1.2 {
            "rising"
        } else if ratio < 0.8 {
            "declining"
        } else {
            "stable"
        }
    }

    /// Get usage summary for a specific module (all recorded data, no period filter).
    #[must_use]
    pub fn get_module_summary(&self, module_id: &str) -> Option<UsageStats> {
        let data = self.data.lock();
        let md = data.get(module_id)?;
        let records: Vec<UsageRecord> = md.records.values().flatten().cloned().collect();
        // No period → trend is "stable" (no comparison window).
        Some(Self::aggregate_records(
            module_id,
            &records,
            "stable".to_string(),
        ))
    }

    /// Get all usage summaries (all recorded data, no period filter).
    #[must_use]
    pub fn get_all_summaries(&self) -> Vec<UsageStats> {
        let data = self.data.lock();
        data.iter()
            .map(|(mid, md)| {
                let records: Vec<UsageRecord> = md.records.values().flatten().cloned().collect();
                Self::aggregate_records(mid, &records, "stable".to_string())
            })
            .collect()
    }

    /// Get summaries for all modules, optionally filtered to a recent
    /// period (D-27). When `period` is `None`, the full history is summarised
    /// and trend is `"stable"` (no window to compare against).
    #[must_use]
    pub fn get_summary_for_period(&self, period: Option<Duration>) -> Vec<UsageStats> {
        let now = Utc::now();
        let data = self.data.lock();
        match period {
            None => data
                .iter()
                .map(|(mid, md)| {
                    let records: Vec<UsageRecord> =
                        md.records.values().flatten().cloned().collect();
                    Self::aggregate_records(mid, &records, "stable".to_string())
                })
                .collect(),
            Some(delta) => {
                let cutoff = now - delta;
                let prev_cutoff = cutoff - delta;
                data.iter()
                    .map(|(mid, md)| {
                        let current = Self::collect_records_in_window(md, cutoff, now);
                        let previous = Self::collect_records_in_window(md, prev_cutoff, cutoff);
                        let trend = Self::compute_trend(current.len(), previous.len()).to_string();
                        Self::aggregate_records(mid, &current, trend)
                    })
                    .collect()
            }
        }
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
    #[must_use]
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

    /// Compute a percentile (0.0..=1.0) from a sorted latency slice.
    /// Returns 0.0 for empty input. Used by `export_prometheus`.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn percentile_from_sorted(sorted: &[f64], q: f64) -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let rank = (sorted.len() as f64 * q).ceil() as usize;
        sorted[rank.min(sorted.len()).saturating_sub(1)]
    }

    /// Render the collector's data in Prometheus text exposition format.
    ///
    /// Spec: system-modules.md §1.3 normative metrics:
    ///   - `apcore_usage_calls_total{module_id, status}` (counter)
    ///   - `apcore_usage_error_rate{module_id}` (gauge, 0.0–1.0)
    ///   - `apcore_usage_p50/p95/p99_latency_ms{module_id}` (gauges)
    ///
    /// `# HELP` and `# TYPE` lines are always emitted, even when the
    /// collector is empty, so scrape discovery succeeds on cold start.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // call counts comfortably fit in f64
    pub fn export_prometheus(&self) -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        let _ = writeln!(
            out,
            "# HELP apcore_usage_calls_total Total module call count by status"
        );
        let _ = writeln!(out, "# TYPE apcore_usage_calls_total counter");
        let _ = writeln!(
            out,
            "# HELP apcore_usage_error_rate Module error rate (0.0–1.0)"
        );
        let _ = writeln!(out, "# TYPE apcore_usage_error_rate gauge");
        let _ = writeln!(
            out,
            "# HELP apcore_usage_p50_latency_ms Module p50 latency in milliseconds"
        );
        let _ = writeln!(out, "# TYPE apcore_usage_p50_latency_ms gauge");
        let _ = writeln!(
            out,
            "# HELP apcore_usage_p95_latency_ms Module p95 latency in milliseconds"
        );
        let _ = writeln!(out, "# TYPE apcore_usage_p95_latency_ms gauge");
        let _ = writeln!(
            out,
            "# HELP apcore_usage_p99_latency_ms Module p99 latency in milliseconds"
        );
        let _ = writeln!(out, "# TYPE apcore_usage_p99_latency_ms gauge");

        // Snapshot under the lock, drop the guard, then format. Keeps the
        // critical section short and avoids holding the mutex across the
        // sorts and writeln! calls below.
        let snapshot: Vec<(String, u64, u64, Vec<f64>)> = {
            let data = self.data.lock();
            data.iter()
                .map(|(mid, md)| {
                    let mut latencies: Vec<f64> = Vec::new();
                    let mut total: u64 = 0;
                    let mut errors: u64 = 0;
                    for recs in md.records.values() {
                        for r in recs {
                            total += 1;
                            if !r.success {
                                errors += 1;
                            }
                            latencies.push(r.latency_ms);
                        }
                    }
                    (mid.clone(), total, errors, latencies)
                })
                .collect()
        };

        let mut snapshot = snapshot;
        snapshot.sort_by(|a, b| a.0.cmp(&b.0));

        for (mid, total, errors, mut latencies) in snapshot {
            let escaped = escape_label_value(&mid);
            let success = total.saturating_sub(errors);
            let _ = writeln!(
                out,
                "apcore_usage_calls_total{{module_id=\"{escaped}\",status=\"success\"}} {success}"
            );
            let _ = writeln!(
                out,
                "apcore_usage_calls_total{{module_id=\"{escaped}\",status=\"error\"}} {errors}"
            );

            let error_rate = if total == 0 {
                0.0
            } else {
                errors as f64 / total as f64
            };
            let _ = writeln!(
                out,
                "apcore_usage_error_rate{{module_id=\"{escaped}\"}} {error_rate}"
            );

            latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let p50 = Self::percentile_from_sorted(&latencies, 0.50);
            let p95 = Self::percentile_from_sorted(&latencies, 0.95);
            let p99 = Self::percentile_from_sorted(&latencies, 0.99);
            let _ = writeln!(
                out,
                "apcore_usage_p50_latency_ms{{module_id=\"{escaped}\"}} {p50}"
            );
            let _ = writeln!(
                out,
                "apcore_usage_p95_latency_ms{{module_id=\"{escaped}\"}} {p95}"
            );
            let _ = writeln!(
                out,
                "apcore_usage_p99_latency_ms{{module_id=\"{escaped}\"}} {p99}"
            );
        }

        out
    }
}

/// Escape a Prometheus label value per the text exposition spec:
/// backslash, double-quote, and newline are the only characters that need
/// escaping. Module IDs in apcore are restricted to `[a-z0-9._-]`, so this
/// is defense-in-depth — but cheap and worth keeping.
fn escape_label_value(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
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
    #[must_use]
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
