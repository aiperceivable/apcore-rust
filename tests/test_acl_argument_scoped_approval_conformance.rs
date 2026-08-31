//! Conformance driver for `acl_argument_scoped_approval.json`
//! (PROTOCOL_SPEC §6.1.6 / §6.1.7 / §6.1.8 / §6.8.1, spec v1.28.0, apcore#108).
//!
//! An ACL rule answers two independent questions — may this caller reach this
//! target at all, and must *this particular call* be put to a human first. The
//! orthogonal `approval` field carries the second, and the built-in
//! structure-only `arguments` condition decides whether a rule matches this
//! call.
//!
//! The two cases worth reading before the rest are
//! `no_projection_must_not_grant_via_an_empty_stand_in` and
//! `no_projection_makes_a_deny_rule_take_effect`: they bracket the same
//! fail-open bug from both directions. Substituting an empty key set for an
//! absent projection makes `has_none_of` vacuously satisfied, so an `allow`
//! rule grants for a call whose arguments were never seen — and leaves
//! `has_key` unsatisfied, so a `deny` rule fails to take effect. Only the
//! UNEVALUABLE reading of §6.1.8 rule 1 refuses in both directions.
//!
//! Driver contract (from the fixture `description`): build an ACL from `rules`
//! in order with the given `default_effect` and an audit sink; supply a
//! governance projection derived from `arguments` — §6.1.8 rule 4 leaves the
//! route idiomatic, and this SDK passes it as a parameter to
//! [`ACL::check_access`] rather than carrying it on the context. `arguments:
//! null` means NO PROJECTION AT ALL, which is a different case from an empty
//! one. Assert `access`, the approval requirement and `matched_rule_index`;
//! assert the legacy boolean separately, because §6.8.1 makes it fail closed
//! on an approval requirement; assert `AuditEntry.handler_error` is non-null
//! exactly where the fixture says; and where `expected_validation_finding_path`
//! is non-null, assert `validate_rules()` reports a finding at that path.

use apcore::acl::{
    ACLRule, AccessDecision, ApprovalRequirement, AuditEntry, GovernanceProjection, ACL,
};
use apcore::context::{Context, Identity};
use serde_json::Value;
use std::sync::{Arc, Mutex};

use crate::conformance_env::find_fixtures_root;

const FIXTURE: &str = "acl_argument_scoped_approval.json";

/// The fixture lands in the spec repo one push after this driver, so that
/// `check_driver_coverage.py --strict` has a driver to find for it. Until then
/// the driver reports "not verified" rather than a pass it did not earn.
fn load_fixture() -> Option<Value> {
    let path = find_fixtures_root().join(FIXTURE);
    if !path.is_file() {
        return None;
    }
    Some(
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read fixture"))
            .expect("parse fixture"),
    )
}

fn build(case: &Value) -> (ACL, Arc<Mutex<Vec<AuditEntry>>>) {
    let entries: Arc<Mutex<Vec<AuditEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&entries);
    let rules: Vec<ACLRule> = case["rules"]
        .as_array()
        .expect("rules array")
        .iter()
        .map(|r| ACLRule {
            callers: r["callers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c.as_str().unwrap().to_string())
                .collect(),
            targets: r["targets"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t.as_str().unwrap().to_string())
                .collect(),
            effect: r["effect"].as_str().unwrap().to_string(),
            approval: r.get("approval").and_then(Value::as_str).map(|a| match a {
                "required" => ApprovalRequirement::Required,
                _ => ApprovalRequirement::NotRequired,
            }),
            description: None,
            conditions: r.get("conditions").cloned(),
        })
        .collect();
    let acl = ACL::new(
        rules,
        case["default_effect"].as_str().unwrap(),
        Some(Arc::new(move |entry: &AuditEntry| {
            sink.lock().unwrap().push(entry.clone())
        })),
    );
    (acl, entries)
}

fn context() -> Context<Value> {
    Context::create(
        Some(Identity::new(
            "u".to_string(),
            "user".to_string(),
            vec!["dev".to_string()],
            std::collections::HashMap::new(),
        )),
        None,
        None,
        None,
        Value::Null,
        None,
    )
}

/// `arguments: null` means no projection at all (§6.1.8 rule 1) — NOT a
/// projection of `{}`, which is a materially different input.
fn projection(case: &Value) -> Option<GovernanceProjection> {
    match &case["arguments"] {
        Value::Null => None,
        args => Some(GovernanceProjection::from_arguments(args)),
    }
}

