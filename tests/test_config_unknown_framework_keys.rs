//! `PROTOCOL_SPEC` §9.14 `reject_unknown_framework_keys` — both tiers.
//!
//! Every framework section in `schemas/apcore-config.schema.json` is
//! `additionalProperties: false`. §9.14 enforces that closedness in two tiers,
//! and this file drives both:
//!
//! * **Default (`_config.strict` absent or false).** An undeclared key inside a
//!   framework section **MUST** be retained and readable through `get()`.
//!   Implementations **MUST NOT** silently discard it — "the operator wrote it
//!   and it vanished" is indistinguishable from "the operator never wrote it".
//! * **`_config.strict: true`.** The key **MUST** raise `CONFIG_INVALID`, and
//!   the error **MUST** enumerate *every* offending key rather than failing on
//!   the first, so one restart shows the whole problem.
//!
//! ## Why the default tier is the sharp one for this SDK
//!
//! apcore-rust models `executor` and `observability` as typed structs sitting
//! OUTSIDE the `#[serde(flatten)] user_namespaces` bag. Serde drops what a
//! typed struct does not model, silently, at parse time — that is exactly how
//! apcore-rust#33 (every `observability.*` subkey) and the `executor` namespace
//! gap shipped. Sections that live in the flatten bag (`acl`, `extensions`,
//! `schema`, `sys_modules`, …) retain undeclared keys by construction, so the
//! interesting cases here are the typed ones.
//!
//! ## Every Config in this file comes from a real file on disk
//!
//! `Config::from_defaults()` + `.set(…)` cannot express this defect at all:
//! `set` writes a `user_namespaces` shadow entry no YAML file can produce, and
//! it skips `Config::deserialize`, which is the one step that discards the key.
//! A test on that path asserts against a state no deployment can reach — which
//! is precisely how #33 survived 198 of 288 Config tests. Nothing in this file
//! may be rewritten onto that path.
//!
//! ## Deliberately NOT registered in the coverage guard's `LOAD_SUITES`
//!
//! `test_config_load_coverage_guard.rs` asks "is every config section written
//! into a real file by some load suite, and asserted there?". This file writes
//! *every* framework section into one document (see
//! [`strict_accepts_a_document_whose_framework_keys_are_all_declared`]), so
//! adding it to that corpus would satisfy the guard for every section forever
//! on the strength of one closedness assertion. The guard's value is that a new
//! section has to earn real load-path coverage; keep this file out of it.

use apcore::config::{Config, ConfigMode};
use apcore::errors::ErrorCode;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// Document construction — always a real file, never `set()`
// ---------------------------------------------------------------------------

/// Expand a flat `{"executor.zz": "kept"}` map into the nested object a YAML
/// document actually carries.
fn expand_dot_paths(flat: &[(&str, Value)]) -> Value {
    let mut root = Map::new();
    for (path, value) in flat {
        let parts: Vec<&str> = path.split('.').collect();
        let mut cursor = &mut root;
        for part in &parts[..parts.len() - 1] {
            cursor = cursor
                .entry((*part).to_string())
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .expect("dot-path segment collides with a scalar");
        }
        cursor.insert(parts[parts.len() - 1].to_string(), value.clone());
    }
    Value::Object(root)
}

/// Write `doc` as a real JSON config file and load it the way a deployment
/// does. JSON rather than YAML only because both go through `Config::load`'s
/// extension dispatch into the same `Config::deserialize`, and JSON is the
/// spelling a `serde_json::Value` fixture serializes to without a translation
/// step that could itself drop a key.
fn load_doc(
    doc: &Value,
) -> (
    tempfile::TempDir,
    Result<Config, apcore::errors::ModuleError>,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("apcore.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(doc).expect("serialize doc"),
    )
    .expect("write config file");
    let loaded = Config::load(&path);
    (dir, loaded)
}

/// The §9.1 required fields, which a legacy-mode document must declare for
/// `validate()` to reach any other check. Not part of what is under test —
/// without them every legacy case would fail for the wrong reason.
fn legacy_doc(flat: &[(&str, Value)]) -> Value {
    let mut with_required: Vec<(&str, Value)> = vec![
        ("version", json!("1.0.0")),
        ("project.name", json!("framework-keys-fixture")),
    ];
    with_required.extend(flat.iter().cloned());
    expand_dot_paths(&with_required)
}

