// APCore Protocol — Trace context propagation
// Spec reference: W3C TraceContext / traceparent header support

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

use crate::errors::{ErrorCode, ModuleError};

/// Pre-compiled regex for traceparent header parsing.
static TRACEPARENT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([0-9a-f]{2})-([0-9a-f]{32})-([0-9a-f]{16})-([0-9a-f]{2})$").unwrap()
});

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
    pub fn parse(header: &str) -> Result<Self, ModuleError> {
        let caps = TRACEPARENT_RE.captures(header).ok_or_else(|| {
            ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                format!("Invalid traceparent format: {}", header),
            )
        })?;

        let version = u8::from_str_radix(&caps[1], 16).unwrap();
        let trace_id = caps[2].to_string();
        let parent_id = caps[3].to_string();
        let trace_flags = u8::from_str_radix(&caps[4], 16).unwrap();

        // Version ff is invalid
        if version == 0xff {
            return Err(ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                "Invalid traceparent version: ff".to_string(),
            ));
        }

        // All-zero trace_id is invalid
        if trace_id.chars().all(|c| c == '0') {
            return Err(ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                "Invalid traceparent: trace_id is all zeros".to_string(),
            ));
        }

        // All-zero parent_id is invalid
        if parent_id.chars().all(|c| c == '0') {
            return Err(ModuleError::new(
                ErrorCode::GeneralInvalidInput,
                "Invalid traceparent: parent_id is all zeros".to_string(),
            ));
        }

        Ok(Self {
            version,
            trace_id,
            parent_id,
            trace_flags,
        })
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
        let trace_id = uuid::Uuid::new_v4().simple().to_string();
        let parent_id = uuid::Uuid::new_v4().simple().to_string()[..16].to_string();

        Self {
            traceparent: TraceParent {
                version: 0,
                trace_id,
                parent_id,
                trace_flags: 1,
            },
            tracestate: vec![],
            baggage: std::collections::HashMap::new(),
        }
    }

    /// Create a child span context.
    pub fn child(&self) -> Self {
        let parent_id = uuid::Uuid::new_v4().simple().to_string()[..16].to_string();

        Self {
            traceparent: TraceParent {
                version: self.traceparent.version,
                trace_id: self.traceparent.trace_id.clone(),
                parent_id,
                trace_flags: self.traceparent.trace_flags,
            },
            tracestate: self.tracestate.clone(),
            baggage: self.baggage.clone(),
        }
    }
}
