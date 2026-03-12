// APCore Protocol — Trace context propagation
// Spec reference: W3C TraceContext / traceparent header support

use serde::{Deserialize, Serialize};

/// Parsed W3C traceparent header.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceParent {
    pub version: u8,
    pub trace_id: String,
    pub parent_id: String,
    pub trace_flags: u8,
}

impl TraceParent {
    /// Parse a traceparent header string.
    pub fn parse(header: &str) -> Result<Self, crate::errors::ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Serialize to a traceparent header string.
    pub fn to_header(&self) -> String {
        format!(
            "{:02x}-{}-{}-{:02x}",
            self.version, self.trace_id, self.parent_id, self.trace_flags
        )
    }
}

/// Trace context carrying parent trace info and baggage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceContext {
    pub traceparent: TraceParent,
    #[serde(default)]
    pub tracestate: Vec<(String, String)>,
    #[serde(default)]
    pub baggage: std::collections::HashMap<String, String>,
}

impl TraceContext {
    /// Create a new trace context from a traceparent.
    pub fn new(traceparent: TraceParent) -> Self {
        Self {
            traceparent,
            tracestate: vec![],
            baggage: std::collections::HashMap::new(),
        }
    }

    /// Generate a new root trace context with random IDs.
    pub fn new_root() -> Self {
        // TODO: Implement
        todo!()
    }

    /// Create a child span context.
    pub fn child(&self) -> Self {
        // TODO: Implement
        todo!()
    }
}