/// The same sections under an `apcore:` block, which selects namespace mode.
/// §9.14 clause (b) applies in both modes, and they take different branches
/// through `Config::deserialize` and `Config::validate`.
fn namespace_doc(flat: &[(&str, Value)]) -> Value {
    let mut inner: Vec<(&str, Value)> = vec![("version", json!("1.0.0"))];
    let mut outer: Vec<(&str, Value)> = Vec::new();
    for (path, value) in flat {
        // `_config` is a reserved TOP-LEVEL namespace (§9.6.3); it never nests
        // under `apcore:`.
        if path.starts_with("_config.") {
            outer.push((path, value.clone()));
        } else {
            inner.push((path, value.clone()));
        }
    }
    let mut doc = expand_dot_paths(&outer);
    doc.as_object_mut()
        .expect("root is an object")
        .insert("apcore".to_string(), expand_dot_paths(&inner));
    doc
}

fn expect_loaded(doc: &Value) -> (tempfile::TempDir, Config) {
    let (dir, loaded) = load_doc(doc);
    match loaded {
        Ok(config) => (dir, config),
        Err(e) => panic!("a real config file must load: {} ({:?})", e.message, e.code),
    }
}

// ---------------------------------------------------------------------------
// Default tier — the key is RETAINED, and asserted by reading it back
// ---------------------------------------------------------------------------

/// `executor` is a typed struct outside the flatten bag, so an undeclared key
/// reached `ExecutorConfig`, was not modelled, and serde discarded it before
/// any accessor existed to see it.
///
/// Asserted by reading the key BACK through `get()`. Asserting only that the
/// load did not error would pass against exactly the implementation this case
/// exists to catch: discarding at parse time raises nothing.
#[test]
fn undeclared_executor_key_is_retained_and_readable_after_load() {
    let (_dir, config) = expect_loaded(&legacy_doc(&[
        ("executor.max_call_depth", json!(7)),
        ("executor.zz_undeclared", json!("kept")),
    ]));

    assert_eq!(
        config.get("executor.zz_undeclared"),
        Some(json!("kept")),
        "an undeclared `executor` subkey written into a real config file came \
         back None — it is being discarded at parse time, which §9.14 forbids \
         under the default tier"
    );
    assert_eq!(
        config.get_declared("executor.zz_undeclared"),
        Some(json!("kept")),
        "the key was DECLARED by the document; it must not look defaulted"
    );
    assert_eq!(
        config.get("executor.max_call_depth"),
        Some(json!(7)),
        "retaining the raw block must not disturb the typed leaves"
    );
}

/// Namespace mode nests the section under `apcore:`, which
/// `Config::deserialize` merges to the top level before the typed structs
/// consume it. A fix that only covered the legacy branch would be a half fix.
#[test]
fn undeclared_executor_key_is_retained_in_namespace_mode() {
    let (_dir, config) = expect_loaded(&namespace_doc(&[
        ("executor.max_call_depth", json!(7)),
        ("executor.zz_undeclared", json!("kept")),
    ]));

    assert_eq!(config.mode, ConfigMode::Namespace);
    assert_eq!(config.get("executor.zz_undeclared"), Some(json!("kept")));
    assert_eq!(config.get("executor.max_call_depth"), Some(json!(7)));
}

/// The apcore-rust#33 section, pinned on the load path from this file too, so
/// the generalization cannot regress the case it generalizes.
#[test]
fn undeclared_observability_key_is_retained_and_readable_after_load() {
    let (_dir, config) = expect_loaded(&legacy_doc(&[
        ("observability.tracing.enabled", json!(true)),
        ("observability.zz_undeclared", json!("kept")),
    ]));

    assert_eq!(
        config.get("observability.zz_undeclared"),
        Some(json!("kept"))
    );
    assert_eq!(
        config.get("observability.tracing.enabled"),
        Some(json!(true))
    );
}

