// APCore Protocol — Configurable redaction rules
// Spec reference: observability.md §1.5 Configurable Redaction Rules

use glob::Pattern;
use regex::{Regex, RegexBuilder};
use serde_json::{Map, Value};

use crate::config::Config;

/// Default replacement string for redacted values.
pub const DEFAULT_REPLACEMENT: &str = "***REDACTED***";

/// Field names that MUST NEVER be redacted (observability.md §1.5, D-54).
///
/// Correlation identifiers `trace_id`, `caller_id`, `target_id`, `module_id`,
/// and `span_id` MUST always appear in logs unmodified, regardless of any
/// `sensitive_keys` content.
pub const NEVER_REDACT_FIELDS: &[&str] =
    &["trace_id", "caller_id", "target_id", "module_id", "span_id"];

/// Default sensitive-key glob patterns applied when the user has not supplied
/// `observability.redaction.sensitive_keys` in their Config. Issue #43 §5.
///
/// User-supplied entries are merged into this list rather than replacing it,
/// matching apcore-python's `_DEFAULT_SENSITIVE_KEYS` semantics. Includes
/// snake_case, camelCase, and lowercase parity (`api_key` / `apiKey` /
/// `apikey`) so common framework conventions are covered out of the box.
pub const DEFAULT_SENSITIVE_KEYS: &[&str] = &[
    "_secret_*",
    "password",
    "passwd",
    "secret",
    "token",
    "api_key",
    "apikey",
    "apiKey",
    "access_key",
    "private_key",
    "authorization",
    "auth",
    "credential",
    "cookie",
    "session",
    "bearer",
];

/// Runtime-configurable redaction rules layered on top of schema-level
/// (`x-sensitive`) annotations.
#[derive(Debug, Clone, Default)]
pub struct RedactionConfig {
    /// Compiled glob patterns (entries containing `*`/`?`/`[`).
    field_patterns: Vec<Pattern>,
    /// Plain substring patterns, pre-normalized via [`normalize_key_for_match`].
    field_substrings: Vec<String>,
    value_patterns: Vec<Regex>,
    replacement: String,
}

/// Normalize a key or pattern for cross-separator, case-insensitive substring
/// matching (D-54). Lower-cases the input, treats `-` / `_` / whitespace as
/// equivalent, and inserts an `_` at every `lowercase->uppercase` boundary so
/// camelCase keys like `AccessKey` match the `access_key` substring pattern.
#[must_use]
pub fn normalize_key_for_match(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut prev_lower = false;
    for ch in s.chars() {
        if ch.is_ascii_uppercase() && prev_lower {
            out.push('_');
        }
        out.push(ch);
        prev_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
    }
    out.to_lowercase()
        .replace('-', "_")
        .replace(char::is_whitespace, "_")
}

impl RedactionConfig {
    /// Create an empty redaction config (no rules, default replacement).
    #[must_use]
    pub fn new() -> Self {
        Self {
            field_patterns: Vec::new(),
            field_substrings: Vec::new(),
            value_patterns: Vec::new(),
            replacement: DEFAULT_REPLACEMENT.to_string(),
        }
    }

    /// Construct a redaction config seeded with the canonical default
    /// `sensitive_keys` list (D-54). The 16-entry list is the spec-defined
    /// superset shipped by every SDK when no operator override is supplied.
    /// Operator overrides MUST replace this list, not merge.
    #[must_use]
    pub fn with_default_sensitive_keys() -> Self {
        let mut cfg = Self::new();
        for key in DEFAULT_SENSITIVE_KEYS {
            cfg.add_sensitive_key(key);
        }
        cfg
    }

    /// Construct a redaction config with the canonical default sensitive_keys
    /// list. Equivalent to [`Self::with_default_sensitive_keys`]; provided so
    /// callers can write `RedactionConfig::default()` and get spec-canonical
    /// behavior matching `apcore-python.RedactionConfig.default()`.
    #[must_use]
    pub fn defaults() -> Self {
        Self::with_default_sensitive_keys()
    }

    /// Append one entry to the matcher. Glob patterns (containing `*`, `?`,
    /// or `[`) compile to a [`glob::Pattern`]; bare strings are stored as
    /// pre-normalized substrings.
    fn add_sensitive_key(&mut self, key: &str) {
        if key.is_empty() {
            return;
        }
        if key.contains(['*', '?', '[']) {
            match Pattern::new(key) {
                Ok(p) => self.field_patterns.push(p),
                Err(e) => tracing::warn!(
                    pattern = %key,
                    error = %e,
                    "Skipping invalid sensitive_keys glob"
                ),
            }
        } else {
            self.field_substrings.push(normalize_key_for_match(key));
        }
    }

