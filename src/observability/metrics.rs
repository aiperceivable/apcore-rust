// APCore Protocol — Metrics collection
// Spec reference: Execution metrics and metrics middleware

use async_trait::async_trait;
use parking_lot::Mutex;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::context::Context;
use crate::errors::ModuleError;
use crate::middleware::base::Middleware;

/// Default histogram bucket boundaries matching Python reference.
pub(crate) const DEFAULT_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0,
];

/// Composite key for metric identification: (name, sorted labels).
type MetricKey = (String, BTreeMap<String, String>);

/// Internal histogram data.
#[derive(Debug, Clone)]
struct HistogramData {
    sum: f64,
    count: u64,
    buckets: Vec<(f64, u64)>, // (upper_bound, cumulative_count)
}

impl HistogramData {
    fn new() -> Self {
        let buckets = DEFAULT_BUCKETS.iter().map(|&b| (b, 0u64)).collect();
        Self {
            sum: 0.0,
            count: 0,
            buckets,
        }
    }

    fn observe(&mut self, value: f64) {
        self.sum += value;
        self.count += 1;
        for bucket in &mut self.buckets {
            if value <= bucket.0 {
                bucket.1 += 1;
            }
        }
    }
}

/// Collects and stores metrics counters and histogram observations.
#[derive(Debug, Clone)]
pub struct MetricsCollector {
    counters: Arc<Mutex<HashMap<MetricKey, f64>>>,
    histograms: Arc<Mutex<HashMap<MetricKey, HistogramData>>>,
}

