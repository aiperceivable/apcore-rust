// APCore Protocol — Trace context propagation
// Spec reference: W3C TraceContext / traceparent header support

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;

use crate::context::Context;
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
                format!("Invalid traceparent format: {header}"),
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
    #[must_use]
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

    /// Build a W3C `traceparent` header map from an apcore [`Context`].
    ///
    /// Extracts the `trace_id` from the context (stripping any UUID dashes to
    /// produce 32 lowercase hex characters) and generates a random 8-byte
    /// parent span ID. Returns a header map containing the `"traceparent"` key.
    /// This mirrors `TraceContext.inject(context)` in the Python and TypeScript SDKs.
    pub fn inject<T: serde::Serialize>(context: &Context<T>) -> HashMap<String, String> {
        // Strip dashes: context.trace_id may be a standard UUID string
        // (36 chars with dashes) or already a 32-char hex string.
        let trace_id_hex = context.trace_id.replace('-', "");
        // Use a random parent_id — the context does not carry an active span ref.
        let parent_id = uuid::Uuid::new_v4().simple().to_string()[..16].to_string();
        let traceparent = format!("00-{trace_id_hex}-{parent_id}-01");
        let mut headers = HashMap::new();
        headers.insert("traceparent".to_string(), traceparent);
        headers
    }

    /// Parse the `traceparent` header from a header map.
    ///
    /// Returns `None` if the header is missing or malformed, matching the
    /// behaviour of `TraceContext.extract(headers)` in Python and TypeScript SDKs.
    pub fn extract(headers: &HashMap<String, String>) -> Option<TraceParent> {
        let raw = headers.get("traceparent")?;
        let lower = raw.trim().to_lowercase();
        let caps = TRACEPARENT_RE.captures(&lower)?;
        let version = u8::from_str_radix(&caps[1], 16).ok()?;
        let trace_id = caps[2].to_string();
        let parent_id = caps[3].to_string();
        let trace_flags = u8::from_str_radix(&caps[4], 16).ok()?;
        // Version ff is invalid per W3C spec.
        if version == 0xff {
            return None;
        }
        // All-zero IDs are invalid.
        if trace_id.chars().all(|c| c == '0') || parent_id.chars().all(|c| c == '0') {
            return None;
        }
        Some(TraceParent {
            version,
            trace_id,
            parent_id,
            trace_flags,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Context, Identity};

    fn make_context() -> Context<serde_json::Value> {
        Context::<serde_json::Value>::new(Identity::new(
            "caller".to_string(),
            "user".to_string(),
            vec![],
            HashMap::default(),
        ))
    }

    #[test]
    fn test_inject_returns_traceparent_header() {
        let ctx = make_context();
        let headers = TraceContext::inject(&ctx);
        assert!(
            headers.contains_key("traceparent"),
            "must include traceparent key"
        );
        let tp = headers["traceparent"].clone();
        // Format: 00-<32hex>-<16hex>-01
        assert!(tp.starts_with("00-"), "version must be 00");
        let parts: Vec<&str> = tp.split('-').collect();
        assert_eq!(parts.len(), 4);
        let expected_trace_id = ctx.trace_id.replace('-', "");
        assert_eq!(
            parts[1], expected_trace_id,
            "trace_id must match context trace_id (dashes stripped)"
        );
        assert_eq!(parts[1].len(), 32, "trace_id must be 32 hex chars");
        assert_eq!(parts[2].len(), 16, "parent_id must be 16 hex chars");
        assert_eq!(parts[3], "01", "flags must be 01");
    }

    #[test]
    fn test_extract_valid_header() {
        let mut headers = HashMap::new();
        headers.insert(
            "traceparent".to_string(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_string(),
        );
        let result = TraceContext::extract(&headers);
        assert!(result.is_some(), "valid header must parse");
        let tp = result.unwrap();
        assert_eq!(tp.version, 0);
        assert_eq!(tp.trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(tp.parent_id, "00f067aa0ba902b7");
        assert_eq!(tp.trace_flags, 1);
    }

    #[test]
    fn test_extract_missing_header_returns_none() {
        let headers: HashMap<String, String> = HashMap::new();
        assert!(TraceContext::extract(&headers).is_none());
    }

    #[test]
    fn test_extract_malformed_header_returns_none() {
        let mut headers = HashMap::new();
        headers.insert("traceparent".to_string(), "not-valid".to_string());
        assert!(TraceContext::extract(&headers).is_none());
    }

    #[test]
    fn test_extract_all_zero_trace_id_returns_none() {
        let mut headers = HashMap::new();
        headers.insert(
            "traceparent".to_string(),
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01".to_string(),
        );
        assert!(TraceContext::extract(&headers).is_none());
    }

    #[test]
    fn test_extract_all_zero_parent_id_returns_none() {
        let mut headers = HashMap::new();
        headers.insert(
            "traceparent".to_string(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01".to_string(),
        );
        assert!(TraceContext::extract(&headers).is_none());
    }

    #[test]
    fn test_extract_version_ff_returns_none() {
        let mut headers = HashMap::new();
        headers.insert(
            "traceparent".to_string(),
            "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_string(),
        );
        assert!(TraceContext::extract(&headers).is_none());
    }

    #[test]
    fn test_inject_then_extract_roundtrip() {
        let ctx = make_context();
        let headers = TraceContext::inject(&ctx);
        let tp = TraceContext::extract(&headers).expect("inject output must be extractable");
        assert_eq!(tp.trace_id, ctx.trace_id.replace('-', ""));
        assert_eq!(tp.version, 0);
        assert_eq!(tp.trace_flags, 1);
    }
}
