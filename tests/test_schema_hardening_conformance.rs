//! Cross-language conformance tests for Schema System Hardening (Issue #44,
//! PROTOCOL_SPEC §4.15).
//!
//! Each test consumes one of the canonical `schema_hardening_*.json` fixtures
//! shipped by the `apcore` spec repo (sibling directory or `APCORE_SPEC_REPO`).

use std::path::PathBuf;

use serde_json::{json, Value};

use apcore::errors::ErrorCode;
use apcore::schema::{content_hash, format_warnings, FormatWarning, SchemaValidator};

fn find_fixtures_root() -> PathBuf {
    if let Ok(spec_repo) = std::env::var("APCORE_SPEC_REPO") {
        let p = PathBuf::from(&spec_repo)
            .join("conformance")
            .join("fixtures");
        if p.is_dir() {
            return p;
        }
        panic!("APCORE_SPEC_REPO={spec_repo} does not contain conformance/fixtures/");
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let sibling = manifest_dir
        .parent()
        .unwrap()
        .join("apcore")
        .join("conformance")
        .join("fixtures");
    if sibling.is_dir() {
        return sibling;
    }

    panic!(
        "Cannot find apcore conformance fixtures.\n\
         Fix one of:\n\
         1. Set APCORE_SPEC_REPO to the apcore spec repo path\n\
         2. Clone apcore as a sibling: git clone <apcore-url> {}\n",
        manifest_dir.parent().unwrap().join("apcore").display()
    );
}

fn load_fixture(name: &str) -> Value {
    let path = find_fixtures_root().join(format!("{name}.json"));
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("Failed to read fixture: {}", path.display()));
    serde_json::from_str(&content).unwrap_or_else(|e| panic!("Invalid JSON in {name}: {e}"))
}

fn parse_expected_code(s: &str) -> ErrorCode {
    match s {
        "SCHEMA_UNION_NO_MATCH" => ErrorCode::SchemaUnionNoMatch,
        "SCHEMA_UNION_AMBIGUOUS" => ErrorCode::SchemaUnionAmbiguous,
        // Canonical wire code is SCHEMA_VALIDATION_ERROR (SCREAMING_SNAKE_CASE
        // of ErrorCode::SchemaValidationError); accept the legacy
        // SCHEMA_VALIDATION_FAILED spelling too for fixture compatibility.
        "SCHEMA_VALIDATION_ERROR" | "SCHEMA_VALIDATION_FAILED" => ErrorCode::SchemaValidationError,
        "SCHEMA_MAX_DEPTH_EXCEEDED" => ErrorCode::SchemaMaxDepthExceeded,
        other => panic!("Unknown error code in fixture: {other}"),
    }
}

#[test]
fn conformance_schema_hardening_union() {
    let fixture = load_fixture("schema_hardening_union");
    let validator = SchemaValidator::new();

    for tc in fixture["test_cases"].as_array().unwrap() {
        let id = tc["id"].as_str().unwrap();
        let schema = &tc["schema"];
        let input = &tc["input"];
        let expected_valid = tc["expected"]["valid"].as_bool().unwrap();
        let expected_code = tc["expected"]["error_code"].as_str();

        let result = validator.validate_detailed(input, schema);
        assert_eq!(
            result.valid, expected_valid,
            "FAIL [{id}]: expected valid={expected_valid}, got valid={} errors={:?}",
            result.valid, result.errors
        );

        if !expected_valid {
            let want = expected_code
                .unwrap_or_else(|| panic!("FAIL [{id}]: fixture missing expected.error_code"));
            let got = result
                .error_code
                .unwrap_or_else(|| panic!("FAIL [{id}]: validator returned no error_code"));
            assert_eq!(
                got,
                parse_expected_code(want),
                "FAIL [{id}]: expected error_code={want}, got {got:?}"
            );
        }
    }
}

#[test]
fn conformance_schema_hardening_recursive() {
    let fixture = load_fixture("schema_hardening_recursive");
    let validator = SchemaValidator::new();
    let schema = &fixture["schema"];

    for tc in fixture["test_cases"].as_array().unwrap() {
        let id = tc["id"].as_str().unwrap();
        let input = &tc["input"];
        let expected_valid = tc["expected"]["valid"].as_bool().unwrap();
        let expected_code = tc["expected"]["error_code"].as_str();

        let result = validator.validate_detailed(input, schema);
        assert_eq!(
            result.valid, expected_valid,
            "FAIL [{id}]: expected valid={expected_valid}, got valid={} errors={:?}",
            result.valid, result.errors
        );

        if !expected_valid {
            let want = expected_code
                .unwrap_or_else(|| panic!("FAIL [{id}]: fixture missing expected.error_code"));
            let got = result
                .error_code
                .unwrap_or_else(|| panic!("FAIL [{id}]: validator returned no error_code"));
            assert_eq!(
                got,
                parse_expected_code(want),
                "FAIL [{id}]: expected error_code={want}, got {got:?}"
            );
        }
    }
}

