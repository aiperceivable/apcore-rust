// APCore Protocol — Schema hardening utilities (Issue #44, PROTOCOL_SPEC §4.15).
//
// Provides:
// - `content_hash` — SHA-256 of canonical (sorted-keys) JSON; used for content-addressable caching.
// - `format_warnings` — opt-in semantic format check (date-time, email, uri, …) that emits
//   non-fatal warnings rather than hard errors (SHOULD-level enforcement).

use std::net::{Ipv4Addr, Ipv6Addr};

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// SHA-256 hex digest (lowercase, 64 chars) of the canonical-JSON form of `schema`.
///
/// Canonical form: object keys are recursively sorted; primitive serialization is
/// the same as `serde_json::to_string`. Two byte-equivalent schemas with different
/// key orderings hash to the same digest — this is the deduplication invariant
/// required by PROTOCOL_SPEC §4.15.5.
#[must_use]
pub fn content_hash(schema: &Value) -> String {
    let canonical = canonical_json(schema);
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Serialize a JSON value with object keys sorted at every level. Output uses the
/// same compact form as `serde_json::to_string` (no whitespace).
fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            out.push('{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                // INVARIANT: serializing a string value via serde_json never fails.
                out.push_str(&serde_json::to_string(key).expect("key is a String"));
                out.push(':');
                // INVARIANT: every iterated key is present in `map`.
                write_canonical(&map[*key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => {
            // INVARIANT: serializing a primitive JSON value never fails.
            out.push_str(&serde_json::to_string(other).expect("primitive serialization"));
        }
    }
}

/// A non-fatal format-mismatch warning produced by [`format_warnings`].
///
/// Format enforcement is SHOULD-level per PROTOCOL_SPEC §4.15.4 — invalid
/// `format` values do not fail validation; they surface here so callers can
/// log or surface them as warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatWarning {
    /// JSON pointer to the offending field, e.g. `/occurred_at`.
    pub path: String,
    /// The declared format, e.g. `date-time`.
    pub format: String,
    /// The string value that did not parse as the declared format.
    pub value: String,
}

/// Walk `data` against `schema` and return one [`FormatWarning`] for each
/// string field whose declared `format` does not parse.
///
/// Unmapped formats are ignored. Non-string values are ignored (per Draft 2020-12,
/// `format` only constrains strings). The traversal mirrors the Python reference
/// implementation in `apcore.schema.hardening._check_formats_and_warn`.
#[must_use]
pub fn format_warnings(data: &Value, schema: &Value) -> Vec<FormatWarning> {
    let mut warnings = Vec::new();
    walk_format(data, schema, "", &mut warnings);
    warnings
}

fn walk_format(data: &Value, schema: &Value, path: &str, out: &mut Vec<FormatWarning>) {
    let Some(schema_obj) = schema.as_object() else {
        return;
    };

    if let (Some(fmt), Some(s)) = (
        schema_obj.get("format").and_then(|v| v.as_str()),
        data.as_str(),
    ) {
        if !check_format(fmt, s) {
            out.push(FormatWarning {
                path: if path.is_empty() {
                    "/".to_string()
                } else {
                    path.to_string()
                },
                format: fmt.to_string(),
                value: s.to_string(),
            });
        }
    }

    if let (Some(props), Some(obj)) = (
        schema_obj.get("properties").and_then(|p| p.as_object()),
        data.as_object(),
    ) {
        for (name, prop_schema) in props {
            if let Some(value) = obj.get(name) {
                let child_path = format!("{path}/{name}");
                walk_format(value, prop_schema, &child_path, out);
            }
        }
    }

    if let (Some(items_schema), Some(arr)) = (schema_obj.get("items"), data.as_array()) {
        for (i, item) in arr.iter().enumerate() {
            let child_path = format!("{path}/{i}");
            walk_format(item, items_schema, &child_path, out);
        }
    }
}

/// Returns `true` when `value` parses as a member of the declared `format`,
/// `false` when it does not. Unmapped formats return `true` (no opinion).
fn check_format(format: &str, value: &str) -> bool {
    match format {
        "date-time" => {
            DateTime::parse_from_rfc3339(value).is_ok() || value.parse::<DateTime<Utc>>().is_ok()
        }
        "date" => NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok(),
        "time" => {
            NaiveTime::parse_from_str(value, "%H:%M:%S").is_ok()
                || NaiveTime::parse_from_str(value, "%H:%M:%S%.f").is_ok()
        }
        "uuid" => Uuid::parse_str(value).is_ok(),
        "ipv4" => value.parse::<Ipv4Addr>().is_ok(),
        "ipv6" => value.parse::<Ipv6Addr>().is_ok(),
        "email" => is_email(value),
        "uri" => is_uri(value),
        _ => true,
    }
}

fn is_email(value: &str) -> bool {
    // Mirrors apcore-python: ^[^@\s]+@[^@\s]+\.[^@\s]+$
    let bytes = value.as_bytes();
    let Some(at_idx) = value.find('@') else {
        return false;
    };
    if at_idx == 0 || at_idx == bytes.len() - 1 {
        return false;
    }
    let (local, domain_with_at) = value.split_at(at_idx);
    let domain = &domain_with_at[1..];
    if local.contains(char::is_whitespace) || domain.contains(char::is_whitespace) {
        return false;
    }
    if local.contains('@') || domain.contains('@') {
        return false;
    }
    let Some(dot_idx) = domain.find('.') else {
        return false;
    };
    if dot_idx == 0 || dot_idx == domain.len() - 1 {
        return false;
    }
    true
}

fn is_uri(value: &str) -> bool {
    // Mirrors apcore-python: scheme://… requires alpha + alnum/+/-/. then "://"
    let Some(scheme_end) = value.find("://") else {
        return false;
    };
    let scheme = &value[..scheme_end];
    if scheme.is_empty() {
        return false;
    }
    let mut chars = scheme.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_content_hash_canonical_form_is_key_order_invariant() {
        let a = json!({ "type": "object", "required": ["name"], "properties": { "name": { "type": "string" } } });
        let b = json!({ "properties": { "name": { "type": "string" } }, "required": ["name"], "type": "object" });
        assert_eq!(content_hash(&a), content_hash(&b));
    }

    #[test]
    fn test_content_hash_distinguishes_different_schemas() {
        let a = json!({ "type": "object", "properties": { "name": { "type": "string" } } });
        let b = json!({ "type": "object", "properties": { "age": { "type": "integer" } } });
        assert_ne!(content_hash(&a), content_hash(&b));
    }

    #[test]
    fn test_content_hash_empty_object_is_stable() {
        let a = json!({});
        let b = json!({});
        let h = content_hash(&a);
        assert_eq!(h.len(), 64);
        assert_eq!(h, content_hash(&b));
    }

    #[test]
    fn test_format_warnings_invalid_datetime_emits_warning() {
        let schema = json!({
            "type": "object",
            "properties": { "occurred_at": { "type": "string", "format": "date-time" } }
        });
        let data = json!({ "occurred_at": "not-a-date" });
        let warnings = format_warnings(&data, &schema);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].format, "date-time");
        assert_eq!(warnings[0].path, "/occurred_at");
    }

    #[test]
    fn test_format_warnings_valid_datetime_no_warning() {
        let schema = json!({
            "type": "object",
            "properties": { "occurred_at": { "type": "string", "format": "date-time" } }
        });
        let data = json!({ "occurred_at": "2026-04-28T12:00:00Z" });
        assert!(format_warnings(&data, &schema).is_empty());
    }

    #[test]
    fn test_format_warnings_unmapped_format_silent() {
        let schema = json!({
            "type": "object",
            "properties": { "exotic": { "type": "string", "format": "made-up-format" } }
        });
        let data = json!({ "exotic": "anything" });
        assert!(format_warnings(&data, &schema).is_empty());
    }

    #[test]
    fn test_check_format_email() {
        assert!(check_format("email", "alice@example.com"));
        assert!(!check_format("email", "not-an-email"));
        assert!(!check_format("email", "@nope.com"));
        assert!(!check_format("email", "bare@nodot"));
    }

    #[test]
    fn test_check_format_uri() {
        assert!(check_format("uri", "https://example.com/path"));
        assert!(check_format("uri", "ftp://example.org"));
        assert!(!check_format("uri", "not-a-uri"));
        assert!(!check_format("uri", "://missing-scheme"));
    }
}