/// The flatten-bag sections retain by construction — there is no typed struct
/// to drop anything. Asserted rather than assumed: if one of them is ever
/// promoted to a typed field, this is where it stops being true.
#[test]
fn undeclared_keys_in_flatten_bag_sections_are_retained() {
    let (_dir, config) = expect_loaded(&legacy_doc(&[
        ("acl.default_effect", json!("deny")),
        ("acl.zz_undeclared", json!("kept-acl")),
        ("extensions.zz_undeclared", json!("kept-extensions")),
        ("schema.zz_undeclared", json!("kept-schema")),
        ("stream.zz_undeclared", json!("kept-stream")),
        ("sys_modules.zz_undeclared", json!("kept-sys-modules")),
    ]));

    for (key, expected) in [
        ("acl.zz_undeclared", "kept-acl"),
        ("extensions.zz_undeclared", "kept-extensions"),
        ("schema.zz_undeclared", "kept-schema"),
        ("stream.zz_undeclared", "kept-stream"),
        ("sys_modules.zz_undeclared", "kept-sys-modules"),
    ] {
        assert_eq!(
            config.get(key),
            Some(json!(expected)),
            "`{key}` is in the `#[serde(flatten)]` bag and must survive the load"
        );
    }
}

/// A retained key that only `get()` can see would be a second store all over
/// again. `namespace()`, `bind()` and `data()` read the same reconciled view,
/// so all four must agree — the invariant pinned for #33 and #34, extended to
/// the keys §9.14 now requires be kept.
#[test]
fn every_reader_agrees_about_a_retained_undeclared_key() {
    let (_dir, config) = expect_loaded(&legacy_doc(&[
        ("executor.max_call_depth", json!(7)),
        ("executor.zz_undeclared", json!("kept")),
    ]));

    assert_eq!(config.get("executor.zz_undeclared"), Some(json!("kept")));

    let ns = config.namespace("executor");
    assert_eq!(
        ns.get("zz_undeclared"),
        Some(&json!("kept")),
        "namespace(\"executor\") lost the retained key that get() resolves"
    );
    assert_eq!(
        ns.get("max_call_depth"),
        Some(&json!(7)),
        "the typed leaves must still be present alongside it"
    );

    let bound: Value = config.bind("executor").expect("bind executor");
    assert_eq!(bound["zz_undeclared"], json!("kept"));
    assert_eq!(bound["max_call_depth"], json!(7));

    let wire = config.data();
    assert_eq!(wire["executor"]["zz_undeclared"], json!("kept"));
    assert_eq!(
        wire["executor"]["max_call_depth"],
        json!(7),
        "the retained raw block must not clobber the typed leaves in the §9.1 \
         wire form — that clobber was apcore-rust#34"
    );

    // A container fetch has to agree with its own leaf.
    let container = config.get("executor").expect("container fetch");
    assert_eq!(container["zz_undeclared"], json!("kept"));
    assert_eq!(container["max_call_depth"], json!(7));
}

/// Retaining the file's raw `executor:` block introduces a stale copy of every
/// typed leaf. A later `set()` lands in the typed struct, and the typed struct
/// is overlaid LAST, so the caller reads back what they just set — not the
/// file's original.
#[test]
fn set_on_a_typed_leaf_still_wins_over_the_files_stale_raw_copy() {
    let (_dir, mut config) = expect_loaded(&legacy_doc(&[
        ("executor.max_call_depth", json!(7)),
        ("executor.zz_undeclared", json!("kept")),
    ]));

    config.set("executor.max_call_depth", json!(11));

    assert_eq!(config.get("executor.max_call_depth"), Some(json!(11)));
    assert_eq!(
        config.namespace("executor").get("max_call_depth"),
        Some(&json!(11)),
        "the file's stale `7` resurfaced through namespace() — the raw block is \
         winning over the typed struct"
    );
    assert_eq!(config.data()["executor"]["max_call_depth"], json!(11));
    assert_eq!(
        config.get("executor.zz_undeclared"),
        Some(json!("kept")),
        "the retained key must survive an unrelated set()"
    );
}

