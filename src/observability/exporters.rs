// APCore Protocol — Span exporters
// Spec reference: Built-in span export destinations

use async_trait::async_trait;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;

use super::span::{Span, SpanExporter};
use crate::errors::ModuleError;

/// Exports spans to stdout as JSON.
#[derive(Debug)]
pub struct StdoutExporter;

#[async_trait]
impl SpanExporter for StdoutExporter {
    async fn export(&self, span: &Span) -> Result<(), ModuleError> {
        let json = serde_json::to_string(span).map_err(|e| {
            ModuleError::new(
                crate::errors::ErrorCode::GeneralInternalError,
                format!("Failed to serialize span: {e}"),
            )
        })?;
        // Route through tracing so the span line integrates with the
        // application's tracing-subscriber configuration and does not
        // bypass log aggregation pipelines.
        tracing::info!(target: "apcore.span", span = %json);
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ModuleError> {
        Ok(())
    }
}

/// Default maximum spans for `InMemoryExporter`.
const DEFAULT_MAX_SPANS: usize = 1000;

/// Exports spans to an in-memory buffer for testing.
#[derive(Debug, Clone)]
pub struct InMemoryExporter {
    spans: Arc<Mutex<VecDeque<Span>>>,
    max_spans: usize,
}

impl InMemoryExporter {
    /// Create a new in-memory exporter with default capacity.
    #[must_use]
    pub fn new() -> Self {
        Self {
            spans: Arc::new(Mutex::new(VecDeque::new())),
            max_spans: DEFAULT_MAX_SPANS,
        }
    }

    /// Create with explicit max spans capacity.
    #[must_use]
    pub fn with_max_spans(max_spans: usize) -> Self {
        Self {
            spans: Arc::new(Mutex::new(VecDeque::new())),
            max_spans,
        }
    }

    /// Get all exported spans.
    #[must_use]
    pub fn get_spans(&self) -> Vec<Span> {
        self.spans.lock().iter().cloned().collect()
    }

    /// Clear all exported spans.
    pub fn clear(&self) {
        self.spans.lock().clear();
    }
}

impl Default for InMemoryExporter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SpanExporter for InMemoryExporter {
    async fn export(&self, span: &Span) -> Result<(), ModuleError> {
        let mut spans = self.spans.lock();
        spans.push_back(span.clone());
        while spans.len() > self.max_spans {
            spans.pop_front();
        }
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ModuleError> {
        Ok(())
    }
}

/// Exports spans to an OTLP-compatible endpoint.
#[derive(Debug)]
pub struct OTLPExporter {
    pub endpoint: String,
}

impl OTLPExporter {
    /// Create a new OTLP exporter.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }
}

/// Convert a Span to OTLP JSON format.
#[cfg(feature = "events")]
fn span_to_otlp(span: &Span) -> serde_json::Value {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    // intentional: nanosecond timestamps fit in u64 for dates post-1970
    let start_ns = (span.start_time * 1_000_000_000.0) as u64;
    let mut otlp_span = serde_json::json!({
        "traceId": span.trace_id,
        "spanId": span.span_id,
        "name": span.name,
        "startTimeUnixNano": start_ns,
        "status": match span.status {
            super::span::SpanStatus::Ok => serde_json::json!({"code": 1}),
            super::span::SpanStatus::Error => serde_json::json!({"code": 2}),
            super::span::SpanStatus::Unset => serde_json::json!({"code": 0}),
        },
        "attributes": span.attributes.iter().map(|(k, v)| {
            serde_json::json!({
                "key": k,
                "value": { "stringValue": v.to_string() }
            })
        }).collect::<Vec<_>>(),
    });
    if let Some(ref parent_id) = span.parent_span_id {
        otlp_span["parentSpanId"] = serde_json::json!(parent_id);
    }
    if let Some(end_time) = span.end_time {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        // intentional: nanosecond timestamps fit in u64 for dates post-1970
        let end_ns = (end_time * 1_000_000_000.0) as u64;
        otlp_span["endTimeUnixNano"] = serde_json::json!(end_ns);
    }
    otlp_span
}

#[cfg(feature = "events")]
#[async_trait]
impl SpanExporter for OTLPExporter {
    async fn export(&self, span: &Span) -> Result<(), ModuleError> {
        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "resourceSpans": [{
                "scopeSpans": [{
                    "spans": [span_to_otlp(span)]
                }]
            }]
        });
        client
            .post(format!("{}/v1/traces", self.endpoint))
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                ModuleError::new(
                    crate::errors::ErrorCode::GeneralInternalError,
                    format!("OTLP export failed: {e}"),
                )
            })?;
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ModuleError> {
        Ok(())
    }
}

#[cfg(not(feature = "events"))]
#[async_trait]
impl SpanExporter for OTLPExporter {
    async fn export(&self, _span: &Span) -> Result<(), ModuleError> {
        // Without the `events` feature, export is a silent no-op.
        // No network call is made; spans are discarded.
        tracing::warn!(
            "OTLPExporter::export called but the `events` feature is not enabled; span discarded"
        );
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ModuleError> {
        Ok(())
    }
}
