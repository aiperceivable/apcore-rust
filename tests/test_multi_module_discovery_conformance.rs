// Cross-language conformance tests for Multi-Module Discovery (Issue #32).
//
// Fixture source: apcore/conformance/fixtures/multi_module_discovery.json
// Spec reference: apcore/docs/features/multi-module-discovery.md
//                 apcore/PROTOCOL_SPEC.md §2.1.1
//
// The eight fixture cases exercise:
//   single_class_id_unchanged           — single-class identity guarantee
//   two_classes_distinct_ids            — multi-class ID derivation
//   class_name_snake_case_addition      — snake_case: simple PascalCase
//   class_name_snake_case_math_ops      — snake_case: word boundary
//   class_name_snake_case_https_sender  — snake_case: ALLCAPS run
//   conflict_same_segment               — MODULE_ID_CONFLICT error code
//   full_id_grammar_valid               — derived ID matches canonical grammar
//   disabled_by_default                 — multi_class=false → only base_id

#![allow(clippy::missing_panics_doc)]
// `apcore::errors::ModuleError` is intentionally large (rich structured error
// for an SDK); boxing it across the test API would diverge from the library's
// public surface. Mirrors the crate-wide allow in src/lib.rs.
#![allow(clippy::result_large_err)]

use std::path::PathBuf;

use serde_json::Value;

use apcore::errors::ErrorCode;
use apcore::registry::registry::MODULE_ID_PATTERN;
use apcore::registry::{
    class_name_to_segment, derive_module_ids, DiscoveredClass, DiscoveryConfig,
};

// ---------------------------------------------------------------------------
// Fixture loading (mirrors tests/conformance_test.rs discovery)
// ---------------------------------------------------------------------------

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