/// The same document that strict mode rejects must load cleanly by default.
/// §9.14: `allow_unknown` does not enter into it, and neither does a warning
/// that fails the load.
#[test]
fn default_tier_accepts_what_strict_rejects() {
    let flat = offending_document();
    let mut without_strict: Vec<(&str, Value)> = flat
        .iter()
        .filter(|(k, _)| *k != "_config.strict")
        .cloned()
        .collect();
    // Also prove `_config.strict: false` is the same as absent.
    without_strict.push(("_config.strict", json!(false)));

    let (_dir, config) = expect_loaded(&legacy_doc(&without_strict));
    assert_eq!(config.get("executor.zz_undeclared"), Some(json!("x")));
    assert_eq!(config.get("acl.zz_also_undeclared"), Some(json!("y")));
    assert_eq!(config.get("obs.zz_third_undeclared"), Some(json!("z")));
}

// ---------------------------------------------------------------------------
// Strict tier — CONFIG_INVALID naming EVERY offending key
// ---------------------------------------------------------------------------

/// Three undeclared keys in three different sections, one of them in a typed
/// section and two in flatten-bag sections.
fn offending_document() -> Vec<(&'static str, Value)> {
    vec![
        ("_config.strict", json!(true)),
        ("executor.zz_undeclared", json!("x")),
        ("acl.zz_also_undeclared", json!("y")),
        ("obs.zz_third_undeclared", json!("z")),
    ]
}

fn assert_enumerates_all(err: &apcore::errors::ModuleError, expected: &[&str]) {
    assert_eq!(
        err.code,
        ErrorCode::ConfigInvalid,
        "§9.14 mandates CONFIG_INVALID, got {:?}: {}",
        err.code,
        err.message
    );
    let missing: Vec<&str> = expected
        .iter()
        .copied()
        .filter(|key| !err.message.contains(key))
        .collect();
    assert!(
        missing.is_empty(),
        "the strict error named only some of the offending keys — missing \
         {missing:?}. §9.14: the error MUST enumerate EVERY offending key \
         rather than failing on the first, so one restart is enough to see the \
         whole problem.\nfull message: {}",
        err.message
    );
}

#[test]
fn strict_rejects_every_undeclared_framework_key_in_legacy_mode() {
    let (_dir, loaded) = load_doc(&legacy_doc(&offending_document()));
    let err = loaded.expect_err("strict mode must reject undeclared framework keys");
    assert_enumerates_all(
        &err,
        &[
            "executor.zz_undeclared",
            "acl.zz_also_undeclared",
            "obs.zz_third_undeclared",
        ],
    );
    assert!(
        !err.message.contains("missing required field"),
        "the document declares `version` and `project.name`; the failure must \
         be about the undeclared keys alone: {}",
        err.message
    );
}

/// §9.14 clause (b) "applies in legacy mode too, where the whole file *is* the
/// `apcore` namespace" — which means it applies in namespace mode as well, via
/// §9.10 step 2. Both branches, one expectation.
#[test]
fn strict_rejects_every_undeclared_framework_key_in_namespace_mode() {
    let (_dir, loaded) = load_doc(&namespace_doc(&offending_document()));
    let err = loaded.expect_err("strict mode must reject undeclared framework keys");
    assert_enumerates_all(
        &err,
        &[
            "executor.zz_undeclared",
            "acl.zz_also_undeclared",
            "obs.zz_third_undeclared",
        ],
    );
}

