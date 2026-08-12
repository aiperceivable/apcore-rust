//! Cross-language conformance test for JSON Schema keyword parity at the
//! validation boundary (PROTOCOL_SPEC §4.15, JSON Schema 2020-12 §6 and §10.2).
//!
//! Consumes the canonical `schema_keyword_parity.json` fixture shipped by the
//! `apcore` spec repo (sibling directory or `APCORE_SPEC_REPO`).
//!
//! The fixture's `driver_contract` requires driving the code path a module
//! invocation actually takes. In this SDK that is
//! [`apcore::executor::validate_against_schema`] — the function
//! `src/builtin_steps.rs` calls for input and output validation. Driving
//! `SchemaValidator` instead would defeat the fixture's purpose.

use std::path::PathBuf;

use serde_json::Value;

use apcore::executor::validate_against_schema;

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

#[test]
fn conformance_schema_keyword_parity() {
    let fixture = load_fixture("schema_keyword_parity");
    let cases = fixture["test_cases"]
        .as_array()
        .expect("fixture must carry a test_cases array");
    assert!(!cases.is_empty(), "fixture must carry at least one case");

    let mut failures: Vec<String> = Vec::new();

    for tc in cases {
        let id = tc["id"].as_str().expect("every case needs an id");
        let description = tc["description"].as_str().unwrap_or("");
        let schema = &tc["schema"];
        let input = &tc["input"];
        let expected_valid = tc["expected_valid"]
            .as_bool()
            .unwrap_or_else(|| panic!("FAIL [{id}]: fixture missing expected_valid"));

        // The executor boundary: exactly what builtin_steps.rs runs for a
        // module's `input_schema` / `output_schema`.
        let outcome = validate_against_schema(input, schema, "Input");
        let actual_valid = outcome.is_ok();

        if actual_valid != expected_valid {
            let actual = match &outcome {
                Ok(()) => "valid".to_string(),
                Err(e) => format!("invalid ({:?}: {})", e.code, e.message),
            };
            failures.push(format!(
                "  [{id}] {description}\n    \
                 schema:   {schema}\n    \
                 input:    {input}\n    \
                 expected: {}\n    \
                 actual:   {actual}",
                if expected_valid { "valid" } else { "invalid" },
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "schema_keyword_parity: {}/{} case(s) diverge from the spec fixture \
         (cross-language divergence in validate_against_schema):\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}
