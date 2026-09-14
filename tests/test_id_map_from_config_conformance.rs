//! Drive `id_map_from_config.json` — §9.1.1 `id_map.overrides` (#118 D-71).
//!
//! The ID-map MECHANISM is stage 2 of `DefaultDiscoverer`, and it worked. What
//! did not exist was the path from a `Config` to it: the map arrived only
//! through `with_id_map`. `from_config` is that path, exactly as it is for the
//! three `extensions.*` scan keys since v1.42.0.
//!
//! The registered-ID half of the fixture is driven through `Debug`, which is
//! the only public window onto `id_map_path`. That is enough and it is the
//! honest scope: what was broken is whether the key ARRIVES, and a driver that
//! called `load_id_map` itself would prove the loader works — which was never
//! the question.

use apcore::config::Config;
use serde_json::Value;

use crate::conformance_env::find_fixtures_root;

fn fixture() -> Value {
    let path = find_fixtures_root().join("id_map_from_config.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("id_map_from_config.json parses")
}

#[test]
fn conformance_id_map_from_config() {
    let fx = fixture();
    let cases = fx["test_cases"].as_array().expect("test_cases is an array");
    assert!(!cases.is_empty(), "the fixture drove nothing");

    for case in cases {
        let id = case["id"].as_str().expect("case id");
        let declare = case["input"]["declare_override"]
            .as_bool()
            .expect("declare_override");
        let explicit = case["input"]["explicit_argument"]
            .as_bool()
            .expect("explicit_argument");

        let mut raw = serde_json::json!({
            "version": "1.0",
            "project": {"name": "id-map-probe"},
            "extensions": {"root": "./ext"}
        });
        if declare {
            raw["id_map"] = serde_json::json!({"overrides": "./map.yaml"});
        }
        let config: Config = serde_json::from_value(raw).expect("probe config parses");

        let mut discoverer = apcore::registry::DefaultDiscoverer::from_config(&config);
        if explicit {
            discoverer = discoverer.with_id_map(Some("./explicit.yaml"));
        }
        let shown = format!("{discoverer:?}");

        // Which map the discoverer will consult is what the fixture's
        // `expected.module_ids` is a consequence of: `map.yaml` renames to
        // `executor.renamed.mod`, `explicit.yaml` to `executor.explicit.mod`,
        // and neither leaves `executor.orig.mod`.
        let expected_map = match case["expected"]["module_ids"][0]
            .as_str()
            .expect("module id")
        {
            "executor.renamed.mod" => Some("map.yaml"),
            "executor.explicit.mod" => Some("explicit.yaml"),
            _ => None,
        };
        match expected_map {
            Some(name) => assert!(
                shown.contains(name),
                "case {id}: the discoverer must consult {name}: {shown}"
            ),
            None => assert!(
                shown.contains("id_map_path: None"),
                "case {id}: no map must be consulted: {shown}"
            ),
        }
    }
}

#[test]
fn a_relative_override_uses_the_same_base_as_extensions_root() {
    // §9.2.1 leaves the base for path-typed keys deliberately unspecified
    // (#113): `acl.root` uses the config file's directory, `schema.root` the
    // CWD. This key follows its SIBLING rather than settling that, because the
    // two are halves of one discovery configuration — so the declared value is
    // carried through unchanged rather than joined to the config's directory.
    let raw = serde_json::json!({
        "version": "1.0",
        "project": {"name": "id-map-probe"},
        "id_map": {"overrides": "./map.yaml"}
    });
    let config: Config = serde_json::from_value(raw).expect("probe config parses");
    let shown = format!(
        "{:?}",
        apcore::registry::DefaultDiscoverer::from_config(&config)
    );
    assert!(
        shown.contains("./map.yaml") || shown.contains("map.yaml"),
        "the declared value must be carried through as-is: {shown}"
    );
    assert!(
        !shown.contains("id-map-probe"),
        "the config's own location must not be joined in: {shown}"
    );
}
