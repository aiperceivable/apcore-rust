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

use crate::conformance_env::find_fixtures_root;

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
///
/// CORRECTED (apcore#93): this asserted `missing.is_empty()` and never read the
/// case's declared `expected.missing`, so mutating that list left the test
/// green and the case was pinned by no apcore-rust driver. Its two sibling
/// tests in this file already compared against the fixture's own list; this one
/// now does the same, so a fixture that ever records a known exception forces
/// the driver to accept exactly that one and no other.
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
    let mut missing: Vec<&String> = canonical
        .keys()
        .filter(|k| resolved.get(k).is_none())
        .collect();
    missing.sort();
    let case = fixture_case(&fx, "sdk_reproduces_every_canonical_default");
    assert_eq!(
        serde_json::to_value(&missing).unwrap(),
        case["expected"]["missing"],
        "defaults.schema.json declares defaults this SDK does not resolve: {missing:?}"
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

// ---------------------------------------------------------------------------
// §9.14 `reject_unknown_framework_keys` — both tiers, driven from the fixture
// ---------------------------------------------------------------------------

/// Expand the fixture's flat `{"executor.zz": "kept"}` config map into the
/// nested object a real document carries, and write it to disk.
///
/// Every case below goes through `Config::load` from a real file. The
/// synthetic `Config::from_defaults()` + `.set(…)` path cannot express this
/// defect: `set` writes a `user_namespaces` shadow entry no YAML file can
/// produce and skips `Config::deserialize` entirely, which is the step that
/// discards the key.
fn write_case_document(
    case: &serde_json::Value,
    namespace_mode: bool,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let flat = case["config"]
        .as_object()
        .expect("case.config is an object");

    let mut sections = serde_json::Map::new();
    let mut root = serde_json::Map::new();
    for (path, value) in flat {
        // `_config` is a reserved TOP-LEVEL namespace (§9.6.3) and never nests
        // under `apcore:`.
        let target = if path.starts_with("_config.") {
            &mut root
        } else {
            &mut sections
        };
        let parts: Vec<&str> = path.split('.').collect();
        let mut cursor = target;
        for part in &parts[..parts.len() - 1] {
            cursor = cursor
                .entry((*part).to_string())
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
                .as_object_mut()
                .expect("dot-path segment collides with a scalar");
        }
        cursor.insert(parts[parts.len() - 1].to_string(), value.clone());
    }

    if namespace_mode {
        // The `apcore:` block selects namespace mode; §9.10 step 2 runs the
        // sub-algorithm against it.
        sections.insert("version".to_string(), serde_json::json!("1.0.0"));
        root.insert(
            "apcore".to_string(),
            serde_json::Value::Object(sections.clone()),
        );
    } else {
        // Legacy mode: the whole file IS the apcore namespace (§9.14). The two
        // §9.1 required fields are added because a legacy document without
        // them fails validation for an unrelated reason — they are not part of
        // what the case is testing.
        root.insert("version".to_string(), serde_json::json!("1.0.0"));
        root.insert(
            "project".to_string(),
            serde_json::json!({ "name": "governance-fixture" }),
        );
        for (key, value) in sections {
            root.insert(key, value);
        }
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("apcore.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&serde_json::Value::Object(root)).expect("serialize"),
    )
    .expect("write config file");
    (dir, path)
}

/// Default tier: an undeclared key inside a framework section is RETAINED.
///
/// Per `driver_contract.default_tier_must_be_asserted_by_reading_it_back`, the
/// retained key is asserted by reading it back through `get()` — not by
/// checking that the load did not raise. Not-raising is also true of an
/// implementation that discarded the key at parse time, which is exactly the
/// defect this case exists to catch (apcore-rust#33 did it for every
/// `observability.*` subkey, and `executor` did it for every undeclared one
/// until §9.14).
#[test]
fn unknown_framework_key_is_retained_by_default() {
    let fx = fixture();
    let case = fixture_case(&fx, "unknown_framework_key_is_retained_by_default");
    let allowed = allowed_keys(&fx);
    let flat = case["config"]
        .as_object()
        .expect("case.config is an object");

    // Both modes: §9.14 clause (b) "applies in legacy mode too, where the whole
    // file *is* the `apcore` namespace", and the two take different branches
    // through `Config::deserialize`.
    for namespace_mode in [false, true] {
        let (_dir, path) = write_case_document(case, namespace_mode);
        let config = apcore::config::Config::load(&path).unwrap_or_else(|e| {
            panic!(
                "expected.load_succeeds is true but load failed in \
                 {} mode: {}",
                if namespace_mode {
                    "namespace"
                } else {
                    "legacy"
                },
                e.message
            )
        });
        assert_eq!(case["expected"]["load_succeeds"], serde_json::json!(true));
        assert_eq!(case["expected"]["error_raised"], serde_json::json!(false));

        // Every key the case declares must read back as written, undeclared or
        // not. Reported as a list so a driver failure names the offenders.
        let dropped: Vec<String> = flat
            .iter()
            .filter(|(key, want)| config.get(key).as_ref() != Some(*want))
            .map(|(key, want)| format!("{key}: want {want}, got {:?}", config.get(key)))
            .collect();
        assert!(
            dropped.is_empty(),
            "keys written into a real config file did not read back through \
             get() in {} mode: {dropped:?}",
            if namespace_mode {
                "namespace"
            } else {
                "legacy"
            }
        );

        // Pin the case's own two expectations against the right keys, derived
        // from the fixture's canonical key surface rather than hardcoded here.
        let undeclared: Vec<&String> = flat.keys().filter(|k| !allowed.contains(*k)).collect();
        assert_eq!(
            undeclared.len(),
            1,
            "the retention case must declare exactly one undeclared key, got \
             {undeclared:?}"
        );
        assert_eq!(
            config.get(undeclared[0]),
            Some(case["expected"]["get_undeclared_key"].clone()),
            "`{}` is the key the case is about; it MUST be retained and \
             readable through get()",
            undeclared[0]
        );
        let declared: Vec<&String> = flat.keys().filter(|k| allowed.contains(*k)).collect();
        assert_eq!(
            declared.len(),
            1,
            "expected one declared key, got {declared:?}"
        );
        assert_eq!(
            config.get(declared[0]),
            Some(case["expected"]["get_declared_key"].clone()),
            "retaining the undeclared key must not disturb `{}`",
            declared[0]
        );
    }
}

/// Strict tier: the same key raises `CONFIG_INVALID`, and the error enumerates
/// EVERY offending key.
///
/// Per `driver_contract.strict_enumerates_every_key`, the case declares two
/// undeclared keys in DIFFERENT sections on purpose and both must appear. An
/// implementation that fails on the first satisfies a raise-only assertion
/// while forcing the operator into one restart per typo.
#[test]
fn unknown_framework_key_is_rejected_under_strict() {
    let fx = fixture();
    let case = fixture_case(&fx, "unknown_framework_key_is_rejected_under_strict");
    let expected_keys: Vec<&str> = case["expected"]["error_names_all_offending_keys"]
        .as_array()
        .expect("error_names_all_offending_keys is an array")
        .iter()
        .map(|v| v.as_str().expect("key is a string"))
        .collect();

    for namespace_mode in [false, true] {
        let mode = if namespace_mode {
            "namespace"
        } else {
            "legacy"
        };
        let (_dir, path) = write_case_document(case, namespace_mode);
        let err = apcore::config::Config::load(&path).expect_err(&format!(
            "expected.load_succeeds is false, but the {mode}-mode document loaded"
        ));
        assert_eq!(
            format!("{:?}", err.code),
            "ConfigInvalid",
            "expected.error_code is CONFIG_INVALID, got {:?}: {}",
            err.code,
            err.message
        );
        // Report the offenders, not a boolean.
        let missing: Vec<&str> = expected_keys
            .iter()
            .copied()
            .filter(|key| !err.message.contains(key))
            .collect();
        assert!(
            missing.is_empty(),
            "the {mode}-mode strict error named only some of the offending \
             keys — missing {missing:?}. §9.14: the error MUST enumerate every \
             offending key rather than failing on the first, so one restart is \
             enough to see the whole problem.\nfull message: {}",
            err.message
        );
    }
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