fn fixture_case<'a>(fixture: &'a Value, id: &str) -> &'a Value {
    fixture["test_cases"]
        .as_array()
        .expect("test_cases must be an array")
        .iter()
        .find(|c| c["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("fixture case '{id}' not present"))
}

fn parse_classes(input: &Value) -> Vec<DiscoveredClass> {
    input["classes"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|c| DiscoveredClass {
                    name: c["name"].as_str().unwrap().to_string(),
                    implements_module: c["implements_module"].as_bool().unwrap_or(true),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn run_derive(case: &Value) -> Result<Vec<String>, apcore::errors::ModuleError> {
    let input = &case["input"];
    let file_path = PathBuf::from(input["file_path"].as_str().unwrap());
    let extensions_root = input["extensions_root"].as_str().unwrap_or("extensions");
    let classes = parse_classes(input);
    let config = DiscoveryConfig {
        multi_class: input["multi_class_enabled"].as_bool().unwrap_or(false),
    };
    derive_module_ids(&file_path, extensions_root, &classes, &config)
}

// ---------------------------------------------------------------------------
// Conformance tests — one per fixture case (8 total)
// ---------------------------------------------------------------------------

#[test]
fn conformance_single_class_id_unchanged() {
    let fixture = load_fixture("multi_module_discovery");
    let case = fixture_case(&fixture, "single_class_id_unchanged");
    let ids = run_derive(case).expect("expected success");
    let expected: Vec<String> = case["expected"]["module_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, expected, "single-class identity guarantee");
}

#[test]
fn conformance_two_classes_distinct_ids() {
    let fixture = load_fixture("multi_module_discovery");
    let case = fixture_case(&fixture, "two_classes_distinct_ids");
    let ids = run_derive(case).expect("expected success");
    let expected: Vec<String> = case["expected"]["module_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, expected, "two-class ID derivation");
}

#[test]
fn conformance_class_name_snake_case_addition() {
    let fixture = load_fixture("multi_module_discovery");
    let case = fixture_case(&fixture, "class_name_snake_case_addition");
    let class_name = case["input"]["class_name"].as_str().unwrap();
    let expected = case["expected"]["class_segment"].as_str().unwrap();
    assert_eq!(class_name_to_segment(class_name), expected);
}

#[test]
fn conformance_class_name_snake_case_math_ops() {
    let fixture = load_fixture("multi_module_discovery");
    let case = fixture_case(&fixture, "class_name_snake_case_math_ops");
    let class_name = case["input"]["class_name"].as_str().unwrap();
    let expected = case["expected"]["class_segment"].as_str().unwrap();
    assert_eq!(class_name_to_segment(class_name), expected);
}

#[test]
fn conformance_class_name_snake_case_https_sender() {
    let fixture = load_fixture("multi_module_discovery");
    let case = fixture_case(&fixture, "class_name_snake_case_https_sender");
    let class_name = case["input"]["class_name"].as_str().unwrap();
    let expected = case["expected"]["class_segment"].as_str().unwrap();
    assert_eq!(class_name_to_segment(class_name), expected);
}

#[test]
fn conformance_conflict_same_segment() {
    let fixture = load_fixture("multi_module_discovery");
    let case = fixture_case(&fixture, "conflict_same_segment");
    let err = run_derive(case).expect_err("expected MODULE_ID_CONFLICT");
    assert_eq!(
        err.code,
        ErrorCode::ModuleIdConflict,
        "expected MODULE_ID_CONFLICT"
    );
    let expected_segment = case["expected"]["error"]["conflicting_segment"]
        .as_str()
        .unwrap();
    let actual_segment = err
        .details
        .get("conflicting_segment")
        .and_then(|v| v.as_str())
        .expect("conflicting_segment must appear in error details");
    assert_eq!(actual_segment, expected_segment);
}

#[test]
fn conformance_full_id_grammar_valid() {
    // The fixture's `expected.module_ids` field is illustrative only — the
    // sole input class triggers the single-class identity guarantee, which
    // returns the bare `base_id` (`executor.math.arithmetic`) rather than the
    // illustrative `executor.math.arithmetic.addition` string in the fixture.
    // The apcore-python conformance test for this fixture case (driven by
    // `_CANONICAL_ID_RE.match`) likewise only verifies grammar conformance
    // and ignores the fixture's `expected.module_ids` — see
    // `apcore-python/tests/registry/test_multi_class.py::TestGrammarConformance`.
    //
    // To exercise the multi-class derivation path on top of grammar
    // conformance, we additionally derive IDs for a 2-class variant of the
    // same input and assert those match the grammar too.
    let fixture = load_fixture("multi_module_discovery");
    let case = fixture_case(&fixture, "full_id_grammar_valid");
    let pattern = regex::Regex::new(MODULE_ID_PATTERN).unwrap();

    let ids = run_derive(case).expect("expected success");
    for id in &ids {
        assert!(
            pattern.is_match(id),
            "single-class derived ID '{id}' must match canonical grammar"
        );
    }

    // Two-class variant — exercises the multi-class derivation path.
    let file_path = PathBuf::from(case["input"]["file_path"].as_str().unwrap());
    let extensions_root = case["input"]["extensions_root"].as_str().unwrap();
    let two_classes = vec![
        DiscoveredClass {
            name: "Addition".to_string(),
            implements_module: true,
        },
        DiscoveredClass {
            name: "Subtraction".to_string(),
            implements_module: true,
        },
    ];
    let config = DiscoveryConfig::with_multi_class();
    let multi_ids = derive_module_ids(&file_path, extensions_root, &two_classes, &config).unwrap();
    for id in &multi_ids {
        assert!(
            pattern.is_match(id),
            "multi-class derived ID '{id}' must match canonical grammar"
        );
    }
    assert_eq!(
        multi_ids,
        vec![
            "executor.math.arithmetic.addition".to_string(),
            "executor.math.arithmetic.subtraction".to_string(),
        ]
    );
}

#[test]
fn conformance_disabled_by_default() {
    let fixture = load_fixture("multi_module_discovery");
    let case = fixture_case(&fixture, "disabled_by_default");
    let ids = run_derive(case).expect("expected success");
    let expected: Vec<String> = case["expected"]["module_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, expected, "disabled-by-default returns base_id only");
}
