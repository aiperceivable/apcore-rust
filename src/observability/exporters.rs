// APCore Protocol — Span exporters
// Spec reference: Built-in span export destinations

use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

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
                format!("Failed to serialize span: {}", e),
            )
        })?;
        println!("{}", json);
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ModuleError> {
        Ok(())
    }
}

/// Default maximum spans for InMemoryExporter.
const DEFAULT_MAX_SPANS: usize = 1000;

/// Exports spans to an in-memory buffer for testing.
#[derive(Debug, Clone)]
pub struct InMemoryExporter {
    spans: Arc<Mutex<VecDeque<Span>>>,
    max_spans: usize,
}

impl InMemoryExporter {
    /// Create a new in-memory exporter with default capacity.
    pub fn new() -> Self {
        Self {
            spans: Arc::new(Mutex::new(VecDeque::new())),
            max_spans: DEFAULT_MAX_SPANS,
        }
    }

    /// Create with explicit max spans capacity.
    pub fn with_max_spans(max_spans: usize) -> Self {
        Self {
            spans: Arc::new(Mutex::new(VecDeque::new())),
            max_spans,
        }
    }

    /// Get all exported spans.
    pub fn get_spans(&self) -> Vec<Span> {
        self.spans.lock().unwrap().iter().cloned().collect()
    }

    /// Clear all exported spans.
    pub fn clear(&self) {
        self.spans.lock().unwrap().clear();
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
        let mut spans = self.spans.lock().unwrap();
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
/// Note: Full HTTP transport requires an HTTP client dependency.
/// This is a placeholder that logs the intent.
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

#[async_trait]
impl SpanExporter for OTLPExporter {
    async fn export(&self, span: &Span) -> Result<(), ModuleError> {
        // Placeholder: OTLP export requires an HTTP client (e.g., reqwest).
        // Log the span that would be exported.
        eprintln!(
            "OTLPExporter: would export span {} to {}",
            span.span_id, self.endpoint
        );
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ModuleError> {
        Ok(())
    }
}
