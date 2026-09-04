//! PROTOCOL_SPEC §6.2.1 — a `callers` / `targets` pattern array is FLAT and its
//! shape is CLOSED (spec v1.31.0, apcore#112).
//!
//! # What the rule is
//!
//! The operators do not nest, there is no precedence, an operand is always a
//! plain pattern string, and there is exactly one operator position — index 0.
//! **Tier 1** is structural and is rejected with `ACLRuleError` at every entry
//! point that accepts a rule: the array is non-empty, every element is a
//! non-empty string, `$or` at index 0 takes at least one operand, `$not` at
//! index 0 takes exactly one, and a reserved token appears nowhere but index 0.
//! **Tier 2** is semantic — an array that is well-formed under every tier-1
//! clause and still matches no legal module ID — and is a `validate_rules()`
//! finding only: it MUST NOT be rejected and MUST NOT change any decision.
//!
//! # Why rejection rather than a non-match
//!
//! Through spec v1.30.0 all three SDKs returned `false` from the matcher for
//! `[]`, `["$or"]` and `["$not"]`, so the rule was inert: with one rule in the
//! ACL the decision tracked `default_effect` exactly and `validate_rules()`
//! reported nothing. On an `allow` rule that is merely useless. On a `deny`
//! rule under `default_effect: allow` it is a **fail-open** — the call the
//! operator wrote the rule to block is permitted, by a rule that loaded without
//! error. `schemas/acl-config.schema.json` has declared `minItems: 1` and
//! `minLength: 1` on both fields since the file existed and no door enforced
//! either, which is the shape #107 and #111 already had.
//!
//! # Scope of this file
//!
//! These are **SDK-local** tests mirroring the staged conformance fixture
//! `acl_pattern_arity.json`, which lands in `conformance/fixtures/` only once
//! all three SDKs have. There is deliberately no driver for it yet, so the
//! cases are transcribed here by their fixture IDs.
//!
//! The fixture's nine `kind: "backstop"` cases that mutate a field on an
//! already-constructed rule are **not** here: [`ACL::rules`] hands back
//! `&[ACLRule]` and this SDK exposes no `rules_mut`, no public field and no
//! `Deserialize` on `ACL`, so that route cannot be reached from outside the
//! crate at all. They live in `src/acl.rs`'s `pattern_arity_backstop_tests`,
//! which can reach it — see the module note there for why the backstop is
//! implemented regardless.

#![allow(clippy::too_many_lines)]

use std::sync::{Arc, Mutex};

use apcore::acl::{ACLRule, AuditEntry, ACL};

/// Every door this SDK exposes that accepts a rule (§6.1.6 rule 3).
///
/// The fallible and infallible halves of each pair are both exercised. A test
/// asserting only on the `Result` forms would prove nothing about `ACL::new`
/// and `ACL::add_rule`, which are the two a caller reaches for first — and a
/// door that is exempt because its signature is infallible is exactly the hole
/// #111 was opened about.
const DOOR_COUNT: usize = 5;

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

fn rule_of(callers: &[&str], targets: &[&str], effect: &str) -> ACLRule {
    ACLRule::new(strings(callers), strings(targets), effect)
}

/// Emit the case as a one-rule ACL document.
///
/// Pattern arrays are written as JSON, which YAML is a superset of, so `""`
/// stays the empty string rather than becoming YAML's null — the distinction
/// the `empty_pattern_string_*` cases exist to test.
fn yaml_of(callers: &[&str], targets: &[&str], effect: &str, default_effect: &str) -> String {
    let json = |items: &[&str]| serde_json::to_string(items).expect("serialize patterns");
    format!(
        "default_effect: {default_effect}\nrules:\n  - callers: {}\n    targets: {}\n    effect: \"{effect}\"\n",
        json(callers),
        json(targets),
    )
}

/// One door's verdict: `Ok(())` accepted, `Err(message)` refused.
///
/// A panic is folded in as a refusal — for `ACL::new` and `ACL::add_rule` that
/// IS the refusal, which is what §6.1.6 rule 3 means by failing loudly in
/// whatever way the language signals an unconstructable value.
type Verdict = Result<(), String>;