/// A document that declares every framework section, all of whose keys the
/// canonical schema declares, must pass strict mode.
///
/// This is the regression guard for the other half of the strict work: Rust
/// merges the `apcore:` block's members up to the top level of
/// `user_namespaces`, so framework sections sit beside genuine Config Bus
/// namespaces there. Before `is_framework_section` filtered them out, §9.10
/// step 3b reported `unknown namespace 'acl'` — and `unknown namespace
/// 'project'`, for the §9.1 required field — for any strict document that
/// declared them.
#[test]
fn strict_accepts_a_document_whose_framework_keys_are_all_declared() {
    let doc = namespace_doc(&[
        ("_config.strict", json!(true)),
        ("project.name", json!("strict-fixture")),
        ("project.version", json!("0.1.0")),
        ("extensions.root", json!("./ext")),
        ("extensions.auto_discover", json!(false)),
        ("extensions.lazy_load", json!(false)),
        ("extensions.follow_symlinks", json!(true)),
        ("extensions.max_depth", json!(4)),
        ("extensions.ignore_patterns", json!(["*.test.*"])),
        ("schema.root", json!("./sch")),
        ("schema.strategy", json!("json_first")),
        ("schema.max_ref_depth", json!(12)),
        ("acl.root", json!("./my-acl")),
        // Fixture value, not a recommendation: the canonical default is `deny`
        // and MUST stay `deny` in every example and deployment.
        ("acl.default_effect", json!("deny")),
        ("acl.audit.enabled", json!(true)),
        ("logging.level", json!("debug")),
        ("logging.format", json!("json")),
        ("observability.tracing.enabled", json!(true)),
        ("observability.metrics.enabled", json!(true)),
        ("middleware.disabled", json!(["some-middleware"])),
        ("executor.default_timeout", json!(1000)),
        ("executor.global_timeout", json!(2000)),
        ("executor.max_call_depth", json!(7)),
        ("executor.max_module_repeat", json!(2)),
        ("pipeline.remove", json!(["step-a"])),
        ("pipeline.configure", json!({})),
        ("pipeline.steps", json!([])),
        ("validation.binding.description_max_length", json!(120)),
        ("validation.pipeline.step_name_max_length", json!(64)),
        ("id_map.auto_detect", json!(false)),
        ("id_map.overrides", json!({})),
        ("bindings.dir", json!("./bindings")),
        ("bindings.pattern", json!("*.yaml")),
        ("sys_modules.enabled", json!(true)),
        ("sys_modules.health.enabled", json!(false)),
        ("sys_modules.manifest.enabled", json!(false)),
        ("sys_modules.usage.retention_hours", json!(24)),
        ("sys_modules.control.enabled", json!(false)),
        ("sys_modules.error_history.max_total_entries", json!(10)),
        ("sys_modules.events.enabled", json!(true)),
        ("stream.max_merge_depth", json!(7)),
        ("obs.redaction.replacement", json!("[hidden]")),
    ]);

    let (_dir, loaded) = load_doc(&doc);
    let config = match loaded {
        Ok(config) => config,
        Err(e) => panic!(
            "a strict document declaring only canonical framework keys was \
             rejected: {}",
            e.message
        ),
    };
    assert_eq!(config.get("acl.root"), Some(json!("./my-acl")));
    assert_eq!(config.get("project.name"), Some(json!("strict-fixture")));
    assert_eq!(config.get("sys_modules.health.enabled"), Some(json!(false)));
}

/// §9.10 step 3b still stands: strict mode rejects a top-level namespace
/// nobody registered. Filtering framework sections out of that check must not
/// have turned it off.
#[test]
fn strict_still_rejects_an_unregistered_namespace() {
    // Built by hand rather than through `namespace_doc`: the offending name
    // has to be a TOP-LEVEL namespace, not a member of the `apcore:` block.
    let doc = json!({
        "apcore": { "version": "1.0.0" },
        "_config": { "strict": true },
        "zz_unregistered_namespace_9_14": { "knob": 1 },
    });
    let (_dir, loaded) = load_doc(&doc);
    let err = loaded.expect_err("strict mode must reject an unregistered namespace");
    assert_eq!(err.code, ErrorCode::ConfigInvalid);
    assert!(
        err.message.contains("zz_unregistered_namespace_9_14"),
        "the error must name the offending namespace: {}",
        err.message
    );
}

/// `allow_unknown` is defined in §9.6.3 for unknown top-level NAMESPACES.
/// §9.14 states it does not apply to keys inside a framework section:
/// "stretching one field across two granularities would make its meaning depend
/// on where it is read."
#[test]
fn allow_unknown_does_not_soften_the_strict_framework_key_check() {
    let mut flat = offending_document();
    flat.push(("_config.allow_unknown", json!(true)));
    let (_dir, loaded) = load_doc(&legacy_doc(&flat));
    let err = loaded.expect_err("allow_unknown must not exempt framework keys");
    assert_enumerates_all(&err, &["executor.zz_undeclared", "acl.zz_also_undeclared"]);
}

// ---------------------------------------------------------------------------
// The section table is a projection of the canonical schema, not a second
// source of truth
// ---------------------------------------------------------------------------

