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
    assert!(result.errors[0].message.contains("expected type"));
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
    assert!(result.errors[0].message.contains("enum"));
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
    assert!(result.errors[0].message.contains("missing required field"));
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
    assert!(result.errors[0].message.contains("address.city"));
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
    assert!(result.errors[0]
        .message
        .contains("additional property not allowed"));
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
    assert!(result.errors[0].message.contains("[1]"));
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
    assert!(result.errors[0].message.contains("pattern"));
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

// ---------------------------------------------------------------------------
// A-D-005 / A-D-006: type-coercion engine (default-on), cross-language parity
// with apcore-python (model_validate(strict=not coerce_types)) and
// apcore-typescript (Value.Decode when coerceTypes). Mirrors pydantic lax mode.
// ---------------------------------------------------------------------------

#[test]
fn test_coerce_string_to_integer_accepted_by_default() {
    // new() defaults coerce_types=true — "42" against {type:integer} is accepted.
    let v = SchemaValidator::new();
    let schema = json!({
        "type": "object",
        "properties": { "age": { "type": "integer" } },
        "required": ["age"]
    });
    let result = v.validate(&json!({ "age": "42" }), &schema);
    assert!(
        result.valid,
        "coerce_types defaults true: \"42\" should coerce to 42 (matches Py/TS)"
    );
}

#[test]
fn test_coerce_string_to_integer_rejected_when_disabled() {
    let v = SchemaValidator::with_coerce_types(false);
    let schema = json!({
        "type": "object",
        "properties": { "age": { "type": "integer" } },
        "required": ["age"]
    });
    let result = v.validate(&json!({ "age": "42" }), &schema);
    assert!(
        !result.valid,
        "coerce_types=false: raw jsonschema rejects \"42\" for integer"
    );
}

#[test]
fn test_coerce_non_numeric_string_to_integer_rejected() {
    // "abc" cannot coerce — always invalid (matches Py/TS + fixture).
    let v = SchemaValidator::new();
    let schema = json!({
        "type": "object",
        "properties": { "count": { "type": "integer" } },
        "required": ["count"]
    });
    let result = v.validate(&json!({ "count": "abc" }), &schema);
    assert!(!result.valid, "non-numeric string is never coerced");
}

#[test]
fn test_coerce_string_to_number_float() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "object", "properties": { "x": { "type": "number" } } });
    assert!(v.validate(&json!({ "x": "3.14" }), &schema).valid);
}

#[test]
fn test_coerce_string_to_bool() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "object", "properties": { "flag": { "type": "boolean" } } });
    assert!(v.validate(&json!({ "flag": "true" }), &schema).valid);
    assert!(v.validate(&json!({ "flag": "false" }), &schema).valid);
    assert!(v.validate(&json!({ "flag": "1" }), &schema).valid);
    assert!(v.validate(&json!({ "flag": "0" }), &schema).valid);
}

#[test]
fn test_coerce_does_not_widen_int_to_string() {
    // pydantic lax mode does NOT coerce int->str; this must stay invalid.
    let v = SchemaValidator::new();
    let schema = json!({ "type": "string" });
    assert!(!v.validate(&json!(42), &schema).valid);
}

#[test]
fn test_coerce_int_to_float_widening() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "object", "properties": { "x": { "type": "number" } } });
    // integer already valid for number; widening is a no-op but must stay valid.
    assert!(v.validate(&json!({ "x": 42 }), &schema).valid);
}

#[test]
fn test_coerce_recurses_into_nested_objects_and_arrays() {
    let v = SchemaValidator::new();
    let schema = json!({
        "type": "object",
        "properties": {
            "nested": {
                "type": "object",
                "properties": { "n": { "type": "integer" } }
            },
            "items": {
                "type": "array",
                "items": { "type": "integer" }
            }
        }
    });
    let result = v.validate(
        &json!({ "nested": { "n": "7" }, "items": ["1", "2"] }),
        &schema,
    );
    assert!(
        result.valid,
        "coercion must recurse into nested objects and array items"
    );
}

// A-D-017: validate_input/validate_output must RETURN the coerced value.
#[test]
fn test_validate_input_returns_coerced_value() {
    let v = SchemaValidator::new();
    let schema = json!({
        "type": "object",
        "properties": { "age": { "type": "integer" } },
        "required": ["age"]
    });
    let returned = v
        .validate_input(&json!({ "age": "42" }), &schema)
        .expect("valid after coercion");
    assert_eq!(
        returned,
        json!({ "age": 42 }),
        "validate_input must return the coerced value, not the raw input"
    );
}

#[test]
fn test_validate_output_returns_coerced_value() {
    let v = SchemaValidator::new();
    let schema = json!({
        "type": "object",
        "properties": { "n": { "type": "number" } }
    });
    let returned = v
        .validate_output(&json!({ "n": "2.5" }), &schema)
        .expect("valid after coercion");
    assert_eq!(returned, json!({ "n": 2.5 }));
}

#[test]
fn test_validate_input_no_coercion_returns_raw_when_disabled() {
    let v = SchemaValidator::with_coerce_types(false);
    let schema = json!({ "type": "string" });
    let returned = v.validate_input(&json!("hello"), &schema).expect("valid");
    assert_eq!(returned, json!("hello"));
}

// ---------------------------------------------------------------------------
// D10-002: structured per-failure error details (path/message/constraint),
// cross-language parity with apcore-python/typescript SchemaValidationResult.
// ---------------------------------------------------------------------------

#[test]
fn test_schema_validator_errors_are_structured_details() {
    let v = SchemaValidator::new();
    let schema = json!({
        "type": "object",
        "properties": { "address": { "type": "object", "properties": { "city": { "type": "integer" } } } }
    });
    let result = v.validate(&json!({ "address": { "city": "not-an-int" } }), &schema);

    assert!(!result.valid);
    assert_eq!(result.errors.len(), 1);

    let detail = &result.errors[0];
    // Structured shape: dedicated `path` and `message` fields (not a flat string).
    assert_eq!(detail.path, "address.city");
    assert!(detail.message.contains("expected type"));
    assert_eq!(detail.constraint.as_deref(), Some("type"));
}

#[test]
fn test_schema_validator_required_error_detail_has_constraint() {
    let v = SchemaValidator::new();
    let schema = json!({ "type": "object", "required": ["name"] });
    let result = v.validate(&json!({}), &schema);

    assert!(!result.valid);
    let detail = &result.errors[0];
    assert!(detail.message.contains("missing required field"));
    assert_eq!(detail.constraint.as_deref(), Some("required"));
}
