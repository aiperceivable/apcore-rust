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

/// Pre-compiled regex matching a 16-char lowercase hex parent_id (W3C span id).
static PARENT_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-f]{16}$").unwrap());

/// W3C tracestate hard-cap on entry count.
const TRACESTATE_MAX_ENTRIES: usize = 32;

/// Case-insensitive header-key lookup helper.
///
/// HTTP header field names are case-insensitive (RFC 7230 §3.2). Many transport
/// shims hand us a `HashMap<String, String>` whose keys retain whatever casing
/// the upstream layer used (e.g. "Traceparent", "TRACEPARENT"). This helper
/// scans the map once with an `eq_ignore_ascii_case` comparison and returns the
/// first matching value.
fn lookup_header_ci<'a>(
    headers: &'a HashMap<String, String>,
    name: &str,
) -> Option<&'a String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
}

/// Parse the W3C `tracestate` header value into ordered key/value pairs.
///
/// Per W3C Trace Context §3.3.1:
/// * Entries are comma-separated.
/// * Each entry is `key=value`; whitespace around the entry MUST be trimmed.
/// * The list MUST be capped at 32 entries; entries beyond the cap are dropped.
/// * Malformed entries (missing `=`, empty key, empty value) are silently dropped.
fn parse_tracestate(raw: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for entry in raw.split(',') {
        if out.len() >= TRACESTATE_MAX_ENTRIES {
            break;
        }
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((k, v)) = trimmed.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let value = v.trim();
        if key.is_empty() || value.is_empty() {
            continue;
        }
        out.push((key.to_string(), value.to_string()));
    }
    out
}

/// Serialize an ordered tracestate list into a header value.
fn format_tracestate(entries: &[(String, String)]) -> String {
    entries
        .iter()
        .take(TRACESTATE_MAX_ENTRIES)
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",")
}

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

        // INVARIANT: TRACEPARENT_RE guarantees caps[1] and caps[4] are exactly 2 lowercase
        // hex digits, so `u8::from_str_radix(.., 16)` cannot fail.
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
    #[must_use]
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
    #[must_use]
    pub fn new(traceparent: TraceParent) -> Self {
        Self {
            traceparent,
            tracestate: vec![],
            baggage: std::collections::HashMap::new(),
        }
    }

    /// Generate a new root trace context with random IDs.
    #[must_use]
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
    ///
    /// New roots default to `trace_flags = 0x01` (sampled) and emit only the
    /// `traceparent` header. To override the parent span ID, set non-default
    /// trace flags, or attach a tracestate, use [`inject_with_options`].
    ///
    /// [`inject_with_options`]: TraceContext::inject_with_options
    pub fn inject<T: serde::Serialize>(context: &Context<T>) -> HashMap<String, String> {
        Self::inject_with_options(context, None, None, None)
    }

    /// Build a W3C `traceparent` (and optional `tracestate`) header map from
    /// an apcore [`Context`], with optional overrides.
    ///
    /// Arguments:
    /// * `parent_id` — when `Some`, must match `^[0-9a-f]{16}$`. Invalid values
    ///   are ignored and a fresh random parent_id is used instead. When `None`,
    ///   a fresh 16-hex random parent_id is generated.
    /// * `trace_flags` — propagated W3C flag byte. When `None`, defaults to
    ///   `0x01` (sampled) for new roots. Callers that extracted an inbound
    ///   traceparent SHOULD pass that header's `trace_flags` here so the flag
    ///   is propagated rather than hardcoded.
    /// * `tracestate` — when present and non-empty, emitted as the `tracestate`
    ///   header. Capped at 32 entries per W3C §3.3.1.
    pub fn inject_with_options<T: serde::Serialize>(
        context: &Context<T>,
        parent_id: Option<&str>,
        trace_flags: Option<u8>,
        tracestate: Option<&[(String, String)]>,
    ) -> HashMap<String, String> {
        // Strip dashes: context.trace_id may be a standard UUID string
        // (36 chars with dashes) or already a 32-char hex string.
        let trace_id_hex = context.trace_id.replace('-', "");

        let parent_id_hex = match parent_id {
            Some(p) if PARENT_ID_RE.is_match(p) => p.to_string(),
            _ => uuid::Uuid::new_v4().simple().to_string()[..16].to_string(),
        };

        let flags = trace_flags.unwrap_or(0x01);
        let traceparent = format!("00-{trace_id_hex}-{parent_id_hex}-{flags:02x}");

        let mut headers = HashMap::new();
        headers.insert("traceparent".to_string(), traceparent);
        if let Some(entries) = tracestate {
            if !entries.is_empty() {
                let value = format_tracestate(entries);
                if !value.is_empty() {
                    headers.insert("tracestate".to_string(), value);
                }
            }
        }
        headers
    }

    /// Parse the `traceparent` header from a header map.
    ///
    /// Header KEY lookup is case-insensitive (RFC 7230 §3.2): the map may use
    /// any casing for the key (`traceparent`, `Traceparent`, `TRACEPARENT`).
    /// Returns `None` if the header is missing or malformed, matching the
    /// behaviour of `TraceContext.extract(headers)` in Python and TypeScript SDKs.
    pub fn extract(headers: &HashMap<String, String>) -> Option<TraceParent> {
        let raw = lookup_header_ci(headers, "traceparent")?;
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

    /// Parse both `traceparent` and `tracestate` headers into a full
    /// [`TraceContext`].
    ///
    /// Header KEY lookup is case-insensitive. Returns `None` if the
    /// `traceparent` header is missing or malformed; the `tracestate` header
    /// is optional, and malformed entries within it are silently dropped per
    /// W3C §3.3.1.
    pub fn extract_context(headers: &HashMap<String, String>) -> Option<TraceContext> {
        let traceparent = Self::extract(headers)?;
        let tracestate = lookup_header_ci(headers, "tracestate")
            .map(|v| parse_tracestate(v))
            .unwrap_or_default();
        Some(TraceContext {
            traceparent,
            tracestate,
            baggage: std::collections::HashMap::new(),
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