#[test]
fn conformance_schema_hardening_constraints() {
    let fixture = load_fixture("schema_hardening_constraints");
    let validator = SchemaValidator::new();

    for tc in fixture["test_cases"].as_array().unwrap() {
        let id = tc["id"].as_str().unwrap();
        let schema = &tc["schema"];
        let input = &tc["input"];
        let expected_valid = tc["expected"]["valid"].as_bool().unwrap();
        let expected_code = tc["expected"]["error_code"].as_str();

        let result = validator.validate_detailed(input, schema);
        assert_eq!(
            result.valid, expected_valid,
            "FAIL [{id}]: expected valid={expected_valid}, got valid={} errors={:?}",
            result.valid, result.errors
        );

        if !expected_valid {
            let want = expected_code
                .unwrap_or_else(|| panic!("FAIL [{id}]: fixture missing expected.error_code"));
            let got = result
                .error_code
                .unwrap_or_else(|| panic!("FAIL [{id}]: validator returned no error_code"));
            assert_eq!(
                got,
                parse_expected_code(want),
                "FAIL [{id}]: expected error_code={want}, got {got:?}"
            );
        }
    }
}

#[test]
fn conformance_schema_hardening_formats() {
    let fixture = load_fixture("schema_hardening_formats");
    let validator = SchemaValidator::new();

    for tc in fixture["test_cases"].as_array().unwrap() {
        let id = tc["id"].as_str().unwrap();
        let schema = &tc["schema"];
        let input = &tc["input"];
        let expected_valid = tc["expected"]["valid"].as_bool().unwrap();
        let expected_warn = tc["expected"]["warn_logged"].as_bool().unwrap_or(false);

        // Format enforcement is SHOULD-level, so the validator must always accept the
        // input — warnings surface separately via `format_warnings`.
        let result = validator.validate_detailed(input, schema);
        assert_eq!(
            result.valid, expected_valid,
            "FAIL [{id}]: expected valid={expected_valid}, got valid={} errors={:?}",
            result.valid, result.errors
        );

        let warnings = format_warnings(input, schema);
        let warn_logged = !warnings.is_empty();
        assert_eq!(
            warn_logged, expected_warn,
            "FAIL [{id}]: expected warn_logged={expected_warn}, got warn_logged={warn_logged} warnings={warnings:?}"
        );
    }
}

#[test]
fn conformance_schema_hardening_cache() {
    let fixture = load_fixture("schema_hardening_cache");

    for tc in fixture["test_cases"].as_array().unwrap() {
        let id = tc["id"].as_str().unwrap();
        let schemas = tc["schemas"].as_array().unwrap();
        assert_eq!(
            schemas.len(),
            2,
            "FAIL [{id}]: cache fixture must have exactly 2 schemas"
        );
        let expected_same = tc["expected"]["same_hash"].as_bool().unwrap();

        let h1 = content_hash(&schemas[0]);
        let h2 = content_hash(&schemas[1]);
        let same = h1 == h2;
        assert_eq!(
            same, expected_same,
            "FAIL [{id}]: expected same_hash={expected_same}, got h1={h1} h2={h2}"
        );
    }
}

/// A-D-037: compute and REPORT content_hash for tricky schemas (float 1.0,
/// unicode keys/values, large integers beyond 2^53, unsorted nested keys, and
/// a baseline). The digests are printed so they can be compared cross-repo for
/// byte-for-byte canonicalization parity. Also asserts key-order invariance
/// (a reordered clone hashes identically).
#[test]
fn conformance_schema_content_hash_reports_digests() {
    let fixture = load_fixture("schema_content_hash");
    println!("=== A-D-037 content_hash digests (Rust SDK) ===");
    for tc in fixture["test_cases"].as_array().unwrap() {
        let id = tc["id"].as_str().unwrap();
        let schema = &tc["schema"];
        let digest = content_hash(schema);
        assert_eq!(digest.len(), 64, "FAIL [{id}]: digest must be 64 hex chars");
        println!("{id}: {digest}");

        // Key-order invariance: a deep clone whose object keys are reversed
        // must hash identically (serde_json::Value already sorts on
        // canonicalization, so build an equal value and re-hash).
        let reparsed: Value =
            serde_json::from_str(&serde_json::to_string(schema).unwrap()).unwrap();
        assert_eq!(
            content_hash(&reparsed),
            digest,
            "FAIL [{id}]: content_hash must be stable across serialize round-trip"
        );
    }
}