impl MetricsCollector {
    /// Create a new metrics collector.
    pub fn new() -> Self {
        Self {
            counters: Arc::new(Mutex::new(HashMap::new())),
            histograms: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Format labels into a composite key.
    fn make_key(name: &str, labels: &HashMap<String, String>) -> MetricKey {
        let sorted: BTreeMap<String, String> =
            labels.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        (name.to_string(), sorted)
    }

    /// Increment a counter metric by `amount`.
    pub fn increment(&self, name: &str, labels: HashMap<String, String>, amount: f64) {
        let key = Self::make_key(name, &labels);
        let mut counters = self.counters.lock();
        let entry = counters.entry(key).or_insert(0.0);
        *entry += amount;
    }

    /// Observe a value for a histogram metric.
    pub fn observe(&self, name: &str, labels: HashMap<String, String>, value: f64) {
        let key = Self::make_key(name, &labels);
        let mut histograms = self.histograms.lock();
        let entry = histograms.entry(key).or_insert_with(HistogramData::new);
        entry.observe(value);
    }

    /// Return a snapshot of all current metric values as JSON.
    pub fn snapshot(&self) -> serde_json::Value {
        let counters = self.counters.lock();
        let histograms = self.histograms.lock();

        let mut counters_map = serde_json::Map::new();
        for ((name, labels), value) in counters.iter() {
            let label_str = if labels.is_empty() {
                name.clone()
            } else {
                let label_parts: Vec<String> =
                    labels.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
                format!("{}|{}", name, label_parts.join(","))
            };
            counters_map.insert(label_str, serde_json::json!(*value));
        }

        let mut histograms_map = serde_json::Map::new();
        for ((name, labels), data) in histograms.iter() {
            let label_str = if labels.is_empty() {
                name.clone()
            } else {
                let label_parts: Vec<String> =
                    labels.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
                format!("{}|{}", name, label_parts.join(","))
            };
            histograms_map.insert(
                label_str,
                serde_json::json!({
                    "sum": data.sum,
                    "count": data.count,
                    "buckets": data.buckets.iter().map(|(b, c)| {
                        serde_json::json!({"le": b, "count": c})
                    }).collect::<Vec<_>>()
                }),
            );
        }

        serde_json::json!({
            "counters": counters_map,
            "histograms": histograms_map,
        })
    }

    /// Reset all metrics.
    pub fn reset(&self) {
        self.counters.lock().clear();
        self.histograms.lock().clear();
    }

    /// Export metrics in Prometheus text format.
    pub fn export_prometheus(&self) -> String {
        let mut output = String::new();
        let counters = self.counters.lock();
        let histograms = self.histograms.lock();

        // Export counters
        let mut seen_counter_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for ((name, labels), value) in counters.iter() {
            if seen_counter_names.insert(name.clone()) {
                output.push_str(&format!("# HELP {} Counter metric\n", name));
                output.push_str(&format!("# TYPE {} counter\n", name));
            }
            let label_str = format_prometheus_labels(labels);
            output.push_str(&format!("{}{} {}\n", name, label_str, value));
        }

        // Export histograms
        let mut seen_hist_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for ((name, labels), data) in histograms.iter() {
            if seen_hist_names.insert(name.clone()) {
                output.push_str(&format!("# HELP {} Histogram metric\n", name));
                output.push_str(&format!("# TYPE {} histogram\n", name));
            }
            let base_labels = format_prometheus_labels(labels);
            for (bound, count) in &data.buckets {
                let le_label = if labels.is_empty() {
                    format!("{{le=\"{}\"}}", bound)
                } else {
                    // Insert le into existing labels
                    let inner = &base_labels[1..base_labels.len() - 1]; // strip { }
                    format!("{{{},le=\"{}\"}}", inner, bound)
                };
                output.push_str(&format!("{}_bucket{} {}\n", name, le_label, count));
            }
            // +Inf bucket
            let inf_label = if labels.is_empty() {
                "{le=\"+Inf\"}".to_string()
            } else {
                let inner = &base_labels[1..base_labels.len() - 1];
                format!("{{{},le=\"+Inf\"}}", inner)
            };
            output.push_str(&format!("{}_bucket{} {}\n", name, inf_label, data.count));
            output.push_str(&format!("{}_sum{} {}\n", name, base_labels, data.sum));
            output.push_str(&format!("{}_count{} {}\n", name, base_labels, data.count));
        }

        output
    }

    /// Convenience: increment call counter.
    pub fn increment_calls(&self, module_id: &str, status: &str) {
        let mut labels = HashMap::new();
        labels.insert("module_id".to_string(), module_id.to_string());
        labels.insert("status".to_string(), status.to_string());
        self.increment("apcore_module_calls_total", labels, 1.0);
    }

    /// Convenience: increment error counter.
    pub fn increment_errors(&self, module_id: &str, error_code: &str) {
        let mut labels = HashMap::new();
        labels.insert("module_id".to_string(), module_id.to_string());
        labels.insert("error_code".to_string(), error_code.to_string());
        self.increment("apcore_module_errors_total", labels, 1.0);
    }

    /// Convenience: observe call duration.
    pub fn observe_duration(&self, module_id: &str, duration_secs: f64) {
        let mut labels = HashMap::new();
        labels.insert("module_id".to_string(), module_id.to_string());
        self.observe("apcore_module_duration_seconds", labels, duration_secs);
    }
}

/// Format labels as Prometheus label string: {key="value",...}
fn format_prometheus_labels(labels: &BTreeMap<String, String>) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = labels
        .iter()
        .map(|(k, v)| format!("{}=\"{}\"", k, v))
        .collect();
    format!("{{{}}}", parts.join(","))
}

/// Estimate p99 latency from histogram buckets in a metrics snapshot.
///
/// Expects `buckets` to be a JSON array of `{"le": <f64>, "count": <u64>}` objects
/// with cumulative counts, and `total_count` to be the total number of observations.
///
/// Returns the upper bound (`le`) of the first bucket whose cumulative count
/// reaches or exceeds the 99th-percentile threshold, converted to milliseconds.
/// Returns 0.0 if `total_count` is 0 or `buckets` is empty/missing.
pub(crate) fn estimate_p99_from_histogram(buckets: &[serde_json::Value], total_count: u64) -> f64 {
    if total_count == 0 || buckets.is_empty() {
        return 0.0;
    }
    let target = (total_count as f64 * 0.99).ceil() as u64;
    for bucket in buckets {
        let le = bucket
            .get("le")
            .and_then(|v| v.as_f64())
            .unwrap_or(f64::INFINITY);
        let cnt = bucket.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
        if cnt >= target {
            return le * 1000.0; // seconds -> ms
        }
    }
    0.0
}

/// Estimate p99 latency from a sorted slice of raw latency values (in ms).
///
/// Returns the value at the 99th-percentile index. Returns 0.0 if the slice is empty.
pub(crate) fn estimate_p99_from_sorted(sorted_latencies: &[f64]) -> f64 {
    if sorted_latencies.is_empty() {
        return 0.0;
    }
    let idx = ((sorted_latencies.len() as f64) * 0.99).ceil() as usize;
    sorted_latencies[idx.min(sorted_latencies.len()) - 1]
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Middleware that records execution metrics.
///
/// WARNING: The internal start-time stack is not safe for concurrent use on
/// the same middleware instance. Use separate instances per concurrent pipeline.
#[derive(Debug)]
pub struct MetricsMiddleware {
    collector: MetricsCollector,
    starts: Mutex<HashMap<String, std::time::Instant>>,
}

impl MetricsMiddleware {
    /// Create a new metrics middleware.
    pub fn new(collector: MetricsCollector) -> Self {
        Self {
            collector,
            starts: Mutex::new(HashMap::new()),
        }
    }

    /// Get a reference to the underlying collector.
    pub fn collector(&self) -> &MetricsCollector {
        &self.collector
    }
}

#[async_trait]
impl Middleware for MetricsMiddleware {
    fn name(&self) -> &str {
        "metrics"
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
        _ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        let duration_secs = {
            let mut starts = self.starts.lock();
            starts
                .remove(&_ctx.trace_id)
                .map(|s| s.elapsed().as_secs_f64())
                .unwrap_or(0.0)
        };

        self.collector.increment_calls(module_id, "success");
        self.collector.observe_duration(module_id, duration_secs);

        Ok(None)
    }

    async fn on_error(
        &self,
        module_id: &str,
        _inputs: serde_json::Value,
        _error: &ModuleError,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        let duration_secs = {
            let mut starts = self.starts.lock();
            starts
                .remove(&_ctx.trace_id)
                .map(|s| s.elapsed().as_secs_f64())
                .unwrap_or(0.0)
        };

        let error_code = format!("{:?}", _error.code);
        self.collector.increment_calls(module_id, "error");
        self.collector.increment_errors(module_id, &error_code);
        self.collector.observe_duration(module_id, duration_secs);

        Ok(None)
    }
}