    /// Builder entry point.
    #[must_use]
    pub fn builder() -> RedactionConfigBuilder {
        RedactionConfigBuilder::default()
    }

    /// Build a redaction config from `observability.redaction.*` keys in a
    /// loaded [`Config`]. Issue #43 §5 — replaces the legacy hardcoded
    /// `_secret_` prefix with a fully Config-driven policy.
    ///
    /// Reads:
    ///   - `observability.redaction.sensitive_keys: Vec<String>` — glob
    ///     patterns applied to field NAMES. User entries are unioned with
    ///     [`DEFAULT_SENSITIVE_KEYS`] (the canonical superset: `_secret_*`,
    ///     `password`, `passwd`, `secret`, `token`, `api_key`, `apikey`,
    ///     `apiKey`, `access_key`, `private_key`, `authorization`, `auth`,
    ///     `credential`, `cookie`, `session`, `bearer`).
    ///   - `observability.redaction.regex_patterns: Vec<String>` — regular
    ///     expressions applied to string VALUES. Compiled with
    ///     [`RegexBuilder::case_insensitive(true)`].
    ///   - `observability.redaction.replacement: String` — replacement string
    ///     (defaults to [`DEFAULT_REPLACEMENT`]).
    ///
    /// Malformed patterns are logged at `warn` and skipped — a typo in one
    /// rule MUST NOT disable redaction for every other rule.
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        let mut cfg = Self::new();

        // Default sensitive keys are always applied.
        for default in DEFAULT_SENSITIVE_KEYS {
            cfg.add_sensitive_key(default);
        }

        // User-supplied sensitive_keys (additive — see D-54: operators that
        // need replace-semantics should construct via the builder directly).
        if let Some(value) = config.get("observability.redaction.sensitive_keys") {
            if let Some(arr) = value.as_array() {
                for entry in arr {
                    if let Some(s) = entry.as_str() {
                        cfg.add_sensitive_key(s);
                    }
                }
            }
        }

        let mut value_patterns: Vec<Regex> = Vec::new();
        if let Some(value) = config.get("observability.redaction.regex_patterns") {
            if let Some(arr) = value.as_array() {
                for entry in arr {
                    if let Some(s) = entry.as_str() {
                        match RegexBuilder::new(s).case_insensitive(true).build() {
                            Ok(r) => value_patterns.push(r),
                            Err(e) => tracing::warn!(
                                pattern = %s,
                                error = %e,
                                "Skipping invalid regex_patterns entry"
                            ),
                        }
                    }
                }
            }
        }

        let replacement = config
            .get("observability.redaction.replacement")
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| DEFAULT_REPLACEMENT.to_string());

        cfg.value_patterns = value_patterns;
        cfg.replacement = replacement;
        cfg
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
            Value::String(s) if self.value_matches(s) => {
                s.clone_from(&self.replacement);
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

    /// Check whether a field name matches any sensitive-key entry. Glob
    /// patterns are evaluated case-insensitively against the raw key; bare
    /// substrings are evaluated against the normalized form (lower-cased,
    /// `-`/whitespace mapped to `_`, and an `_` inserted at every
    /// `lowercase->uppercase` boundary so `AccessKey` matches `access_key`).
    #[must_use]
    pub fn field_matches(&self, name: &str) -> bool {
        if self.field_patterns.iter().any(|p| {
            // Try the raw name and the lowered form so legacy globs like
            // `_secret_*` still match `_secret_token` while case-insensitive
            // operator entries like `password*` match `Password123`.
            p.matches(name) || p.matches(&name.to_lowercase())
        }) {
            return true;
        }
        if self.field_substrings.is_empty() {
            return false;
        }
        let normalized = normalize_key_for_match(name);
        self.field_substrings
            .iter()
            .any(|sub| normalized.contains(sub))
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
    sensitive_keys: Vec<String>,
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

    /// Append `sensitive_keys` entries (D-54). Each entry is auto-detected:
    /// strings containing `*`, `?`, or `[` are compiled as globs; bare
    /// strings become case-insensitive substring matchers (with separator +
    /// camelCase normalization). Operator-supplied lists MUST replace the
    /// canonical default — call [`RedactionConfig::with_default_sensitive_keys`]
    /// if you want both.
    #[must_use]
    pub fn sensitive_keys<I, S>(mut self, keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.sensitive_keys.extend(keys.into_iter().map(Into::into));
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
        let mut cfg = RedactionConfig {
            field_patterns,
            field_substrings: Vec::new(),
            value_patterns,
            replacement: self
                .replacement
                .unwrap_or_else(|| DEFAULT_REPLACEMENT.to_string()),
        };
        for key in self.sensitive_keys {
            cfg.add_sensitive_key(&key);
        }
        Ok(cfg)
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
