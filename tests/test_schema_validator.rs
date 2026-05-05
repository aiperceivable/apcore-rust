//! Tests for SchemaValidator — JSON Schema validation of values.

use apcore::schema::SchemaValidator;
use serde_json::json;

// ---------------------------------------------------------------------------
// Type validation
// ---------------------------------------------------------------------------

#[test]
fn test_schema_validator_valid_string() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "string" });
    let result = v.validate(&json!("hello"), &schema);
    assert!(result.valid);
    assert!(result.errors.is_empty());
}

#[test]
fn test_schema_validator_invalid_type_string_expected() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "string" });
    let result = v.validate(&json!(42), &schema);
    assert!(!result.valid);
    assert_eq!(result.errors.len(), 1);
    assert!(result.errors[0].contains("expected type"));
}

#[test]
fn test_schema_validator_valid_integer() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "integer" });
    let result = v.validate(&json!(42), &schema);
    assert!(result.valid);
}

#[test]
fn test_schema_validator_valid_number_accepts_float() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "number" });
    let result = v.validate(&json!(1.5_f64), &schema);
    assert!(result.valid);
}

#[test]
fn test_schema_validator_valid_number_accepts_integer() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "number" });
    let result = v.validate(&json!(42), &schema);
    assert!(result.valid);
}

#[test]
fn test_schema_validator_valid_boolean() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "boolean" });
    let result = v.validate(&json!(true), &schema);
    assert!(result.valid);
}

#[test]
fn test_schema_validator_valid_null() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "null" });
    let result = v.validate(&json!(null), &schema);
    assert!(result.valid);
}

#[test]
fn test_schema_validator_valid_object() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "object" });
    let result = v.validate(&json!({}), &schema);
    assert!(result.valid);
}

#[test]
fn test_schema_validator_valid_array() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "array" });
    let result = v.validate(&json!([1, 2, 3]), &schema);
    assert!(result.valid);
}

// ---------------------------------------------------------------------------
// Union type (array of types)
// ---------------------------------------------------------------------------

#[test]
fn test_schema_validator_union_type_matches_first() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": ["string", "null"] });
    let result = v.validate(&json!("hello"), &schema);
    assert!(result.valid);
}

#[test]
fn test_schema_validator_union_type_matches_second() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": ["string", "null"] });
    let result = v.validate(&json!(null), &schema);
    assert!(result.valid);
}

#[test]
fn test_schema_validator_union_type_no_match() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": ["string", "null"] });
    let result = v.validate(&json!(42), &schema);
    assert!(!result.valid);
}

// ---------------------------------------------------------------------------
// Enum validation
// ---------------------------------------------------------------------------

#[test]
fn test_schema_validator_enum_valid() {
    let v = SchemaValidator::new();
    let schema = json!({ "enum": ["red", "green", "blue"] });
    let result = v.validate(&json!("green"), &schema);
    assert!(result.valid);
}

#[test]
fn test_schema_validator_enum_invalid() {
    let v = SchemaValidator::new();
    let schema = json!({ "enum": ["red", "green", "blue"] });
    let result = v.validate(&json!("yellow"), &schema);
    assert!(!result.valid);
    assert!(result.errors[0].contains("enum"));
}

// ---------------------------------------------------------------------------
// Required fields
// ---------------------------------------------------------------------------

#[test]
fn test_schema_validator_required_field_present() {
    let v = SchemaValidator::new();
    let schema = json!({
        "type": "object",
        "required": ["name"],
        "properties": {
            "name": { "type": "string" }
        }
    });
    let result = v.validate(&json!({ "name": "Alice" }), &schema);
    assert!(result.valid);
}

#[test]
fn test_schema_validator_required_field_missing() {
    let v = SchemaValidator::new();
    let schema = json!({
        "type": "object",
        "required": ["name"],
        "properties": {
            "name": { "type": "string" }
        }
    });
    let result = v.validate(&json!({}), &schema);
    assert!(!result.valid);
    assert!(result.errors[0].contains("missing required field"));
}

#[test]
fn test_schema_validator_multiple_required_fields_missing() {
    let v = SchemaValidator::new();
    let schema = json!({
        "type": "object",
        "required": ["name", "age"],
        "properties": {
            "name": { "type": "string" },
            "age": { "type": "integer" }
        }
    });
    let result = v.validate(&json!({}), &schema);
    assert!(!result.valid);
    assert_eq!(result.errors.len(), 2);
}

// ---------------------------------------------------------------------------
// Nested object validation
// ---------------------------------------------------------------------------

#[test]
fn test_schema_validator_nested_object_valid() {
    let v = SchemaValidator::new();
    let schema = json!({
        "type": "object",
        "properties": {
            "address": {
                "type": "object",
                "properties": {
                    "city": { "type": "string" }
                },
                "required": ["city"]
            }
        }
    });
    let result = v.validate(&json!({ "address": { "city": "NYC" } }), &schema);
    assert!(result.valid);
}

#[test]
fn test_schema_validator_nested_object_invalid_type() {
    let v = SchemaValidator::new();
    let schema = json!({
        "type": "object",
        "properties": {
            "address": {
                "type": "object",
                "properties": {
                    "city": { "type": "string" }
                }
            }
        }
    });
    let result = v.validate(&json!({ "address": { "city": 42 } }), &schema);
    assert!(!result.valid);
    assert!(result.errors[0].contains("address.city"));
}

// ---------------------------------------------------------------------------
// additionalProperties: false
// ---------------------------------------------------------------------------

