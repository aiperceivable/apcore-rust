// APCore Protocol — Schema validator (Issue #44, PROTOCOL_SPEC §4.15).
//
// Wraps `jsonschema::Validator` (Draft 2020-12) so anyOf/oneOf/allOf/not, recursive
// `$ref`, numerical/string constraints and format keyword handling are all delegated
// to a battle-tested implementation. Compiled validators are cached by SHA-256 of
// the canonical-JSON form of the schema so repeated validation against the same
// schema (or two byte-equivalent copies) only pays the compile cost once.

use std::collections::HashMap;
use std::sync::Arc;

use jsonschema::{error::ValidationErrorKind, Validator};
use parking_lot::Mutex;
use serde_json::Value;

use crate::errors::{ErrorCode, ModuleError, SchemaValidationError};
use crate::module::ValidationResult;
use crate::schema::hardening::{content_hash, format_warnings, FormatWarning};

/// Validates JSON values against JSON Schema documents (Draft 2020-12).
#[derive(Debug, Default)]
pub struct SchemaValidator {
    cache: Arc<Mutex<HashMap<String, Arc<Validator>>>>,
}

/// Outcome of [`SchemaValidator::validate_detailed`] — keeps richer error metadata
/// than the legacy [`ValidationResult`] without changing the existing public surface.
#[derive(Debug, Clone)]
pub struct DetailedValidationResult {
    /// `true` only if the input matches the schema.
    pub valid: bool,
    /// One-line error messages, suitable for logging or surfacing to users.
    pub errors: Vec<String>,
    /// SCREAMING_SNAKE_CASE error code derived from the *first* error, mapped to
    /// apcore semantics: `SCHEMA_UNION_NO_MATCH`, `SCHEMA_UNION_AMBIGUOUS`, or
    /// `SCHEMA_VALIDATION_FAILED`. `None` when valid.
    pub error_code: Option<ErrorCode>,
    /// Non-fatal format warnings (SHOULD-level enforcement, opt-in).
    pub warnings: Vec<FormatWarning>,
}

impl SchemaValidator {
    /// Create a new validator with an empty internal compile cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Legacy API: validate `value` against `schema` and return a coarse-grained
    /// [`ValidationResult`] with stringified errors. Format warnings are dropped
    /// here — use [`Self::validate_detailed`] to receive them.
    #[must_use]
    pub fn validate(&self, value: &Value, schema: &Value) -> ValidationResult {
        let detailed = self.validate_detailed(value, schema);
        ValidationResult {
            valid: detailed.valid,
            errors: detailed.errors,
            warnings: Vec::new(),
        }
    }

    /// Validate `value` against `schema`, returning a [`DetailedValidationResult`]
    /// with mapped error codes and format warnings.
    #[must_use]
    pub fn validate_detailed(&self, value: &Value, schema: &Value) -> DetailedValidationResult {
        let validator = match self.compile_cached(schema) {
            Ok(v) => v,
            Err(message) => {
                return DetailedValidationResult {
                    valid: false,
                    errors: vec![format!("invalid schema: {message}")],
                    error_code: Some(ErrorCode::SchemaParseError),
                    warnings: Vec::new(),
                };
            }
        };

        let raw_errors: Vec<_> = validator.iter_errors(value).collect();

        if raw_errors.is_empty() {
            return DetailedValidationResult {
                valid: true,
                errors: Vec::new(),
                error_code: None,
                warnings: format_warnings(value, schema),
            };
        }

        let error_code = Some(map_error_code(&raw_errors));
        let errors = raw_errors.iter().map(format_error).collect();
        DetailedValidationResult {
            valid: false,
            errors,
            error_code,
            warnings: Vec::new(),
        }
    }