#[test]
fn acl_argument_scoped_approval_conformance() {
    let Some(fixture) = load_fixture() else {
        eprintln!(
            "SKIP: {FIXTURE} not in the spec repo yet (spec v1.28.0, apcore#108) — NOT VERIFIED"
        );
        return;
    };
    let cases = fixture["test_cases"].as_array().expect("test_cases array");
    assert!(!cases.is_empty(), "fixture carries no cases");
    let mut skipped: Vec<String> = Vec::new();

    for case in cases {
        let id = case["id"].as_str().unwrap();
        let note = case["note"].as_str().unwrap();
        let (acl, entries) = build(case);
        let ctx = context();
        let caller = case["caller_id"].as_str().unwrap();
        let target = case["target_id"].as_str().unwrap();
        let proj = projection(case);

        let decision: AccessDecision =
            acl.check_access(Some(caller), target, Some(&ctx), proj.as_ref());
        assert_eq!(
            decision.access,
            case["expected_access"].as_str().unwrap(),
            "[{id}] {note}"
        );
        assert_eq!(
            decision.approval_required,
            case["expected_approval_required"].as_bool().unwrap(),
            "[{id}] {note}"
        );
        assert_eq!(
            decision.matched_rule_index,
            case["expected_matched_rule_index"]
                .as_u64()
                .map(|i| i as usize),
            "[{id}] {note}"
        );

        // §6.3.1: handler_error is non-null IF AND ONLY IF a condition was
        // unevaluable. Read before the legacy call below, which emits its own.
        let logged = entries.lock().unwrap().clone();
        assert_eq!(
            logged.len(),
            1,
            "[{id}] check_access must emit exactly one audit entry"
        );
        let entry = &logged[0];
        assert_eq!(
            entry.handler_error.is_some(),
            case["expected_audit_handler_error_present"]
                .as_bool()
                .unwrap(),
            "[{id}] {note}\n  handler_error was {:?}",
            entry.handler_error
        );
        assert_eq!(
            entry.approval_required,
            case["expected_approval_required"].as_bool().unwrap(),
            "[{id}] {note}"
        );

        // §6.8.1: the legacy boolean fails closed on an approval requirement,
        // so allow + required reads as false.
        //
        // `ACL::check` takes no projection in this SDK — its signature is
        // `(caller_id, target_id, ctx)` — so a rule carrying an `arguments`
        // condition is unevaluable through it for want of one (§6.1.8 rule 1),
        // whatever `expected_legacy_check` says about the same call made WITH
        // a projection. Those cases are skipped rather than asserted against
        // the answer to a different question, and the skip is reconciled
        // against the fixture after the loop so a case that stops carrying an
        // `arguments` condition starts being asserted instead of silently
        // staying skipped.
        if takes_arguments(case) {
            skipped.push(id.to_string());
        } else {
            assert_eq!(
                acl.check(Some(caller), target, Some(&ctx)),
                case["expected_legacy_check"].as_bool().unwrap(),
                "[{id}] {note}"
            );
        }

        // §6.1.8 closing paragraph: the well-formedness cases are decidable
        // with no context and no handler, so validate_rules() must surface
        // them at deploy time rather than at the first call that trips them.
        let findings = acl.validate_rules();
        // §6.1.8 rule 3: every faulty predicate is reported, so a case may pin
        // the exact finding set rather than the presence of one.
        if let Some(paths) = case["expected_validation_finding_paths"].as_array() {
            let want: Vec<&str> = paths.iter().map(|p| p.as_str().unwrap()).collect();
            let got: Vec<&str> = findings.iter().map(|f| f.condition_path.as_str()).collect();
            assert_eq!(got, want, "[{id}] {note}");
            for finding in &findings {
                assert!(!finding.sync_resolvable, "[{id}] {note}");
                assert!(!finding.async_resolvable, "[{id}] {note}");
            }
            continue;
        }
        match case["expected_validation_finding_path"].as_str() {
            Some(path) => {
                let at: Vec<_> = findings
                    .iter()
                    .filter(|f| f.condition_path == path)
                    .collect();
                assert!(
                    !at.is_empty(),
                    "[{id}] {note}\n  no finding at '{path}': {findings:?}"
                );
                assert!(!at[0].sync_resolvable, "[{id}] {note}");
                assert!(!at[0].async_resolvable, "[{id}] {note}");
            }
            None => assert!(
                findings.is_empty(),
                "[{id}] {note}\n  unexpected findings: {findings:?}"
            ),
        }
    }

    // Reconciled against the fixture rather than trusted.
    let expected_skips: Vec<String> = cases
        .iter()
        .filter(|c| takes_arguments(c))
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        skipped, expected_skips,
        "the legacy-boolean skip list drifted from the fixture"
    );
    assert!(
        skipped.len() < cases.len(),
        "every case skipped the legacy assertion — §6.8.1 would go unexercised"
    );
    eprintln!(
        "{}/{} case(s) skip the legacy `check()` assertion: ACL::check takes no projection in this SDK",
        skipped.len(),
        cases.len()
    );
}

/// Whether any of a case's rules carries an `arguments` condition, and so is
/// unevaluable through the projection-less legacy [`ACL::check`].
fn takes_arguments(case: &Value) -> bool {
    case["rules"].as_array().is_some_and(|rules| {
        rules.iter().any(|r| {
            r.get("conditions")
                .and_then(Value::as_object)
                .is_some_and(|c| c.contains_key("arguments"))
        })
    })
}

/// §6.1.6 rule 3 — the meaningless combination cannot get in by any door.
#[test]
fn deny_plus_approval_is_rejected_at_every_entry_point() {
    let rule = ACLRule {
        callers: vec!["*".to_string()],
        targets: vec!["x.y".to_string()],
        effect: "deny".to_string(),
        approval: Some(ApprovalRequirement::Required),
        description: None,
        conditions: None,
    };
    assert!(
        ACL::try_new(vec![rule.clone()], "deny", None).is_err(),
        "try_new must reject"
    );
    let mut acl = ACL::new(vec![], "deny", None);
    assert!(acl.try_add_rule(rule).is_err(), "try_add_rule must reject");
}
