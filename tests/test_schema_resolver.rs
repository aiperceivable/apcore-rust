//! Tests for RefResolver — JSON $ref resolution and circular reference detection.

use apcore::schema::RefResolver;
use serde_json::json;

// ---------------------------------------------------------------------------
// Local $ref resolution
// ---------------------------------------------------------------------------

#[test]
fn test_schema_resolver_resolve_local_ref() {
    let resolver = RefResolver::new();
    let schema = json!({
        "$defs": {
            "name": { "type": "string" }
        },
        "properties": {
            "first_name": { "$ref": "#/$defs/name" }
        }
    });
    let result = resolver.resolve(&schema).unwrap();
    assert_eq!(result["properties"]["first_name"]["type"], "string");
}

#[test]
fn test_schema_resolver_resolve_definitions_path() {
    let resolver = RefResolver::new();
    let schema = json!({
        "definitions": {
            "count": { "type": "integer" }
        },
        "properties": {
            "total": { "$ref": "#/definitions/count" }
        }
    });
    let result = resolver.resolve(&schema).unwrap();
    assert_eq!(result["properties"]["total"]["type"], "integer");
}

#[test]
fn test_schema_resolver_resolve_root_ref() {
    // #  (empty pointer) should return the root
    let resolver = RefResolver::new();
    let schema = json!({
        "type": "object",
        "properties": {
            "self_ref": { "$ref": "#" }
        }
    });
    // This should trigger circular detection because #-># is circular
    // Actually: resolve_inner inserts "#" into seen, then resolves root which
    // contains the same $ref "#" again -> circular.
    let result = resolver.resolve(&schema);
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Registered URI references
// ---------------------------------------------------------------------------

#[test]
fn test_schema_resolver_resolve_registered_uri() {
    let mut resolver = RefResolver::new();
    resolver.register(
        "https://example.com/schemas/address",
        json!({
            "type": "object",
            "properties": {
                "street": { "type": "string" }
            }
        }),
    );

    let schema = json!({
        "properties": {
            "home_address": { "$ref": "https://example.com/schemas/address" }
        }
    });
    let result = resolver.resolve(&schema).unwrap();
    assert_eq!(result["properties"]["home_address"]["type"], "object");
    assert_eq!(
        result["properties"]["home_address"]["properties"]["street"]["type"],
        "string"
    );
}

#[test]
fn test_schema_resolver_resolve_unregistered_uri_error() {
    let resolver = RefResolver::new();
    let schema = json!({
        "properties": {
            "x": { "$ref": "https://missing.example.com/schema" }
        }
    });
    let result = resolver.resolve(&schema);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, apcore::errors::ErrorCode::SchemaNotFound);
    assert!(err.message.contains("Referenced schema not found"));
}

// ---------------------------------------------------------------------------
// Local $ref not found
// ---------------------------------------------------------------------------

#[test]
fn test_schema_resolver_resolve_local_ref_not_found() {
    let resolver = RefResolver::new();
    let schema = json!({
        "properties": {
            "x": { "$ref": "#/$defs/nonexistent" }
        }
    });
    let result = resolver.resolve(&schema);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, apcore::errors::ErrorCode::SchemaNotFound);
    assert!(err.message.contains("Local $ref not found"));
}

// ---------------------------------------------------------------------------
// Circular reference detection
// ---------------------------------------------------------------------------

#[test]
fn test_schema_resolver_has_circular_refs_false() {
    let resolver = RefResolver::new();
    let schema = json!({
        "$defs": {
            "name": { "type": "string" }
        },
        "properties": {
            "x": { "$ref": "#/$defs/name" }
        }
    });
    assert!(!resolver.has_circular_refs(&schema));
}

#[test]
fn test_schema_resolver_has_circular_refs_true_self_ref() {
    let resolver = RefResolver::new();
    let schema = json!({
        "$defs": {
            "node": {
                "type": "object",
                "properties": {
                    "child": { "$ref": "#/$defs/node" }
                }
            }
        },
        "properties": {
            "root": { "$ref": "#/$defs/node" }
        }
    });
    assert!(resolver.has_circular_refs(&schema));
}

#[test]
fn test_schema_resolver_resolve_circular_ref_returns_error() {
    let resolver = RefResolver::new();
    let schema = json!({
        "$defs": {
            "node": {
                "type": "object",
                "properties": {
                    "child": { "$ref": "#/$defs/node" }
                }
            }
        },
        "properties": {
            "root": { "$ref": "#/$defs/node" }
        }
    });
    let result = resolver.resolve(&schema);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, apcore::errors::ErrorCode::SchemaCircularRef);
    assert!(err.message.contains("Circular"));
}

// ---------------------------------------------------------------------------
// Array resolution
// ---------------------------------------------------------------------------

#[test]
fn test_schema_resolver_resolve_refs_in_array() {
    let resolver = RefResolver::new();
    let schema = json!({
        "$defs": {
            "tag": { "type": "string" }
        },
        "items": [
            { "$ref": "#/$defs/tag" },
            { "type": "integer" }
        ]
    });
    let result = resolver.resolve(&schema).unwrap();
    let items = result["items"].as_array().unwrap();
    assert_eq!(items[0]["type"], "string");
    assert_eq!(items[1]["type"], "integer");
}

