// APCore Protocol — Span exporters
// Spec reference: Built-in span export destinations

use async_trait::async_trait;
use std::sync::{Arc, Mutex};

use crate::errors::ModuleError;
use super::span::{Span, SpanExporter};

/// Exports spans to stdout.
#[derive(Debug)]
pub struct StdoutExporter;

#[async_trait]
impl SpanExporter for StdoutExporter {
    async fn export(&self, _spans: &[Span]) -> Result<(), ModuleError> {
        // TODO: Implement — print spans as JSON to stdout
        todo!()
    }

    async fn shutdown(&self) -> Result<(), ModuleError> {
        Ok(())
    }
}

/// Exports spans to an in-memory buffer for testing.
#[derive(Debug, Clone)]
pub struct InMemoryExporter {
    spans: Arc<Mutex<Vec<Span>>>,
}

impl InMemoryExporter {
    /// Create a new in-memory exporter.
    pub fn new() -> Self {
        Self {
            spans: Arc::new(Mutex::new(vec![])),
        }
    }

    /// Get all exported spans.
    pub fn get_spans(&self) -> Vec<Span> {
        self.spans.lock().unwrap().clone()
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
    async fn export(&self, spans: &[Span]) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
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

#[async_trait]
impl SpanExporter for OTLPExporter {
    async fn export(&self, _spans: &[Span]) -> Result<(), ModuleError> {
        // TODO: Implement — HTTP POST to OTLP endpoint
        todo!()
    }

    async fn shutdown(&self) -> Result<(), ModuleError> {
        Ok(())
    }
}
