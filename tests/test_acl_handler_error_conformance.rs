//! Conformance driver for `acl_handler_error.json` (sync findings A-D-011 /
//! A-D-012, SECURITY).
//!
//! A custom ACL condition handler that panics during evaluation MUST fail
//! CLOSED (the rule does not match → the decision never silently flips to a
//! less-safe outcome) AND the emitted `AuditEntry` MUST carry a non-null
//! `handler_error`. This exercises the `catch_unwind` fail-closed boundary in
//! `ACL::check`.
//!
//! Driver contract (from the fixture `description`): register a test condition
//! handler under `throwing_condition_key` whose evaluate panics, build an ACL
//! from `rules` + `default_effect` with an audit sink, call
//! `check(caller_id, target_id)`, then assert the decision equals `expected`
//! and that the captured `AuditEntry.handler_error` is non-null when
//! `expected_audit_handler_error_present` is true.
#![allow(clippy::pedantic)] // fixture-driven test file: casts/layout follow the fixture schema

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use apcore::acl::{ACLRule, AuditEntry, ACL};
use apcore::acl_handlers::ACLConditionHandler;
use apcore::context::{Context, Identity};
use async_trait::async_trait;
use serde_json::{Map, Value};

// ---------------------------------------------------------------------------
// Fixture loading
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
         Set APCORE_SPEC_REPO or clone apcore as a sibling of {}",
        manifest_dir.parent().unwrap().display()
    );
}

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

#[test]
fn acl_handler_error_conformance() {
    let fixture = load_fixture();
    let throwing_key = fixture["throwing_condition_key"]
        .as_str()
        .expect("fixture must declare throwing_condition_key");

    // Register the panicking handler under the fixture-declared key (global,
    // process-wide registry). The key is fixture-specific and unique, so
    // cross-test interference is not a concern.
    ACL::init_builtin_handlers();
    ACL::register_condition(throwing_key, Arc::new(ThrowingHandler));

    let cases = fixture["test_cases"]
        .as_array()
        .expect("test_cases must be an array");

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
        let expected = tc["expected"].as_bool().expect("case.expected");
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
            "case {id}: a panicking condition handler must fail closed — \
             expected decision {expected}, got {decision}"
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