/// Locate `apcore/schemas/` the same way the conformance drivers locate
/// `apcore/conformance/fixtures/`.
use crate::conformance_env::find_schemas_root;

fn read_schema(name: &str) -> Value {
    let path = find_schemas_root().join(name);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{} does not parse: {e}", path.display()))
}

/// Every key a `$defs` entry declares at its own level: its `properties`, plus
/// the `properties` of every `oneOf` / `anyOf` / `allOf` branch.
///
/// `ExtensionsConfig` is the reason the branches count: it spells its
/// closedness `unevaluatedProperties: false` over a `oneOf` whose branches
/// carry `root` / `namespace` / `roots`. A derivation that read only the
/// top-level `properties` would call those three undeclared and strict mode
/// would reject `extensions.root`.
fn declared_keys(def: &Value) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut collect = |node: &Value| {
        if let Some(props) = node.get("properties").and_then(Value::as_object) {
            out.extend(props.keys().cloned());
        }
    };
    collect(def);
    for combinator in ["oneOf", "anyOf", "allOf"] {
        if let Some(branches) = def.get(combinator).and_then(Value::as_array) {
            for branch in branches {
                collect(branch);
            }
        }
    }
    out
}

/// `FRAMEWORK_SECTION_KEYS` must equal what the canonical schemas declare —
/// section for section, key for key.
///
/// The SDK cannot read `apcore/schemas/` at runtime, so the projection is
/// transcribed into `src/config.rs`. Transcriptions drift: this re-derives the
/// projection from the schema files on every run so a section added upstream
/// fails here instead of being quietly exempt from strict mode forever.
#[test]
fn framework_section_keys_match_the_canonical_schema() {
    let schema = read_schema("apcore-config.schema.json");
    let defs = schema["$defs"].as_object().expect("$defs is an object");
    let properties = schema["properties"]
        .as_object()
        .expect("properties is an object");

    // A framework SECTION is a top-level property that refs a `$defs` object.
    // `$schema` and `version` are scalars — §9.14 iterates the keys *inside* a
    // section, so a scalar has nothing to iterate.
    let mut derived: Vec<(String, BTreeSet<String>)> = Vec::new();
    for (name, prop) in properties {
        let Some(reference) = prop.get("$ref").and_then(Value::as_str) else {
            continue;
        };
        let def_name = reference
            .strip_prefix("#/$defs/")
            .unwrap_or_else(|| panic!("`{name}` has a non-local $ref: {reference}"));
        let def = defs
            .get(def_name)
            .unwrap_or_else(|| panic!("`{name}` refs missing $defs/{def_name}"));
        derived.push((name.clone(), declared_keys(def)));
    }

    // `sys_modules` is declared across two canonical files: apcore-config
    // carries only `enabled`, and sys-modules.schema.json carries the rest.
    // This is the same projection `config_key_governance.json` performs when it
    // builds `allowed_keys` ("the last namespaced under `sys_modules.`").
    // Without the union, strict mode would reject `sys_modules.health.enabled`.
    let sys_modules_keys = declared_keys(&read_schema("sys-modules.schema.json"));
    assert!(
        sys_modules_keys.contains("health"),
        "sys-modules.schema.json no longer declares the families this union \
         depends on: {sys_modules_keys:?}"
    );
    for (name, keys) in &mut derived {
        if name == "sys_modules" {
            keys.extend(sys_modules_keys.iter().cloned());
        }
    }
    derived.sort_by(|a, b| a.0.cmp(&b.0));

    let mut table: Vec<(String, BTreeSet<String>)> = apcore::config::FRAMEWORK_SECTION_KEYS
        .iter()
        .map(|(section, keys)| {
            (
                (*section).to_string(),
                keys.iter().map(|k| (*k).to_string()).collect(),
            )
        })
        .collect();
    table.sort_by(|a, b| a.0.cmp(&b.0));

    let derived_sections: Vec<&String> = derived.iter().map(|(n, _)| n).collect();
    let table_sections: Vec<&String> = table.iter().map(|(n, _)| n).collect();
    assert_eq!(
        table_sections, derived_sections,
        "FRAMEWORK_SECTION_KEYS lists a different set of sections than \
         apcore-config.schema.json declares. A section added upstream and not \
         added here is silently exempt from strict mode; one removed upstream \
         is dead weight."
    );

    let mut drift: Vec<String> = Vec::new();
    for ((name, want), (_, got)) in derived.iter().zip(table.iter()) {
        if want != got {
            let missing: Vec<&String> = want.difference(got).collect();
            let extra: Vec<&String> = got.difference(want).collect();
            drift.push(format!("{name}: missing {missing:?}, extra {extra:?}"));
        }
    }
    assert!(
        drift.is_empty(),
        "FRAMEWORK_SECTION_KEYS disagrees with the canonical schema:\n  {}",
        drift.join("\n  ")
    );
}