#[test]
fn test_schema_validator_additional_properties_false_rejects_extra() {
    let v = SchemaValidator::new();
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        },
        "additionalProperties": false
    });
    let result = v.validate(&json!({ "name": "Alice", "age": 30 }), &schema);
    assert!(!result.valid);
    assert!(result.errors[0].contains("additional property not allowed"));
}

#[test]
fn test_schema_validator_additional_properties_false_allows_known() {
    let v = SchemaValidator::new();
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        },
        "additionalProperties": false
    });
    let result = v.validate(&json!({ "name": "Alice" }), &schema);
    assert!(result.valid);
}

// ---------------------------------------------------------------------------
// Array items validation
// ---------------------------------------------------------------------------

#[test]
fn test_schema_validator_array_items_valid() {
    let v = SchemaValidator::new();
    let schema = json!({
        "type": "array",
        "items": { "type": "string" }
    });
    let result = v.validate(&json!(["a", "b", "c"]), &schema);
    assert!(result.valid);
}

#[test]
fn test_schema_validator_array_items_invalid_element() {
    let v = SchemaValidator::new();
    let schema = json!({
        "type": "array",
        "items": { "type": "string" }
    });
    let result = v.validate(&json!(["a", 42, "c"]), &schema);
    assert!(!result.valid);
    assert!(result.errors[0].contains("[1]"));
}

#[test]
fn test_schema_validator_empty_array_valid() {
    let v = SchemaValidator::new();
    let schema = json!({
        "type": "array",
        "items": { "type": "string" }
    });
    let result = v.validate(&json!([]), &schema);
    assert!(result.valid);
}

// ---------------------------------------------------------------------------
// Pattern validation
// ---------------------------------------------------------------------------

#[test]
fn test_schema_validator_pattern_matches() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "string", "pattern": "^[a-z]+$" });
    let result = v.validate(&json!("hello"), &schema);
    assert!(result.valid);
}

#[test]
fn test_schema_validator_pattern_no_match() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "string", "pattern": "^[a-z]+$" });
    let result = v.validate(&json!("Hello123"), &schema);
    assert!(!result.valid);
    assert!(result.errors[0].contains("pattern"));
}

#[test]
fn test_schema_validator_pattern_not_applied_to_non_string() {
    let v = SchemaValidator::new();
    // pattern only fires when the value is a string
    let schema = json!({ "pattern": "^[a-z]+$" });
    let result = v.validate(&json!(42), &schema);
    assert!(result.valid);
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_schema_validator_empty_schema_accepts_anything() {
    let v = SchemaValidator::new();
    let schema = json!({});
    assert!(v.validate(&json!("hello"), &schema).valid);
    assert!(v.validate(&json!(42), &schema).valid);
    assert!(v.validate(&json!(null), &schema).valid);
    assert!(v.validate(&json!([1, 2]), &schema).valid);
}

#[test]
fn test_schema_validator_boolean_schema_true_accepts_anything() {
    let v = SchemaValidator::new();
    // non-object schema => permissive
    let schema = json!(true);
    assert!(v.validate(&json!("hello"), &schema).valid);
}

#[test]
fn test_schema_validator_warnings_always_empty() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "string" });
    let result = v.validate(&json!("ok"), &schema);
    assert!(result.warnings.is_empty());
}

// ---------------------------------------------------------------------------
// validate_or_error
// ---------------------------------------------------------------------------

#[test]
fn test_schema_validator_validate_or_error_ok() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "string" });
    assert!(v.validate_or_error(&json!("hello"), &schema).is_ok());
}

#[test]
fn test_schema_validator_validate_or_error_returns_module_error() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "string" });
    let err = v.validate_or_error(&json!(42), &schema).unwrap_err();
    assert_eq!(err.code, apcore::errors::ErrorCode::SchemaValidationError);
    assert!(err.message.contains("validation failed"));
    assert!(err.details.contains_key("errors"));
}

// ---------------------------------------------------------------------------
// D11-010: validate_input / validate_output cross-language parity
// ---------------------------------------------------------------------------
//
// Python (`validator.py:69`) exposes `validate_input(data, model)` and
// `validate_output(data, model)` returning the validated dict, raising
// SchemaValidationError on failure. TypeScript (`validator.ts:78`) exposes
// `validateInput` / `validateOutput` with the same role. Rust previously
// only exposed `validate`, `validate_detailed`, `validate_or_error` — user
// code calling the SDK validator directly could not port between languages.

#[test]
fn test_validate_input_returns_data_on_success() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "string" });
    let data = json!("hello");
    let returned = v.validate_input(&data, &schema).expect("valid input");
    assert_eq!(
        returned, data,
        "validate_input must return the input on success"
    );
}

#[test]
fn test_validate_input_raises_on_failure() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "string" });
    let err = v.validate_input(&json!(42), &schema).unwrap_err();
    assert_eq!(err.code, apcore::errors::ErrorCode::SchemaValidationError);
}

#[test]
fn test_validate_output_returns_data_on_success() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "object", "required": ["ok"], "properties": { "ok": { "type": "boolean" } } });
    let data = json!({"ok": true});
    let returned = v.validate_output(&data, &schema).expect("valid output");
    assert_eq!(returned, data);
}

#[test]
fn test_validate_output_raises_on_failure() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "object", "required": ["ok"] });
    let err = v.validate_output(&json!({}), &schema).unwrap_err();
    assert_eq!(err.code, apcore::errors::ErrorCode::SchemaValidationError);
}

// ---------------------------------------------------------------------------
// Default impl
// ---------------------------------------------------------------------------

#[test]
fn test_schema_validator_default() {
    let v = SchemaValidator::default();
    let schema = json!({ "type": "string" });
    assert!(v.validate(&json!("ok"), &schema).valid);
}