fn verdict_of_panicking(f: impl FnOnce()) -> Verdict {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(()) => Ok(()),
        Err(payload) => Err(payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
            .unwrap_or_else(|| "<non-string panic payload>".to_string())),
    }
}

/// Run every entry point against one rule and report what each did.
fn run_all_doors(
    id: &str,
    callers: &[&str],
    targets: &[&str],
    effect: &str,
    default_effect: &str,
) -> Vec<(&'static str, Verdict)> {
    let dir = std::env::temp_dir().join("apcore_acl_pattern_arity");
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let file = dir.join(format!("{id}.yaml"));
    std::fs::write(&file, yaml_of(callers, targets, effect, default_effect)).expect("write yaml");

    vec![
        (
            "ACL::load",
            ACL::load(file.to_str().expect("utf8"))
                .map(|_| ())
                .map_err(|e| e.message),
        ),
        (
            "ACL::try_new",
            ACL::try_new(
                vec![rule_of(callers, targets, effect)],
                default_effect,
                None,
            )
            .map(|_| ())
            .map_err(|e| e.message),
        ),
        (
            "ACL::new",
            verdict_of_panicking(|| {
                let _ = ACL::new(
                    vec![rule_of(callers, targets, effect)],
                    default_effect,
                    None,
                );
            }),
        ),
        (
            "ACL::try_add_rule",
            ACL::try_new(vec![], default_effect, None)
                .expect("host ACL is valid")
                .try_add_rule(rule_of(callers, targets, effect))
                .map_err(|e| e.message),
        ),
        (
            "ACL::add_rule",
            verdict_of_panicking(|| {
                let mut acl =
                    ACL::try_new(vec![], default_effect, None).expect("host ACL is valid");
                acl.add_rule(rule_of(callers, targets, effect));
            }),
        ),
    ]
}

/// Assert that every door refuses the rule, and that each refusal names the
/// offending field so an operator can find it in their file (§6.2.1).
///
/// `field` is `callers` or `targets`; a message naming `targets[1]` satisfies
/// it, because the index narrows rather than replaces the field name.
fn assert_rejected_at_every_door(
    id: &str,
    callers: &[&str],
    targets: &[&str],
    effect: &str,
    default_effect: &str,
    field: &str,
) {
    let verdicts = run_all_doors(id, callers, targets, effect, default_effect);
    assert_eq!(verdicts.len(), DOOR_COUNT);
    for (api, verdict) in verdicts {
        let message = verdict.expect_err(&format!(
            "[{id}] {api} ACCEPTED a rule whose pattern shape is outside §6.2.1's table. \
             The shape is closed at EVERY entry point — a shape legal through one door and \
             illegal through another IS the defect (§6.1.6 rule 3)"
        ));
        assert!(
            message.contains(field),
            "[{id}] {api} refused without naming '{field}': {message}"
        );
    }
}

/// Assert that every door accepts the rule.
///
/// The controls matter as much as the rejections: a precheck that over-rejects
/// refuses configurations the specification permits, and `$orders.*` is the one
/// that separates reserved-token detection by **equality** from detection by a
/// `$` prefix.
fn assert_accepted_at_every_door(
    id: &str,
    callers: &[&str],
    targets: &[&str],
    effect: &str,
    default_effect: &str,
) {
    let verdicts = run_all_doors(id, callers, targets, effect, default_effect);
    assert_eq!(verdicts.len(), DOOR_COUNT);
    for (api, verdict) in verdicts {
        assert!(
            verdict.is_ok(),
            "[{id}] {api} REFUSED a rule §6.2.1 permits: {verdict:?}"
        );
    }
}

/// The `validate_rules()` finding paths for a one-rule ACL.
fn finding_paths(
    callers: &[&str],
    targets: &[&str],
    effect: &str,
    default_effect: &str,
) -> Vec<String> {
    let acl = ACL::try_new(
        vec![rule_of(callers, targets, effect)],
        default_effect,
        None,
    )
    .expect("a tier-2 array is well-formed and MUST load");
    acl.validate_rules()
        .into_iter()
        .map(|f| f.condition_path)
        .collect()
}

// ---------------------------------------------------------------------------
// Controls — the forms §6.2.1 permits must still load, at every door
// ---------------------------------------------------------------------------