    /// Validate and return `Ok(())` on success, or a `ModuleError` carrying the
    /// mapped apcore [`ErrorCode`] and structured per-failure details.
    pub fn validate_or_error(&self, value: &Value, schema: &Value) -> Result<(), ModuleError> {
        let detailed = self.validate_detailed(value, schema);
        if detailed.valid {
            return Ok(());
        }
        let error_maps: Vec<HashMap<String, String>> = detailed
            .errors
            .iter()
            .map(|message| {
                let mut m = HashMap::new();
                m.insert("message".to_string(), message.clone());
                m
            })
            .collect();
        let message = format!(
            "Schema validation failed with {} error(s)",
            detailed.errors.len()
        );
        let mut err = SchemaValidationError::new(message, error_maps).to_module_error();
        if let Some(code) = detailed.error_code {
            err.code = code;
        }
        Err(err)
    }

    fn compile_cached(&self, schema: &Value) -> Result<Arc<Validator>, String> {
        let digest = content_hash(schema);

        if let Some(v) = self.cache.lock().get(&digest) {
            return Ok(Arc::clone(v));
        }

        let compiled = jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .build(schema)
            .map_err(|e| e.to_string())?;
        let arc = Arc::new(compiled);

        // Another thread may have populated the entry while we were compiling;
        // both Arcs point to equivalent compiled validators, so overwriting is harmless.
        self.cache.lock().insert(digest, Arc::clone(&arc));
        Ok(arc)
    }

    /// Clear the internal compile cache. Useful for tests or long-running services
    /// that want to reclaim memory after schemas churn.
    pub fn clear_cache(&self) {
        self.cache.lock().clear();
    }

    /// Number of distinct schemas currently held in the compile cache.
    #[must_use]
    pub fn cache_len(&self) -> usize {
        self.cache.lock().len()
    }
}

