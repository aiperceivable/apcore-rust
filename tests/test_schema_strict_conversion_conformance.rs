//! Cross-language conformance test for Algorithm A23 `to_strict_schema()`
//! (PROTOCOL_SPEC §4.16 / ALGORITHMS A23).
//!
//! Consumes the canonical `schema_strict_conversion.json` fixture shipped by
//! the `apcore` spec repo (sibling directory or `APCORE_SPEC_REPO`).
//!
//! DRIVER CONTRACT: this file MUST drive [`apcore::to_strict_schema`] — the A23
//! entry point — not the exporter and not the binding wrapper. A23 is the
//! shared deterministic surface; the three SDKs must emit the same strict
//! schema for the same input.

use std::path::PathBuf;

use serde_json::Value;

use apcore::to_strict_schema;

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
        sibling.display()
    );
}

fn load_fixture(name: &str) -> Value {
    let path = find_fixtures_root().join(format!("{name}.json"));
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()))
}

#[test]
fn schema_strict_conversion_fixture_parity() {
    let fixture = load_fixture("schema_strict_conversion");
    let cases = fixture["test_cases"].as_array().expect("test_cases array");
    assert!(!cases.is_empty(), "fixture has no test cases");

    let mut ids: Vec<&str> = Vec::new();
    for case in cases {
        let id = case["id"].as_str().expect("case id");
        ids.push(id);
        let schema = &case["schema"];
        let expected = &case["expected"];

        let before = schema.clone();
        let got = to_strict_schema(schema);

        assert_eq!(
            &got,
            expected,
            "[{id}] strict-schema mismatch.\n  description: {}\n  input: {schema}",
            case["description"].as_str().unwrap_or("(none)")
        );
        // A23 MUST deep-copy — the caller's schema is never mutated.
        assert_eq!(&before, schema, "[{id}] to_strict_schema mutated its input");
    }

    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "duplicate case ids in fixture");
}
