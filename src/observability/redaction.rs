// APCore Protocol — Configurable redaction rules
// Spec reference: observability.md §1.5 Configurable Redaction Rules

use glob::Pattern;
use regex::Regex;
use serde_json::{Map, Value};

/// Default replacement string for redacted values.
pub const DEFAULT_REPLACEMENT: &str = "***REDACTED***";

/// Field names that MUST NEVER be redacted (observability.md §1.5).
///
/// `trace_id`, `caller_id`, and `module_id` are required for observability
/// correlation and must always appear in logs unmodified.
pub const NEVER_REDACT_FIELDS: &[&str] = &["trace_id", "caller_id", "module_id"];

/// Runtime-configurable redaction rules layered on top of schema-level
/// (`x-sensitive`) annotations.
#[derive(Debug, Clone, Default)]
pub struct RedactionConfig {
    field_patterns: Vec<Pattern>,
    value_patterns: Vec<Regex>,
    replacement: String,
}

impl RedactionConfig {
    /// Create an empty redaction config (no rules, default replacement).
    #[must_use]
    pub fn new() -> Self {
        Self {
            field_patterns: Vec::new(),
            value_patterns: Vec::new(),
            replacement: DEFAULT_REPLACEMENT.to_string(),
        }
    }

    /// Builder entry point.
    #[must_use]
    pub fn builder() -> RedactionConfigBuilder {
        RedactionConfigBuilder::default()
    }

    /// Apply both field-name and value-pattern rules to a JSON value in place.
    /// Fields named in [`NEVER_REDACT_FIELDS`] are excluded.
    pub fn redact(&self, value: &mut Value) {
        self.redact_inner(value, /*field_name=*/ None);
    }

    fn redact_inner(&self, value: &mut Value, field_name: Option<&str>) {
        // Field-name rule applies based on the field this value is bound to.
        if let Some(name) = field_name {
            if !NEVER_REDACT_FIELDS.contains(&name) && self.field_matches(name) {
                *value = Value::String(self.replacement.clone());
                return;
            }
        }

        match value {
            Value::Object(map) => {
                self.redact_object(map);
            }
            Value::Array(arr) => {
                for item in arr.iter_mut() {
                    self.redact_inner(item, None);
                }
            }
            Value::String(s) => {
                if self.value_matches(s) {
                    s.clone_from(&self.replacement);
                }
            }
            _ => {}
        }
    }

    fn redact_object(&self, map: &mut Map<String, Value>) {
        let keys: Vec<String> = map.keys().cloned().collect();
        for key in keys {
            let preserved = NEVER_REDACT_FIELDS.contains(&key.as_str());
            if !preserved && self.field_matches(&key) {
                if let Some(slot) = map.get_mut(&key) {
                    *slot = Value::String(self.replacement.clone());
                }
                continue;
            }
            if let Some(child) = map.get_mut(&key) {
                self.redact_inner(child, Some(&key));
            }
        }
    }

    /// Check whether a field name matches any glob pattern.
    #[must_use]
    pub fn field_matches(&self, name: &str) -> bool {
        self.field_patterns.iter().any(|p| p.matches(name))
    }

    /// Check whether a value matches any value regex.
    #[must_use]
    pub fn value_matches(&self, value: &str) -> bool {
        self.value_patterns.iter().any(|r| r.is_match(value))
    }

    /// The configured replacement string.
    #[must_use]
    pub fn replacement(&self) -> &str {
        &self.replacement
    }
}

/// Builder for [`RedactionConfig`]. Returns errors for malformed patterns.
#[derive(Debug, Default)]
pub struct RedactionConfigBuilder {
    field_patterns: Vec<String>,
    value_patterns: Vec<String>,
    replacement: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RedactionConfigError {
    #[error("invalid field glob pattern '{pattern}': {source}")]
    InvalidFieldPattern {
        pattern: String,
        #[source]
        source: glob::PatternError,
    },
    #[error("invalid value regex '{pattern}': {source}")]
    InvalidValuePattern {
        pattern: String,
        #[source]
        source: regex::Error,
    },
}

impl RedactionConfigBuilder {
    #[must_use]
    pub fn field_patterns<I, S>(mut self, patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.field_patterns
            .extend(patterns.into_iter().map(Into::into));
        self
    }

    #[must_use]
    pub fn value_patterns<I, S>(mut self, patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.value_patterns
            .extend(patterns.into_iter().map(Into::into));
        self
    }

    #[must_use]
    pub fn replacement(mut self, replacement: impl Into<String>) -> Self {
        self.replacement = Some(replacement.into());
        self
    }

    /// Compile the patterns. Returns an error if any glob/regex is malformed.
    pub fn try_build(self) -> Result<RedactionConfig, RedactionConfigError> {
        let field_patterns = self
            .field_patterns
            .into_iter()
            .map(|p| {
                Pattern::new(&p).map_err(|source| RedactionConfigError::InvalidFieldPattern {
                    pattern: p,
                    source,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let value_patterns = self
            .value_patterns
            .into_iter()
            .map(|p| {
                Regex::new(&p).map_err(|source| RedactionConfigError::InvalidValuePattern {
                    pattern: p,
                    source,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RedactionConfig {
            field_patterns,
            value_patterns,
            replacement: self
                .replacement
                .unwrap_or_else(|| DEFAULT_REPLACEMENT.to_string()),
        })
    }

    /// Build, panicking on invalid patterns. Prefer [`Self::try_build`] when
    /// patterns come from external configuration.
    #[must_use]
    pub fn build(self) -> RedactionConfig {
        self.try_build().expect("invalid redaction pattern")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn field_glob_redacts_matching_field() {
        let cfg = RedactionConfig::builder()
            .field_patterns(["*password*"])
            .build();
        let mut payload = json!({
            "username": "alice",
            "user_password": "hunter2",
        });
        cfg.redact(&mut payload);
        assert_eq!(payload["username"], json!("alice"));
        assert_eq!(payload["user_password"], json!(DEFAULT_REPLACEMENT));
    }

    #[test]
    fn value_regex_redacts_matching_value() {
        let cfg = RedactionConfig::builder()
            .value_patterns([r"^Bearer .*"])
            .build();
        let mut payload = json!({
            "url": "https://api.example.com/data",
            "authorization": "Bearer abc123xyz",
        });
        cfg.redact(&mut payload);
        assert_eq!(payload["authorization"], json!(DEFAULT_REPLACEMENT));
        assert_eq!(payload["url"], json!("https://api.example.com/data"));
    }

    #[test]
    fn never_redact_fields_are_preserved() {
        let cfg = RedactionConfig::builder()
            .field_patterns(["*"]) // would otherwise match everything
            .build();
        let mut payload = json!({
            "trace_id": "trace-1",
            "caller_id": "api.gateway",
            "module_id": "executor.auth",
            "other_field": "secret",
        });
        cfg.redact(&mut payload);
        assert_eq!(payload["trace_id"], json!("trace-1"));
        assert_eq!(payload["caller_id"], json!("api.gateway"));
        assert_eq!(payload["module_id"], json!("executor.auth"));
        assert_eq!(payload["other_field"], json!(DEFAULT_REPLACEMENT));
    }

    #[test]
    fn invalid_regex_returns_error() {
        let result = RedactionConfig::builder()
            .value_patterns(["[invalid"])
            .try_build();
        assert!(result.is_err());
    }
}