#[test]
fn flat_single_pattern_loads() {
    assert_accepted_at_every_door(
        "flat_single_pattern_loads",
        &["api.*"],
        &["executor.*"],
        "allow",
        "deny",
    );
}

#[test]
fn flat_multi_pattern_loads() {
    assert_accepted_at_every_door(
        "flat_multi_pattern_loads",
        &["api.*", "worker.*"],
        &["executor.*"],
        "allow",
        "deny",
    );
}

#[test]
fn or_with_two_operands_loads() {
    assert_accepted_at_every_door(
        "or_with_two_operands_loads",
        &["$or", "admin", "moderator"],
        &["*"],
        "allow",
        "deny",
    );
}

#[test]
fn or_with_one_operand_loads() {
    // The boundary: `$or` requires AT LEAST one operand, not at least two. An
    // implementation that rejects this has read the rule as `minItems: 2` on
    // the array rather than as an arity rule on the operator.
    assert_accepted_at_every_door(
        "or_with_one_operand_loads",
        &["$or", "admin"],
        &["*"],
        "allow",
        "deny",
    );
}

#[test]
fn not_with_one_operand_loads() {
    assert_accepted_at_every_door(
        "not_with_one_operand_loads",
        &["$not", "banned.*"],
        &["*"],
        "allow",
        "deny",
    );
}

#[test]
fn token_lookalike_pattern_loads() {
    // Reserved-token detection is EQUALITY, never a `$` prefix or a substring.
    // `$orders.*` is an ordinary pattern that merely begins with the same
    // character. An implementation testing `starts_with('$')` passes every
    // rejection case in this file and fails here.
    assert_accepted_at_every_door(
        "token_lookalike_pattern_loads",
        &["api.*", "$orders.*"],
        &["*"],
        "allow",
        "deny",
    );
}

// ---------------------------------------------------------------------------
// Tier 1 — rejected at every door
// ---------------------------------------------------------------------------

#[test]
fn empty_callers_is_rejected() {
    // `ACL::load` rejects an OMITTED `callers` / `targets` and used to permit
    // an empty one, so a plain YAML file reaches this — not only direct
    // construction.
    assert_rejected_at_every_door(
        "empty_callers_is_rejected",
        &[],
        &["*"],
        "allow",
        "deny",
        "callers",
    );
}

#[test]
fn empty_targets_is_rejected() {
    assert_rejected_at_every_door(
        "empty_targets_is_rejected",
        &["*"],
        &[],
        "allow",
        "deny",
        "targets",
    );
}

#[test]
fn both_fields_empty_are_rejected() {
    // One rejection is enough — the error names at least one field — but the
    // rule MUST NOT be accepted.
    assert_rejected_at_every_door(
        "both_fields_empty_are_rejected",
        &[],
        &[],
        "allow",
        "deny",
        "callers",
    );
}

#[test]
fn empty_targets_on_deny_rule_under_default_allow_is_rejected() {
    // THE DRIVING CASE of #112. Written as YAML this loaded clean,
    // `validate_rules()` returned zero findings, and `check(_, "cli.rm", _)`
    // returned ALLOW: the operator has a rule that says "block everything
    // dangerous" and a deployment that blocks nothing.
    assert_rejected_at_every_door(
        "empty_targets_on_deny_rule_under_default_allow_is_rejected",
        &["*"],
        &[],
        "deny",
        "allow",
        "targets",
    );
}

#[test]
fn or_with_no_operands_is_rejected() {
    // A one-element array that passes `minItems: 1` and is still an OR over
    // the empty set — which is why the arity is stated per operator.
    assert_rejected_at_every_door(
        "or_with_no_operands_is_rejected",
        &["*"],
        &["$or"],
        "deny",
        "allow",
        "targets",
    );
}

#[test]
fn not_with_no_operands_is_rejected() {
    // §6.2.1 through v1.30.0 required this form to "evaluate to false
    // (fail-closed)". The parenthetical was wrong: a `deny` rule that never
    // matches refuses nothing, and under `default_effect: allow` the blocked
    // call is permitted.
    assert_rejected_at_every_door(
        "not_with_no_operands_is_rejected",
        &["*"],
        &["$not"],
        "deny",
        "allow",
        "targets",
    );
}

