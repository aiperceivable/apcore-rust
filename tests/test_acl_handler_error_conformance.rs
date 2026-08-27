//! Conformance driver for `acl_handler_error.json` (PROTOCOL_SPEC §6.1.1 /
//! §6.3.1, sync findings A-D-011 / A-D-012, SECURITY).
//!
//! A condition that CANNOT BE EVALUATED is not a condition that is FALSE, and
//! the rule's `effect` decides what the difference means. An unevaluable
//! condition — no registered handler, a handler that panics, or an async
//! handler unresolvable on the sync path — MUST resolve the rule toward
//! refusing access: a `deny` rule takes effect and the call is DENIED; an
//! `allow` rule does not match and MUST NOT grant. The emitted `AuditEntry`
//! MUST carry a non-null `handler_error` in both directions.
//!
//! Driver contract (from the fixture `description`): register a test condition
//! handler under `throwing_condition_key` whose evaluate panics, register
//! nothing for `unknown_condition_key`, build an ACL from `rules` +
//! `default_effect` with an audit sink, call `check(caller_id, target_id)`,
//! then assert the decision equals `expected` and that the captured
//! `AuditEntry.handler_error` is non-null when
//! `expected_audit_handler_error_present` is true.
//!
//! # Staged-fixture rollout (apcore#100)
//!
//! spec v1.22.0 CHANGED one of these decisions, and the corrected fixture is
//! deliberately staged at
//! `apcore/planning/acl-unevaluable-conditions/staged-fixtures/` rather than
//! in `conformance/fixtures/`, so CI does not go red across every SDK
//! repository while the three drivers land one at a time. Until it moves, the
//! fixture on disk still pins the pre-v1.22.0 expectation for exactly one
//! case: `throwing_handler_does_not_flip_default_allow_to_deny_unsafely`, a
//! `deny` rule whose handler panics under `default_effect: allow`, which used
//! to expect `true` — i.e. a handler crash silently disabling the block, which
//! is the bug #100 was opened for.
//!
//! [`superseded_expectation`] overrides that one case by ID, and the override
//! is keyed on the fixture NOT already carrying the corrected shape (detected
//! by the `unknown_condition_key` field the new fixture adds). When the
//! corrected fixture lands, the detection turns the override off on its own
//! and [`SUPERSEDED_BY_V1_22_0`] can be deleted.
//!
//! The unknown-key half of the corrected fixture has no counterpart in the old
//! one, so it is covered natively by
//! [`unknown_condition_key_resolves_toward_refusing_access`] below.
#![allow(clippy::pedantic)] // fixture-driven test file: casts/layout follow the fixture schema

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use apcore::acl::{ACLRule, AuditEntry, ACL};
use apcore::acl_handlers::ACLConditionHandler;
use apcore::context::{Context, Identity};
use async_trait::async_trait;
use serde_json::{Map, Value};

// ---------------------------------------------------------------------------
// Fixture loading
// ---------------------------------------------------------------------------

use crate::conformance_env::find_fixtures_root;

fn load_fixture() -> Value {
    let path = find_fixtures_root().join("acl_handler_error.json");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("Failed to read fixture: {}", path.display()));
    serde_json::from_str(&content).unwrap_or_else(|e| panic!("Invalid JSON: {e}"))
}

// ---------------------------------------------------------------------------
// Test condition handler that panics during evaluation.
// ---------------------------------------------------------------------------

struct ThrowingHandler;

#[async_trait]
impl ACLConditionHandler for ThrowingHandler {
    async fn evaluate(&self, _value: &Value, _ctx: &Context<Value>) -> bool {
        panic!("conformance: __test_throwing_condition__ simulated handler panic");
    }
}

fn make_context() -> Context<Value> {
    let identity = Identity::new(
        "test-user".to_string(),
        "user".to_string(),
        vec![],
        HashMap::new(),
    );
    Context::new(identity)
}

/// Build an `ACLRule` from a fixture rule object.
fn build_rule(rule: &Value) -> ACLRule {
    let callers = rule["callers"]
        .as_array()
        .expect("rule.callers")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let targets = rule["targets"]
        .as_array()
        .expect("rule.targets")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let effect = rule["effect"].as_str().expect("rule.effect").to_string();
    let conditions = rule.get("conditions").and_then(|c| {
        c.as_object().map(|obj| {
            let mut map = Map::new();
            for (k, v) in obj {
                map.insert(k.clone(), v.clone());
            }
            Value::Object(map)
        })
    });
    ACLRule {
        callers,
        targets,
        effect,
        description: None,
        conditions,
    }
}

/// Suppress the default panic-hook stderr noise so caught panics in these
/// tests don't spam the log.
fn silence_panic_hook<T>(f: impl FnOnce() -> T) -> T {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let out = f();
    std::panic::set_hook(prev);
    out
}

/// Cases whose expectation spec v1.22.0 (#100) changed, mapped to the value
/// the corrected fixture asserts. Consulted only while the fixture on disk is
/// still the pre-v1.22.0 generation — see the module docs.
const SUPERSEDED_BY_V1_22_0: &[(&str, bool)] = &[(
    // A `deny` rule + a panicking handler + `default_effect: allow`. Was
    // `true`: the deny rule did not match and the call fell through to the
    // permissive default, so a handler crash silently disabled the block.
    // §6.1.1 now makes the deny rule take effect.
    "throwing_handler_does_not_flip_default_allow_to_deny_unsafely",
    false,
)];

/// The expectation to assert for `case_id`, honouring the staged-fixture
/// override while the corrected fixture has not yet landed.
fn superseded_expectation(fixture_is_corrected: bool, case_id: &str, from_fixture: bool) -> bool {
    if fixture_is_corrected {
        return from_fixture;
    }
    SUPERSEDED_BY_V1_22_0
        .iter()
        .find(|(id, _)| *id == case_id)
        .map_or(from_fixture, |(_, corrected)| *corrected)
}

