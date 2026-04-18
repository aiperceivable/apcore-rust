// APCore Protocol — Schema validator
// Spec reference: JSON Schema validation of module inputs/outputs

use crate::errors::{ModuleError, SchemaValidationError};
use crate::module::ValidationResult;

/// Validates data against JSON schemas.
#[derive(Debug)]
pub struct SchemaValidator;

impl SchemaValidator {
    /// Create a new schema validator.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Validate a value against a JSON schema.
    #[must_use]
    pub fn validate(
        &self,
        value: &serde_json::Value,
        schema: &serde_json::Value,
    ) -> ValidationResult {
        let mut errors = Vec::new();
        self.validate_inner(value, schema, "", &mut errors);
        ValidationResult {
            valid: errors.is_empty(),
            errors,
            warnings: Vec::new(),
        }
    }

    /// Validate and return a `ModuleError` on failure.
    pub fn validate_or_error(
        &self,
        value: &serde_json::Value,
        schema: &serde_json::Value,
    ) -> Result<(), ModuleError> {
        let result = self.validate(value, schema);
        if result.valid {
            Ok(())
        } else {
            let error_maps = result
                .errors
                .iter()
                .map(|e| {
                    let mut m = std::collections::HashMap::new();
                    m.insert("message".to_string(), e.clone());
                    m
                })
                .collect();
            let err = SchemaValidationError::new(
                format!(
                    "Schema validation failed with {} error(s)",
                    result.errors.len()
                ),
                error_maps,
            );
            Err(err.to_module_error())
        }
    }

    /// Recursively validate a value against a schema node, collecting errors.
    #[allow(clippy::self_only_used_in_recursion)] // `self` needed for recursive dispatch
    #[allow(clippy::too_many_lines)] // single-pass recursive schema validator; splitting would hurt readability
    fn validate_inner(
        &self,
        value: &serde_json::Value,
        schema: &serde_json::Value,
        path: &str,
        errors: &mut Vec<String>,
    ) {
        let Some(schema_obj) = schema.as_object() else {
            return; // non-object schema (e.g. `true`) – permissive
        };

        // --- "enum" check ---
        if let Some(enum_values) = schema_obj.get("enum") {
            if let Some(arr) = enum_values.as_array() {
                if !arr.contains(value) {
                    let display_path = if path.is_empty() { "<root>" } else { path };
                    errors.push(format!(
                        "{display_path}: value {value:?} is not one of the allowed enum values"
                    ));
                    return;
                }
            }
        }

        // --- "type" check (supports single string or array of types) ---
        if let Some(type_val) = schema_obj.get("type") {
            let check_type = |t: &str, v: &serde_json::Value| -> bool {
                match t {
                    "string" => v.is_string(),
                    "integer" => v.is_i64() || v.is_u64(),
                    "number" => v.is_number(),
                    "boolean" => v.is_boolean(),
                    "object" => v.is_object(),
                    "array" => v.is_array(),
                    "null" => v.is_null(),
                    _ => true,
                }
            };
            let type_ok = if let Some(type_str) = type_val.as_str() {
                check_type(type_str, value)
            } else if let Some(type_arr) = type_val.as_array() {
                type_arr
                    .iter()
                    .any(|t| t.as_str().is_some_and(|s| check_type(s, value)))
            } else {
                true
            };
            if !type_ok {
                let display_path = if path.is_empty() { "<root>" } else { path };
                errors.push(format!(
                    "{display_path}: expected type {type_val:?}, got {value:?}"
                ));
                return;
            }
        }

        // --- "pattern" check for strings ---
        if let (Some(pattern_val), Some(str_val)) = (
            schema_obj.get("pattern").and_then(|p| p.as_str()),
            value.as_str(),
        ) {
            if let Ok(re) = regex::Regex::new(pattern_val) {
                if !re.is_match(str_val) {
                    let display_path = if path.is_empty() { "<root>" } else { path };
                    errors.push(format!(
                        "{display_path}: value {str_val:?} does not match pattern {pattern_val:?}"
                    ));
                }
            }
        }

        // --- object-specific checks ---
        if value.is_object() {
            // INVARIANT: `value.is_object()` guard above ensures this succeeds.
            let obj = value.as_object().unwrap();

            // "required" fields
            if let Some(required) = schema_obj.get("required") {
                if let Some(arr) = required.as_array() {
                    for req in arr {
                        if let Some(field_name) = req.as_str() {
                            if !obj.contains_key(field_name) {
                                let field_path = if path.is_empty() {
                                    field_name.to_string()
                                } else {
                                    format!("{path}.{field_name}")
                                };
                                errors.push(format!("{field_path}: missing required field"));
                            }
                        }
                    }
                }
            }

            // "properties" -- recursive validation
            if let Some(properties) = schema_obj.get("properties") {
                if let Some(props) = properties.as_object() {
                    for (key, prop_schema) in props {
                        if let Some(prop_value) = obj.get(key) {
                            let child_path = if path.is_empty() {
                                key.clone()
                            } else {
                                format!("{path}.{key}")
                            };
                            self.validate_inner(prop_value, prop_schema, &child_path, errors);
                        }
                    }
                }
            }

            // "additionalProperties": false -- reject unknown keys
            if schema_obj.get("additionalProperties") == Some(&serde_json::Value::Bool(false)) {
                if let Some(props) = schema_obj.get("properties").and_then(|p| p.as_object()) {
                    for key in obj.keys() {
                        if !props.contains_key(key) {
                            let field_path = if path.is_empty() {
                                key.clone()
                            } else {
                                format!("{path}.{key}")
                            };
                            errors.push(format!("{field_path}: additional property not allowed"));
                        }
                    }
                }
            }
        }

        // --- array-specific checks ---
        if value.is_array() {
            if let Some(items_schema) = schema_obj.get("items") {
                // INVARIANT: `value.is_array()` guard above ensures this succeeds.
                let arr = value.as_array().unwrap();
                for (i, item) in arr.iter().enumerate() {
                    let child_path = if path.is_empty() {
                        format!("[{i}]")
                    } else {
                        format!("{path}[{i}]")
                    };
                    self.validate_inner(item, items_schema, &child_path, errors);
                }
            }
        }
    }
}

impl Default for SchemaValidator {
    fn default() -> Self {
        Self::new()
    }
}