/// Every typed field on `ConfigHelper` must be classified: either it is in
/// `TYPED_SECTIONS` (its raw object is retained so undeclared subkeys survive)
/// or it is one of the acknowledged scalars (nothing to lose).
///
/// This is the guard against the next apcore-rust#33. A field added outside the
/// `#[serde(flatten)]` bag silently discards everything its type does not
/// model; a new one fails here until its author decides which group it is in.
/// The list is recovered by parsing `src/config.rs` — Rust has no runtime
/// reflection over struct fields — pulled in with `include_str!`, so the test
/// does not depend on the working directory.
#[test]
fn every_typed_config_field_is_classified() {
    const CONFIG_RS: &str = include_str!("../src/config.rs");

    let start = CONFIG_RS
        .find("struct ConfigHelper")
        .expect("`struct ConfigHelper` no longer appears in src/config.rs");
    let open = CONFIG_RS[start..]
        .find('{')
        .expect("ConfigHelper has no body")
        + start;
    let close = CONFIG_RS[open..]
        .find('}')
        .expect("ConfigHelper body is unbalanced")
        + open;
    let fields: Vec<String> = CONFIG_RS[open + 1..close]
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') || line.starts_with("//") {
                return None;
            }
            let name = line.split(':').next()?.trim();
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                return None;
            }
            Some(name.to_string())
        })
        // The flatten bag itself is not a section.
        .filter(|name| name != "user_namespaces")
        .collect();

    assert!(
        fields.len() >= 3,
        "expected at least modules_path/executor/observability, got {fields:?} \
         — the ConfigHelper parse broke and this guard is asserting nothing"
    );

    // Scalars have no subkeys, so there is nothing serde can drop from them.
    // A field added here is a deliberate statement that it carries no nested
    // structure.
    const SCALAR_FIELDS: &[&str] = &["modules_path"];

    // `TYPED_SECTIONS` is private; read it out of the source the same way.
    let typed_sections_line = CONFIG_RS
        .find("const TYPED_SECTIONS:")
        .expect("`TYPED_SECTIONS` no longer appears in src/config.rs");
    let end = CONFIG_RS[typed_sections_line..]
        .find("];")
        .expect("TYPED_SECTIONS is not a slice literal")
        + typed_sections_line;
    // Lowercased so the entries — spelled as the `EXECUTOR_NS` /
    // `OBSERVABILITY_NS` consts rather than as string literals — match the
    // `ConfigHelper` field names.
    let typed_sections_src = CONFIG_RS[typed_sections_line..end].to_ascii_lowercase();
    assert!(
        typed_sections_src.contains("executor") && typed_sections_src.contains("observability"),
        "TYPED_SECTIONS must retain the raw object for both sections that broke \
         (apcore-rust#33, #34): {typed_sections_src}"
    );

    let unclassified: Vec<&String> = fields
        .iter()
        .filter(|name| {
            !SCALAR_FIELDS.contains(&name.as_str()) && !typed_sections_src.contains(name.as_str())
        })
        .collect();
    assert!(
        unclassified.is_empty(),
        "{unclassified:?} are typed fields on ConfigHelper that are neither in \
         TYPED_SECTIONS nor acknowledged scalars. A typed field sits OUTSIDE \
         the `#[serde(flatten)]` bag, so serde discards every subkey its type \
         does not model — the apcore-rust#33 defect, which PROTOCOL_SPEC §9.14 \
         now forbids. Add the section to TYPED_SECTIONS (raw object retained, \
         reconciled by `typed_namespace_view`) or to SCALAR_FIELDS here."
    );
}