#[test]
fn acl_handler_error_conformance() {
    let fixture = load_fixture();
    let throwing_key = fixture["throwing_condition_key"]
        .as_str()
        .expect("fixture must declare throwing_condition_key");
    // The corrected (v1.22.0) fixture adds `unknown_condition_key`; the old
    // one has no such field. That is the generation switch.
    let fixture_is_corrected = fixture.get("unknown_condition_key").is_some();

    // Register the panicking handler under the fixture-declared key (global,
    // process-wide registry). The key is fixture-specific and unique, so
    // cross-test interference is not a concern.
    ACL::init_builtin_handlers();
    ACL::register_condition(throwing_key, Arc::new(ThrowingHandler));

    let cases = fixture["test_cases"]
        .as_array()
        .expect("test_cases must be an array");

    // Keep the override table honest: a superseded ID that no longer appears
    // in the fixture is dead weight that would hide a real regression.
    if !fixture_is_corrected {
        let ids: Vec<&str> = cases.iter().filter_map(|tc| tc["id"].as_str()).collect();
        for (superseded_id, _) in SUPERSEDED_BY_V1_22_0 {
            assert!(
                ids.contains(superseded_id),
                "SUPERSEDED_BY_V1_22_0 names case '{superseded_id}', which the fixture no \
                 longer carries. Either the corrected fixture landed (drop the entry, and the \
                 whole override with it) or the case was renamed."
            );
        }
    }

    for tc in cases {
        let id = tc["id"].as_str().expect("each case needs an id");

        let rules: Vec<ACLRule> = tc["rules"]
            .as_array()
            .expect("case.rules")
            .iter()
            .map(build_rule)
            .collect();
        let default_effect = tc["default_effect"].as_str().expect("case.default_effect");
        let caller_id = tc["caller_id"].as_str();
        let target_id = tc["target_id"].as_str().expect("case.target_id");
        let expected = superseded_expectation(
            fixture_is_corrected,
            id,
            tc["expected"].as_bool().expect("case.expected"),
        );
        let expect_handler_error = tc["expected_audit_handler_error_present"]
            .as_bool()
            .unwrap_or(false);

        let captured: Arc<Mutex<Vec<AuditEntry>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_for_logger = Arc::clone(&captured);
        let mut acl = ACL::new(rules, default_effect, None);
        acl.set_audit_logger(move |entry: &AuditEntry| {
            captured_for_logger.lock().unwrap().push(entry.clone());
        });

        let ctx = make_context();
        let decision = silence_panic_hook(|| acl.check(caller_id, target_id, Some(&ctx)));

        assert_eq!(
            decision, expected,
            "case {id}: an unevaluable condition must resolve the rule toward \
             refusing access (PROTOCOL_SPEC §6.1.1) — expected decision \
             {expected}, got {decision}"
        );

        let entries = captured.lock().unwrap();
        let entry = entries
            .last()
            .unwrap_or_else(|| panic!("case {id}: an audit entry must be emitted"));
        if expect_handler_error {
            assert!(
                entry.handler_error.is_some(),
                "case {id}: AuditEntry.handler_error must be non-null when a \
                 condition handler panics, got {:?}",
                entry.handler_error
            );
        }
    }
}

/// The unknown-key half of the corrected fixture, covered natively until that
/// fixture lands (see the module docs).
///
/// A condition key nobody registered is UNEVALUABLE, not false — this is the
/// misspelled-key case #100 was opened for (`role:` written for `roles:`). On
/// an `allow` rule the outcome is unchanged (no grant); on a `deny` rule the
/// rule now takes effect instead of falling through to a permissive
/// `default_effect`.
#[test]
fn unknown_condition_key_resolves_toward_refusing_access() {
    ACL::init_builtin_handlers();
    // Deliberately NOT registered anywhere.
    let unknown_key = "__test_unregistered_condition_rs__";

    // (effect, default_effect, expected decision)
    let cases = [
        ("allow", "deny", false),
        // The regression guard: pre-v1.22.0 this returned `true`.
        ("deny", "allow", false),
    ];

    for (effect, default_effect, expected) in cases {
        let mut conditions = Map::new();
        conditions.insert(unknown_key.to_string(), Value::Bool(true));
        let rule = ACLRule {
            callers: vec!["*".to_string()],
            targets: vec!["*".to_string()],
            effect: effect.to_string(),
            description: None,
            conditions: Some(Value::Object(conditions)),
        };

        let captured: Arc<Mutex<Vec<AuditEntry>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_for_logger = Arc::clone(&captured);
        let mut acl = ACL::new(vec![rule], default_effect, None);
        acl.set_audit_logger(move |entry: &AuditEntry| {
            captured_for_logger.lock().unwrap().push(entry.clone());
        });

        let ctx = make_context();
        let decision = acl.check(Some("user"), "service.operation", Some(&ctx));
        assert_eq!(
            decision, expected,
            "effect={effect} default_effect={default_effect}: an unregistered condition key is \
             unevaluable (PROTOCOL_SPEC §6.1.1), so a deny rule denies and an allow rule does \
             not grant"
        );

        let entries = captured.lock().unwrap();
        let entry = entries.last().expect("an audit entry must be emitted");
        let handler_error = entry
            .handler_error
            .as_deref()
            .expect("AuditEntry.handler_error must be non-null for an unevaluable condition");
        assert!(
            handler_error.contains(unknown_key),
            "handler_error must name the offending condition key: {handler_error}"
        );
    }
}
