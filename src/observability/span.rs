// APCore Protocol — Span and SpanExporter
// Spec reference: Distributed tracing spans

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::errors::ModuleError;

/// A tracing span representing a unit of work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Span {
    pub trace_id: String,
    pub span_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    pub name: String,
    pub start_time: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub attributes: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub events: Vec<SpanEvent>,
    pub status: SpanStatus,
}

/// An event within a span.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanEvent {
    pub name: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub attributes: HashMap<String, serde_json::Value>,
}

/// Status of a span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanStatus {
    Unset,
    Ok,
    Error,
}

impl Span {
    /// Create a new span.
    pub fn new(name: impl Into<String>, trace_id: impl Into<String>) -> Self {
        Self {
            trace_id: trace_id.into(),
            span_id: uuid::Uuid::new_v4().to_string(),
            parent_span_id: None,
            name: name.into(),
            start_time: Utc::now(),
            end_time: None,
            attributes: HashMap::new(),
            events: vec![],
            status: SpanStatus::Unset,
        }
    }

    /// End the span.
    pub fn end(&mut self) {
        self.end_time = Some(Utc::now());
    }

    /// Add an attribute to the span.
    pub fn set_attribute(&mut self, key: impl Into<String>, value: serde_json::Value) {
        self.attributes.insert(key.into(), value);
    }

    /// Add an event to the span.
    pub fn add_event(&mut self, name: impl Into<String>) {
        self.events.push(SpanEvent {
            name: name.into(),
            timestamp: Utc::now(),
            attributes: HashMap::new(),
        });
    }
}

/// Trait for exporting completed spans.
#[async_trait]
pub trait SpanExporter: Send + Sync + std::fmt::Debug {
    /// Export a single completed span.
    async fn export(&self, span: &Span) -> Result<(), ModuleError>;

    /// Shut down the exporter.
    async fn shutdown(&self) -> Result<(), ModuleError>;
}