#[test]
fn not_with_two_operands_is_rejected() {
    // THE SECOND DEFECT, and the one that is not inert. §6.2.1 called this
    // implementation-defined — consult the first operand, ignore the rest —
    // and all three SDKs did exactly that, so the form was uniform across
    // implementations and uniformly wider than written: the operator excluded
    // two targets from an `allow` rule and the second one was granted.
    assert_rejected_at_every_door(
        "not_with_two_operands_is_rejected",
        &["*"],
        &["$not", "secrets.a", "secrets.b"],
        "allow",
        "deny",
        "targets",
    );
}

#[test]
fn not_with_two_operands_on_deny_rule_is_rejected() {
    // The same arity fault where the old reading is over-broad rather than
    // escalating. It fails for the same reason rather than surviving because
    // this effect happens to land on the safe side.
    assert_rejected_at_every_door(
        "not_with_two_operands_on_deny_rule_is_rejected",
        &["*"],
        &["$not", "secrets.a", "secrets.b"],
        "deny",
        "deny",
        "targets",
    );
}

#[test]
fn empty_pattern_string_is_rejected() {
    // The empty pattern matches only the empty module ID, which is not a legal
    // module ID, so the array is structurally never-matching rather than
    // semantically so and belongs at the door rather than in the validator.
    assert_rejected_at_every_door(
        "empty_pattern_string_is_rejected",
        &["*"],
        &[""],
        "deny",
        "allow",
        "targets",
    );
}

#[test]
fn empty_pattern_string_under_or_is_rejected() {
    // The same fault as an operand rather than as the whole array. An
    // implementation that checks only index 0 passes the case above.
    assert_rejected_at_every_door(
        "empty_pattern_string_under_or_is_rejected",
        &["*"],
        &["$or", ""],
        "deny",
        "allow",
        "targets",
    );
}

#[test]
fn reserved_token_after_operator_is_rejected() {
    // THE NESTING CASE. A pattern array is FLAT, but nothing said so before
    // v1.31.0 while `$or` / `$not` DO nest arbitrarily in `conditions`. An
    // operator who learned the condition grammar writes `["$or", "$not", "a"]`
    // expecting or-of-not and got an OR of two literals — matching `a`, and
    // also matching a module literally named `$not`, which §6.2.1's own
    // reserved-token clause says MUST NOT happen.
    assert_rejected_at_every_door(
        "reserved_token_after_operator_is_rejected",
        &["*"],
        &["$or", "$not", "a"],
        "allow",
        "deny",
        "targets",
    );
}

#[test]
fn reserved_token_in_flat_list_is_rejected() {
    // `["api.*", "$not", "cli.*"]` is not "api.* but not cli.*" — no such form
    // exists. Rejecting the token outside index 0 makes §6.2.1's reserved-token
    // MUST NOT hold by construction instead of by a check nothing performs.
    assert_rejected_at_every_door(
        "reserved_token_in_flat_list_is_rejected",
        &["*"],
        &["api.*", "$not", "cli.*"],
        "allow",
        "deny",
        "targets",
    );
}

#[test]
fn reserved_token_at_index_one_in_callers_is_rejected() {
    assert_rejected_at_every_door(
        "reserved_token_at_index_one_in_callers_is_rejected",
        &["api.*", "$or"],
        &["*"],
        "allow",
        "deny",
        "callers",
    );
}

// --- field parity: §6.2.1 constrains `callers` and `targets` identically -----
//
// An implementation that validates only `targets` passes almost every
// rejection above. Every structural shape rejected on `targets` is mirrored
// onto `callers` here so that asymmetry cannot pass.

#[test]
fn or_with_no_operands_in_callers_is_rejected() {
    assert_rejected_at_every_door(
        "or_with_no_operands_in_callers_is_rejected",
        &["$or"],
        &["*"],
        "deny",
        "allow",
        "callers",
    );
}

#[test]
fn not_with_no_operands_in_callers_is_rejected() {
    assert_rejected_at_every_door(
        "not_with_no_operands_in_callers_is_rejected",
        &["$not"],
        &["*"],
        "deny",
        "allow",
        "callers",
    );
}