/// Map a list of raw validator errors to a single apcore [`ErrorCode`].
///
/// The first error decides: top-level `oneOf` ambiguity outranks plain failures
/// because it's a stricter classification (the input was almost-valid).
fn map_error_code(errors: &[jsonschema::ValidationError<'_>]) -> ErrorCode {
    for error in errors {
        match &error.kind {
            ValidationErrorKind::OneOfMultipleValid => return ErrorCode::SchemaUnionAmbiguous,
            ValidationErrorKind::OneOfNotValid | ValidationErrorKind::AnyOf => {
                return ErrorCode::SchemaUnionNoMatch;
            }
            _ => {}
        }
    }
    ErrorCode::SchemaValidationError
}

/// Render a single validator error as the legacy substring-friendly message format.
///
/// The existing test suite asserts that error strings contain phrases like
/// "expected type", "missing required field", "additional property not allowed",
/// dot-separated paths (`address.city`), and bracketed array indices (`[1]`).
/// We keep that contract while delegating actual checking to the jsonschema crate.
fn format_error(error: &jsonschema::ValidationError<'_>) -> String {
    let path = format_instance_path(error.instance_path.as_str());
    let display_path = if path.is_empty() {
        "<root>".to_string()
    } else {
        path
    };

    match &error.kind {
        ValidationErrorKind::Required { property } => {
            let field = property.as_str().unwrap_or("?");
            let scoped = if display_path == "<root>" {
                field.to_string()
            } else {
                format!("{display_path}.{field}")
            };
            format!("{scoped}: missing required field")
        }
        ValidationErrorKind::AdditionalProperties { unexpected } => {
            let mut msgs = Vec::with_capacity(unexpected.len());
            for key in unexpected {
                let scoped = if display_path == "<root>" {
                    key.clone()
                } else {
                    format!("{display_path}.{key}")
                };
                msgs.push(format!("{scoped}: additional property not allowed"));
            }
            // The validator emits one error per group of unexpected keys; collapse
            // multi-key groups into one comma-separated message so the wrapper
            // still produces one string per error.
            msgs.join("; ")
        }
        ValidationErrorKind::Type { kind } => {
            format!(
                "{display_path}: expected type {kind:?}, got {}",
                error.instance
            )
        }
        ValidationErrorKind::Pattern { pattern } => {
            format!("{display_path}: value does not match pattern {pattern:?}")
        }
        ValidationErrorKind::Enum { .. } => {
            format!(
                "{display_path}: value {} is not one of the allowed enum values",
                error.instance
            )
        }
        ValidationErrorKind::Constant { expected_value } => {
            format!(
                "{display_path}: expected const {expected_value}, got {}",
                error.instance
            )
        }
        ValidationErrorKind::MinLength { limit } => {
            format!("{display_path}: minLength {limit} not satisfied")
        }
        ValidationErrorKind::MaxLength { limit } => {
            format!("{display_path}: maxLength {limit} exceeded")
        }
        ValidationErrorKind::Minimum { limit } => {
            format!("{display_path}: minimum {limit} not satisfied")
        }
        ValidationErrorKind::Maximum { limit } => {
            format!("{display_path}: maximum {limit} exceeded")
        }
        ValidationErrorKind::ExclusiveMinimum { limit } => {
            format!("{display_path}: exclusiveMinimum {limit} not satisfied")
        }
        ValidationErrorKind::ExclusiveMaximum { limit } => {
            format!("{display_path}: exclusiveMaximum {limit} exceeded")
        }
        ValidationErrorKind::OneOfMultipleValid => {
            format!("{display_path}: oneOf — input matched more than one branch")
        }
        ValidationErrorKind::OneOfNotValid => {
            format!("{display_path}: oneOf — input matched no branch")
        }
        ValidationErrorKind::AnyOf => {
            format!("{display_path}: anyOf — input matched no branch")
        }
        ValidationErrorKind::Not { schema: _ } => {
            format!("{display_path}: not — input matched the negated schema")
        }
        _ => format!("{display_path}: {error}"),
    }
}

/// Convert the validator's JSON Pointer (`/address/city`, `/items/1`) into the
/// dot-and-bracket form the existing tests expect (`address.city`, `[1]`).
fn format_instance_path(pointer: &str) -> String {
    use std::fmt::Write as _;

    if pointer.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for segment in pointer.split('/').filter(|s| !s.is_empty()) {
        if let Ok(idx) = segment.parse::<usize>() {
            // INVARIANT: writing to a String never fails.
            let _ = write!(out, "[{idx}]");
        } else {
            if !out.is_empty() {
                out.push('.');
            }
            out.push_str(segment);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_validator_compile_cache_reuses_compiled_schema() {
        let v = SchemaValidator::new();
        let schema_a = json!({ "type": "object", "required": ["x"] });
        let schema_b = json!({ "required": ["x"], "type": "object" }); // same content, different key order

        let _ = v.validate(&json!({ "x": 1 }), &schema_a);
        let _ = v.validate(&json!({ "x": 1 }), &schema_b);

        // Two byte-equivalent schemas hash to the same digest, so the cache
        // contains exactly one compiled validator.
        assert_eq!(v.cache_len(), 1);
    }

    #[test]
    fn test_validator_one_of_ambiguous_returns_dedicated_error_code() {
        let v = SchemaValidator::new();
        let schema = json!({
            "oneOf": [
                { "type": "object", "properties": { "value": { "type": "integer" } }, "required": ["value"] },
                { "type": "object", "properties": { "value": { "type": "number" } }, "required": ["value"] }
            ]
        });
        let detailed = v.validate_detailed(&json!({ "value": 42 }), &schema);
        assert!(!detailed.valid);
        assert_eq!(detailed.error_code, Some(ErrorCode::SchemaUnionAmbiguous));
    }

    #[test]
    fn test_validator_one_of_no_match_returns_no_match_code() {
        let v = SchemaValidator::new();
        let schema = json!({
            "oneOf": [
                { "type": "object", "properties": { "kind": { "const": "circle" } }, "required": ["kind"] },
                { "type": "object", "properties": { "kind": { "const": "rect" } }, "required": ["kind"] }
            ]
        });
        let detailed = v.validate_detailed(&json!({ "kind": "pentagon" }), &schema);
        assert!(!detailed.valid);
        assert_eq!(detailed.error_code, Some(ErrorCode::SchemaUnionNoMatch));
    }
}