// ---------------------------------------------------------------------------
// Nested $ref chains
// ---------------------------------------------------------------------------

#[test]
fn test_schema_resolver_resolve_chained_refs() {
    let resolver = RefResolver::new();
    let schema = json!({
        "$defs": {
            "base": { "type": "string" },
            "alias": { "$ref": "#/$defs/base" }
        },
        "properties": {
            "x": { "$ref": "#/$defs/alias" }
        }
    });
    let result = resolver.resolve(&schema).unwrap();
    assert_eq!(result["properties"]["x"]["type"], "string");
}

// ---------------------------------------------------------------------------
// No $refs — passthrough
// ---------------------------------------------------------------------------

#[test]
fn test_schema_resolver_resolve_no_refs_returns_same() {
    let resolver = RefResolver::new();
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        }
    });
    let result = resolver.resolve(&schema).unwrap();
    assert_eq!(result, schema);
}

// ---------------------------------------------------------------------------
// Scalar passthrough
// ---------------------------------------------------------------------------

#[test]
fn test_schema_resolver_resolve_scalar_values() {
    let resolver = RefResolver::new();
    assert_eq!(resolver.resolve(&json!("hello")).unwrap(), json!("hello"));
    assert_eq!(resolver.resolve(&json!(42)).unwrap(), json!(42));
    assert_eq!(resolver.resolve(&json!(true)).unwrap(), json!(true));
    assert_eq!(resolver.resolve(&json!(null)).unwrap(), json!(null));
}

// ---------------------------------------------------------------------------
// Default impl
// ---------------------------------------------------------------------------

#[test]
fn test_schema_resolver_default() {
    let resolver = RefResolver::default();
    let schema = json!({ "type": "string" });
    assert_eq!(resolver.resolve(&schema).unwrap(), schema);
}

// ---------------------------------------------------------------------------
// has_circular_refs with registered URI
// ---------------------------------------------------------------------------

#[test]
fn test_schema_resolver_has_circular_refs_with_unresolvable_uri() {
    let resolver = RefResolver::new();
    let schema = json!({
        "properties": {
            "x": { "$ref": "https://missing.com/not-here" }
        }
    });
    // unresolvable URI => lookup fails, so no circular detected
    assert!(!resolver.has_circular_refs(&schema));
}

#[test]
fn test_schema_resolver_has_circular_refs_false_for_scalars() {
    let resolver = RefResolver::new();
    assert!(!resolver.has_circular_refs(&json!(42)));
    assert!(!resolver.has_circular_refs(&json!("hello")));
    assert!(!resolver.has_circular_refs(&json!(null)));
}

#[test]
fn test_schema_resolver_has_circular_refs_in_array() {
    let resolver = RefResolver::new();
    let schema = json!({
        "$defs": {
            "node": {
                "type": "object",
                "properties": {
                    "child": { "$ref": "#/$defs/node" }
                }
            }
        },
        "items": [
            { "$ref": "#/$defs/node" }
        ]
    });
    assert!(resolver.has_circular_refs(&schema));
}

// ---------------------------------------------------------------------------
// max_depth — sync SCHEMA-001
// ---------------------------------------------------------------------------

#[test]
fn test_schema_resolver_default_max_depth_is_32() {
    // Cross-language parity: apcore-python and apcore-typescript both default
    // to schema.max_ref_depth = 32.
    let resolver = RefResolver::new();
    assert_eq!(resolver.max_depth(), 32);
}

#[test]
fn test_schema_resolver_rejects_chain_exceeding_max_depth() {
    // Build a non-circular chain of 40 cascading $refs:
    //   #/$defs/level0 -> #/$defs/level1 -> ... -> #/$defs/level39
    // With max_depth=32 this MUST fail with SchemaCircularRef (max-depth bucket).
    let resolver = RefResolver::with_max_depth(32);
    let mut defs = serde_json::Map::new();
    for i in 0..40usize {
        let body = if i + 1 < 40 {
            json!({ "type": "object", "properties": { "next": { "$ref": format!("#/$defs/level{}", i + 1) } } })
        } else {
            json!({ "type": "string" })
        };
        defs.insert(format!("level{i}"), body);
    }
    let schema = json!({
        "$ref": "#/$defs/level0",
        "$defs": serde_json::Value::Object(defs),
    });
    let err = resolver
        .resolve(&schema)
        .expect_err("40-level $ref chain must exceed max_depth=32");
    assert_eq!(err.code, apcore::errors::ErrorCode::SchemaCircularRef);
    assert!(
        err.message.to_lowercase().contains("max_depth")
            || err.message.to_lowercase().contains("max-depth")
            || err.message.to_lowercase().contains("recursion"),
        "error should mention the depth cap; got: {}",
        err.message
    );
}

#[test]
fn test_schema_resolver_with_max_depth_constructor_round_trip() {
    let resolver = RefResolver::with_max_depth(8);
    assert_eq!(resolver.max_depth(), 8);
}