#[test]
fn not_with_two_operands_in_callers_is_rejected() {
    assert_rejected_at_every_door(
        "not_with_two_operands_in_callers_is_rejected",
        &["$not", "admin.*", "ops.*"],
        &["*"],
        "allow",
        "deny",
        "callers",
    );
}

#[test]
fn empty_pattern_string_in_callers_is_rejected() {
    assert_rejected_at_every_door(
        "empty_pattern_string_in_callers_is_rejected",
        &[""],
        &["*"],
        "deny",
        "allow",
        "callers",
    );
}

#[test]
fn empty_pattern_string_under_or_in_callers_is_rejected() {
    // Not a duplicate of the case above: this one fails only if the
    // implementation scans the OPERANDS of a `$or` on the `callers` side.
    assert_rejected_at_every_door(
        "empty_pattern_string_under_or_in_callers_is_rejected",
        &["$or", ""],
        &["*"],
        "deny",
        "allow",
        "callers",
    );
}

#[test]
fn reserved_token_after_operator_in_callers_is_rejected() {
    // Distinct from `reserved_token_at_index_one_in_callers_is_rejected`, where
    // index 0 is an ordinary pattern: here index 0 is an OPERATOR, so an
    // implementation that stops checking positions once it has consumed a
    // leading `$or` passes that case and fails this one.
    assert_rejected_at_every_door(
        "reserved_token_after_operator_in_callers_is_rejected",
        &["$or", "$not", "admin"],
        &["*"],
        "allow",
        "deny",
        "callers",
    );
}

// ---------------------------------------------------------------------------
// Tier 2 — reported by validate_rules(), never rejected, never decisive
// ---------------------------------------------------------------------------

#[test]
fn not_of_wildcard_loads_and_is_reported() {
    // The case that proves the two tiers are distinct. `["$not", "*"]` has
    // perfectly legal arity — exactly one operand — and matches NOTHING,
    // producing the identical fail-open the arity shapes produce. It is
    // well-formed, so it MUST load; it protects nothing, so it MUST be
    // reported. Collapsing the tiers here turns a well-formed rule into one
    // that denies every call.
    assert_accepted_at_every_door(
        "not_of_wildcard_loads_and_is_reported",
        &["*"],
        &["$not", "*"],
        "deny",
        "allow",
    );
    assert_eq!(
        finding_paths(&["*"], &["$not", "*"], "deny", "allow"),
        vec!["targets".to_string()]
    );
}

#[test]
fn not_of_double_wildcard_loads_and_is_reported() {
    // `**` is universal too. The MUST-detect minimum is "any pattern
    // consisting only of wildcards", not the single literal `*` — an
    // implementation that string-compares against "*" passes the case above
    // and fails this one.
    assert_accepted_at_every_door(
        "not_of_double_wildcard_loads_and_is_reported",
        &["*"],
        &["$not", "**"],
        "deny",
        "allow",
    );
    assert_eq!(
        finding_paths(&["*"], &["$not", "**"], "deny", "allow"),
        vec!["targets".to_string()]
    );
}

#[test]
fn external_sentinel_as_target_loads_and_is_reported() {
    // `@external` is the caller-side sentinel §6.5 substitutes for a null
    // `caller_id`; no module ID is `@external`, so as a TARGET pattern it
    // matches nothing.
    assert_accepted_at_every_door(
        "external_sentinel_as_target_loads_and_is_reported",
        &["*"],
        &["@external"],
        "deny",
        "allow",
    );
    assert_eq!(
        finding_paths(&["*"], &["@external"], "deny", "allow"),
        vec!["targets".to_string()]
    );
}

#[test]
fn external_sentinel_as_caller_loads_clean() {
    // The control for the case above. `@external` in `callers` is the
    // documented way to write a rule about top-level entry points. A finding
    // that fires on both fields has read the rule as being about the token
    // rather than about the field.
    assert_accepted_at_every_door(
        "external_sentinel_as_caller_loads_clean",
        &["@external"],
        &["*"],
        "deny",
        "allow",
    );
    assert!(finding_paths(&["@external"], &["*"], "deny", "allow").is_empty());
}

