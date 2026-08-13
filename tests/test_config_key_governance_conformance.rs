//! Drive `config_key_governance.json` — the configuration key-surface guard.
//!
//! The fixture derives its `allowed_keys` / `canonical_defaults` from the
//! canonical schemas, so this suite is really asking: does `apcore::config`'s
//! idea of the config surface still match `apcore/schemas/`?
//!
//! It exists because four separate instances of the same defect shipped
//! undetected: `schema.validation.*` validated by every SDK and declared by no
//! schema, a frozen `version`/`project` default pair that made the required-field
//! check unreachable, `middleware.circuit_breaker.*` forbidden by
//! `apcore-config.schema.json` yet validated everywhere and read nowhere, and a
//! missing Rust default table that resolved 15 documented keys to null. None was
//! findable by any existing test.

use std::collections::HashSet;
use std::path::PathBuf;

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
        "Cannot find apcore conformance fixtures. Set APCORE_SPEC_REPO or clone \
         apcore as a sibling at {}",
        manifest_dir.parent().unwrap().join("apcore").display()
    );
}

fn fixture() -> serde_json::Value {
    let path = find_fixtures_root().join("config_key_governance.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("fixture parses")
}

fn fixture_case<'a>(fx: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    fx["test_cases"]
        .as_array()
        .expect("test_cases is an array")
        .iter()
        .find(|c| c["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("fixture case '{id}' not present"))
}

fn allowed_keys(fx: &serde_json::Value) -> HashSet<String> {
    fx["allowed_keys"]
        .as_array()
        .expect("allowed_keys is an array")
        .iter()
        .map(|v| v.as_str().expect("key is a string").to_string())
        .collect()
}

/// `apcore-config.schema.json` is `additionalProperties: false`, so a default
/// for a key no schema declares means a user config carrying that key fails the
/// canonical schema while the SDK quietly supplies a value for it.
#[test]
fn config_defaults_declare_no_undeclared_key() {
    let fx = fixture();
    let allowed = allowed_keys(&fx);
    let mut violations: Vec<&str> = apcore::config::config_default_keys()
        .into_iter()
        .filter(|k| !allowed.contains(*k))
        .collect();
    violations.sort_unstable();
    // `violations` — compared against the list the fixture declares, not merely
    // asserted empty: if the fixture ever accepts a known exception, the driver
    // must accept exactly that one and no other.
    let case = fixture_case(&fx, "sdk_default_table_declares_no_undeclared_key");
    assert_eq!(
        serde_json::to_value(&violations).unwrap(),
        case["expected"]["violations"],
        "CONFIG_DEFAULTS declares keys the fixture does not list under `violations`.\n\
         Either add them to a schema in apcore/schemas/ (and regenerate the \
         fixture) or remove them from CONFIG_DEFAULTS."
    );
}

/// Validating a key the canonical schema forbids is worse than not validating
/// it: it tells the operator the key is understood.
#[test]
fn config_constraints_declare_no_undeclared_key() {
    let fx = fixture();
    let allowed = allowed_keys(&fx);
    let mut violations: Vec<&str> = apcore::config::CONSTRAINED_CONFIG_KEYS
        .iter()
        .copied()
        .filter(|k| !allowed.contains(*k))
        .collect();
    violations.sort_unstable();
    // `violations`
    let case = fixture_case(&fx, "sdk_constraint_table_declares_no_undeclared_key");
    assert_eq!(
        serde_json::to_value(&violations).unwrap(),
        case["expected"]["violations"],
        "validate_key_constraint covers keys the fixture does not list under `violations`"
    );
}

/// A missing entry means the key resolves to `None` here while its peers return
/// the documented value — the exact defect that left 15 keys null in Rust.
#[test]
fn config_defaults_reproduce_every_canonical_default() {
    let fx = fixture();
    let canonical = fx["canonical_defaults"]
        .as_object()
        .expect("canonical_defaults is an object");
    // Compare the RESOLVED default view, not the const table: an SDK may
    // implement a default as a typed struct field (Rust's ExecutorConfig,
    // ObservabilityConfig) rather than a table entry, and both are legitimate.
    // The behavioural question is "does this key resolve to the documented
    // value?", which is what a caller observes.
    let resolved = apcore::config::Config::from_defaults();
    let missing: Vec<&String> = canonical
        .keys()
        .filter(|k| resolved.get(k).is_none())
        .collect();
    assert!(
        missing.is_empty(),
        "defaults.schema.json declares defaults CONFIG_DEFAULTS does not carry: {missing:?}"
    );
}

#[test]
fn config_default_values_match_canonical_defaults() {
    let fx = fixture();
    let canonical = fx["canonical_defaults"]
        .as_object()
        .expect("canonical_defaults is an object");
    let resolved = apcore::config::Config::from_defaults();
    let mut mismatched = Vec::new();
    for (key, want) in canonical {
        let got = resolved.get(key);
        // Compare numerically where both are numbers so 1.0 and 1 agree, as the
        // fixture's driver_contract requires.
        let equal = match (&got, want) {
            (Some(g), w) if g.is_number() && w.is_number() => g.as_f64() == w.as_f64(),
            (Some(g), w) => g == w,
            (None, _) => false,
        };
        if !equal {
            mismatched.push(format!("{key}: sdk={got:?} canonical={want}"));
        }
    }
    // `mismatched`
    mismatched.sort();
    let case = fixture_case(&fx, "sdk_default_values_match_canonical_defaults");
    assert_eq!(
        serde_json::to_value(&mismatched).unwrap(),
        case["expected"]["mismatched"],
        "CONFIG_DEFAULTS disagrees with defaults.schema.json:\n  {}",
        mismatched.join("\n  ")
    );
}

/// Guard the guard: if the fixture ever stops naming its generator, the next
/// person to hand-edit it will make it a second source of truth.
#[test]
fn governance_fixture_is_derived_not_authored() {
    let fx = fixture();
    let sources = fx["driver_contract"]["sources"]
        .as_str()
        .expect("driver_contract.sources is a string");
    assert!(sources.contains("regenerated"), "{sources}");
    assert!(sources.contains("do NOT hand-edit"), "{sources}");
    let canonical_sources: Vec<&str> = fx["canonical_sources"]
        .as_array()
        .expect("canonical_sources is an array")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        canonical_sources,
        vec![
            "schemas/apcore-config.schema.json",
            "schemas/defaults.schema.json",
            "schemas/sys-modules.schema.json",
        ]
    );
}