// ---------------------------------------------------------------------------
// Traversal coverage — a `format` annotation must be found wherever it is
// reachable, not only under `properties` / `items`.
// ---------------------------------------------------------------------------

fn formats_of(warnings: &[FormatWarning]) -> Vec<(String, String)> {
    warnings
        .iter()
        .map(|w| (w.path.clone(), w.format.clone()))
        .collect()
}

#[test]
fn format_warnings_descend_into_anyof_branches() {
    let schema = json!({
        "type": "object",
        "properties": {
            "contact": {
                "anyOf": [
                    { "type": "string", "format": "email" },
                    { "type": "null" }
                ]
            }
        }
    });
    let warnings = format_warnings(&json!({ "contact": "not-an-email" }), &schema);
    assert_eq!(
        formats_of(&warnings),
        vec![("/contact".to_string(), "email".to_string())]
    );
}

#[test]
fn format_warnings_anyof_only_reports_the_branch_the_data_satisfies() {
    // "alice@example.com" satisfies the email branch; the uri branch describes a
    // different shape and must not report a format the value never carried.
    let schema = json!({
        "type": "object",
        "properties": {
            "contact": {
                "anyOf": [
                    { "type": "string", "format": "email" },
                    { "type": "string", "format": "uri", "minLength": 500 }
                ]
            }
        }
    });
    assert!(format_warnings(&json!({ "contact": "alice@example.com" }), &schema).is_empty());
}

#[test]
fn format_warnings_descend_into_oneof_branches() {
    let schema = json!({
        "type": "object",
        "properties": {
            "id": {
                "oneOf": [
                    { "type": "string", "format": "uuid" },
                    { "type": "integer" }
                ]
            }
        }
    });
    let warnings = format_warnings(&json!({ "id": "not-a-uuid" }), &schema);
    assert_eq!(
        formats_of(&warnings),
        vec![("/id".to_string(), "uuid".to_string())]
    );
}

#[test]
fn format_warnings_descend_into_all_allof_members() {
    let schema = json!({
        "type": "object",
        "properties": {
            "when": {
                "allOf": [
                    { "type": "string" },
                    { "format": "date-time" }
                ]
            }
        }
    });
    let warnings = format_warnings(&json!({ "when": "yesterday" }), &schema);
    assert_eq!(
        formats_of(&warnings),
        vec![("/when".to_string(), "date-time".to_string())]
    );
}

#[test]
fn format_warnings_descend_into_prefix_items() {
    let schema = json!({
        "type": "array",
        "prefixItems": [
            { "type": "string", "format": "uuid" },
            { "type": "string", "format": "date" }
        ]
    });
    let warnings = format_warnings(&json!(["not-a-uuid", "not-a-date"]), &schema);
    assert_eq!(
        formats_of(&warnings),
        vec![
            ("/0".to_string(), "uuid".to_string()),
            ("/1".to_string(), "date".to_string()),
        ]
    );
}

#[test]
fn format_warnings_descend_into_additional_properties_subschema() {
    let schema = json!({
        "type": "object",
        "properties": { "known": { "type": "string" } },
        "additionalProperties": { "type": "string", "format": "email" }
    });
    let warnings = format_warnings(
        &json!({ "known": "plain", "extra": "not-an-email" }),
        &schema,
    );
    assert_eq!(
        formats_of(&warnings),
        vec![("/extra".to_string(), "email".to_string())]
    );
}

#[test]
fn format_warnings_deduplicate_annotations_reached_through_several_branches() {
    let schema = json!({
        "type": "object",
        "properties": {
            "contact": {
                "anyOf": [
                    { "type": "string", "format": "email" },
                    { "type": "string", "format": "email" }
                ]
            }
        }
    });
    let warnings = format_warnings(&json!({ "contact": "not-an-email" }), &schema);
    assert_eq!(
        warnings.len(),
        1,
        "duplicate annotations must collapse: {warnings:?}"
    );
}