#[test]
fn external_sentinel_beside_a_real_pattern_loads_clean() {
    // Tier 2 judges the array AS A WHOLE — the criterion is "matches no legal
    // module ID for any input", not "contains an unmatchable operand". The
    // array is OR-ed, so `api.*` still matches and the rule still protects
    // something. An implementation that reports any occurrence of `@external`
    // in `targets` passes the case above and fails here, and would be flagging
    // a rule that is doing its job.
    assert_accepted_at_every_door(
        "external_sentinel_beside_a_real_pattern_loads_clean",
        &["*"],
        &["api.*", "@external"],
        "deny",
        "allow",
    );
    assert!(finding_paths(&["*"], &["api.*", "@external"], "deny", "allow").is_empty());
    // The same array under an explicit `$or`, which is observably the same OR.
    assert!(finding_paths(&["*"], &["$or", "@external", "api.*"], "deny", "allow").is_empty());
}

#[test]
fn not_of_narrow_pattern_loads_clean() {
    // The control against tier 2 over-firing. `["$not", "cli.*"]` matches every
    // module outside `cli.`, which is the form's whole purpose. An
    // implementation that reports every `$not` has confused "negation" with
    // "matches nothing".
    assert_accepted_at_every_door(
        "not_of_narrow_pattern_loads_clean",
        &["*"],
        &["$not", "cli.*"],
        "deny",
        "allow",
    );
    assert!(finding_paths(&["*"], &["$not", "cli.*"], "deny", "allow").is_empty());
}

// ---------------------------------------------------------------------------
// Tier 2 changes no decision, and neither tier fires on a well-formed rule
// ---------------------------------------------------------------------------

fn acl_with_sink(rule: ACLRule, default_effect: &str) -> (ACL, Arc<Mutex<Vec<AuditEntry>>>) {
    ACL::init_builtin_handlers();
    let captured: Arc<Mutex<Vec<AuditEntry>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&captured);
    let mut acl = ACL::try_new(vec![rule], default_effect, None).expect("rule is well-formed");
    acl.set_audit_logger(move |entry: &AuditEntry| {
        sink.lock().expect("audit sink").push(entry.clone());
    });
    (acl, captured)
}

fn only_handler_error(captured: &Arc<Mutex<Vec<AuditEntry>>>) -> Option<String> {
    let entries = captured.lock().expect("audit sink");
    assert_eq!(entries.len(), 1, "exactly one audit entry per check()");
    entries[0].handler_error.clone()
}

#[test]
fn not_of_wildcard_does_not_change_the_decision() {
    // The other half of tier 2: reported, and inert BY DESIGN. The rule matches
    // nothing, so `cli.rm` falls through to `default_effect: allow` exactly as
    // it did before v1.31.0. A denial here means tier 2 was implemented as a
    // rejection or as an UNEVALUABLE fault, which would make a well-formed rule
    // deny every call.
    let (acl, captured) = acl_with_sink(rule_of(&["*"], &["$not", "*"], "deny"), "allow");

    let decision = acl.check_access(Some("api.gateway"), "cli.rm", None, None);
    assert_eq!(
        decision.access, "allow",
        "a tier-2 finding MUST NOT change any access decision (§6.2.1)"
    );
    assert!(
        !decision.approval_required,
        "and it raises no question either — the rule simply does not match"
    );
    assert_eq!(
        only_handler_error(&captured),
        None,
        "tier 2 is a validator finding — it MUST NOT reach the per-call handler-error scope"
    );
    assert_eq!(
        acl.validate_rules()
            .into_iter()
            .map(|f| f.condition_path)
            .collect::<Vec<_>>(),
        vec!["targets".to_string()],
        "and it MUST still be reported"
    );
}

#[test]
fn well_formed_rule_raises_no_finding() {
    // The control without which an implementation that flags every rule passes
    // everything else in this file.
    let (acl, captured) = acl_with_sink(rule_of(&["*"], &["cli.*"], "deny"), "allow");

    assert!(!acl.check(Some("api.gateway"), "cli.rm", None));
    assert_eq!(only_handler_error(&captured), None);
    assert!(acl.validate_rules().is_empty());
}
