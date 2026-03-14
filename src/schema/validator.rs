// APCore Protocol — Schema validator
// Spec reference: JSON Schema validation of module inputs/outputs

use crate::errors::{ModuleError, SchemaValidationError};
use crate::module::ValidationResult;

/// Validates data against JSON schemas.
#[derive(Debug)]
pub struct SchemaValidator;

impl SchemaValidator {
    /// Create a new schema validator.
    pub fn new() -> Self {
        Self
    }

    /// Validate a value against a JSON schema.
    pub fn validate(
        &self,
        value: &serde_json::Value,
        schema: &serde_json::Value,
    ) -> ValidationResult {
        let mut errors = Vec::new();
        self.validate_inner(value, schema, "".to_string(), &mut errors);
        ValidationResult {
            valid: errors.is_empty(),
            errors,
            warnings: Vec::new(),
        }
    }

    /// Validate and return a ModuleError on failure.
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
    fn validate_inner(
        &self,
        value: &serde_json::Value,
        schema: &serde_json::Value,
        path: String,
        errors: &mut Vec<String>,
    ) {
        let schema_obj = match schema.as_object() {
            Some(obj) => obj,
            None => return, // non-object schema (e.g. `true`) – permissive
        };

        // --- "enum" check ---
        if let Some(enum_values) = schema_obj.get("enum") {
            if let Some(arr) = enum_values.as_array() {
                if !arr.contains(value) {
                    let display_path = if path.is_empty() { "<root>" } else { &path };
                    errors.push(format!(
                        "{}: value {:?} is not one of the allowed enum values",
                        display_path, value
                    ));
                    return;
                }
            }
        }

        // --- "type" check ---
        if let Some(type_val) = schema_obj.get("type") {
            let type_str = type_val.as_str().unwrap_or("");
            let type_ok = match type_str {
                "string" => value.is_string(),
                "integer" => value.is_i64() || value.is_u64(),
                "number" => value.is_number(),
                "boolean" => value.is_boolean(),
                "object" => value.is_object(),
                "array" => value.is_array(),
                "null" => value.is_null(),
                _ => true, // unknown type – permissive
            };
            if !type_ok {
                let display_path = if path.is_empty() { "<root>" } else { &path };
                errors.push(format!(
                    "{}: expected type '{}', got {:?}",
                    display_path, type_str, value
                ));
                return;
            }
        }

        // --- object-specific checks ---
        if value.is_object() {
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
                                    format!("{}.{}", path, field_name)
                                };
                                errors.push(format!("{}: missing required field", field_path));
                            }
                        }
                    }
                }
            }

            // "properties" – recursive validation
            if let Some(properties) = schema_obj.get("properties") {
                if let Some(props) = properties.as_object() {
                    for (key, prop_schema) in props {
                        if let Some(prop_value) = obj.get(key) {
                            let child_path = if path.is_empty() {
                                key.clone()
                            } else {
                                format!("{}.{}", path, key)
                            };
                            self.validate_inner(prop_value, prop_schema, child_path, errors);
                        }
                    }
                }
            }
        }

        // --- array-specific checks ---
        if value.is_array() {
            if let Some(items_schema) = schema_obj.get("items") {
                let arr = value.as_array().unwrap();
                for (i, item) in arr.iter().enumerate() {
                    let child_path = if path.is_empty() {
                        format!("[{}]", i)
                    } else {
                        format!("{}[{}]", path, i)
                    };
                    self.validate_inner(item, items_schema, child_path, errors);
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
