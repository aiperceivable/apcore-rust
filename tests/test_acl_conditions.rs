//! Tests for ACL conditions redesign — handler registry, dispatch, and compound operators.

use apcore::acl::{ACLRule, ACL};
use apcore::acl_handlers::{
    register_condition, ACLConditionHandler, IdentityTypesHandler, MaxCallDepthHandler,
    RolesHandler, CONDITION_HANDLERS,
};
use apcore::context::{Context, Identity};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_context(
    identity_type: &str,
    roles: Vec<String>,
    call_chain: Vec<String>,
) -> Context<Value> {
    let identity = Identity::new(
        "test-user".to_string(),
        identity_type.to_string(),
        roles,
        HashMap::new(),
    );
    let mut ctx = Context::new(identity);
    ctx.call_chain = call_chain;
    ctx
}

fn make_acl_with_condition(condition_key: &str, condition_value: Value) -> ACL {
    let mut conditions = serde_json::Map::new();
    conditions.insert(condition_key.to_string(), condition_value);
    let rule = ACLRule {
        callers: vec!["*".to_string()],
        targets: vec!["*".to_string()],
        effect: "allow".to_string(),
        description: None,
        conditions: Some(Value::Object(conditions)),
    };
    ACL::new(vec![rule], "deny", None)
}

fn init_handlers() {
    ACL::init_builtin_handlers();
}

// ---------------------------------------------------------------------------
// Handler Registry
// ---------------------------------------------------------------------------

#[test]
fn test_register_condition_adds_handler() {
    struct TestHandler;
    #[async_trait]
    impl ACLConditionHandler for TestHandler {
        async fn evaluate(&self, value: &Value, _ctx: &Context<Value>) -> bool {
            value.as_bool().unwrap_or(false)
        }
    }

    init_handlers();
    register_condition("_test_custom_rs", Arc::new(TestHandler));
    let handlers = CONDITION_HANDLERS.read();
    assert!(handlers.contains_key("_test_custom_rs"));
}

#[test]
fn test_builtin_handlers_registered() {
    init_handlers();
    let handlers = CONDITION_HANDLERS.read();
    for key in &["identity_types", "roles", "max_call_depth", "$or", "$not"] {
        assert!(
            handlers.contains_key(*key),
            "Missing built-in handler: {key}"
        );
    }
}

// ---------------------------------------------------------------------------
// Built-in Handlers — Unit Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_identity_types_match() {
    let handler = IdentityTypesHandler;
    let ctx = make_context("service", vec![], vec![]);
    assert!(handler.evaluate(&json!(["service", "admin"]), &ctx).await);
}

#[tokio::test]
async fn test_identity_types_no_match() {
    let handler = IdentityTypesHandler;
    let ctx = make_context("user", vec![], vec![]);
    assert!(!handler.evaluate(&json!(["service", "admin"]), &ctx).await);
}

#[tokio::test]
async fn test_identity_types_no_identity() {
    let handler = IdentityTypesHandler;
    let ctx: Context<Value> = Context::anonymous();
    assert!(!handler.evaluate(&json!(["user"]), &ctx).await);
}

#[tokio::test]
async fn test_roles_match() {
    let handler = RolesHandler;
    let ctx = make_context(
        "user",
        vec!["admin".to_string(), "viewer".to_string()],
        vec![],
    );
    assert!(handler.evaluate(&json!(["admin"]), &ctx).await);
}

#[tokio::test]
async fn test_roles_no_match() {
    let handler = RolesHandler;
    let ctx = make_context("user", vec!["viewer".to_string()], vec![]);
    assert!(!handler.evaluate(&json!(["admin"]), &ctx).await);
}

#[tokio::test]
async fn test_max_call_depth_within_limit() {
    let handler = MaxCallDepthHandler;
    let ctx = make_context("user", vec![], vec!["a".to_string(), "b".to_string()]);
    assert!(handler.evaluate(&json!(5), &ctx).await);
}

#[tokio::test]
async fn test_max_call_depth_exceeds_limit() {
    let handler = MaxCallDepthHandler;
    let ctx = make_context(
        "user",
        vec![],
        vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ],
    );
    assert!(!handler.evaluate(&json!(3), &ctx).await);
}

// Regression: sync finding A-D-024 — `max_call_depth` MUST accept the dict
// form `{"lte": N}` for cross-language parity with apcore-python and
// apcore-typescript. Previously Rust accepted only the bare integer form
// and silently fail-closed on the dict form.
#[tokio::test]
async fn test_max_call_depth_accepts_lte_dict_within_limit() {
    let handler = MaxCallDepthHandler;
    let ctx = make_context("user", vec![], vec!["a".to_string(), "b".to_string()]);
    assert!(handler.evaluate(&json!({"lte": 5}), &ctx).await);
}

#[tokio::test]
async fn test_max_call_depth_accepts_lte_dict_exceeds_limit() {
    let handler = MaxCallDepthHandler;
    let ctx = make_context(
        "user",
        vec![],
        vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ],
    );
    assert!(!handler.evaluate(&json!({"lte": 3}), &ctx).await);
}

#[tokio::test]
async fn test_max_call_depth_rejects_unrecognized_form() {
    // Other dict shapes (e.g. {"max": N}, {"gte": N}) are NOT spec-supported
    // and remain fail-closed. Only the {"lte": N} form is honored.
    let handler = MaxCallDepthHandler;
    let ctx = make_context("user", vec![], vec!["a".to_string()]);
    assert!(!handler.evaluate(&json!({"max": 5}), &ctx).await);
    assert!(!handler.evaluate(&json!("string-value"), &ctx).await);
    assert!(!handler.evaluate(&json!(null), &ctx).await);
}

// ---------------------------------------------------------------------------
// Compound Handlers (via full check)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_or_passes_when_any_match() {
    init_handlers();
    let acl = make_acl_with_condition(
        "$or",
        json!([
            {"roles": ["admin"]},
            {"identity_types": ["service"]},
        ]),
    );
    let ctx = make_context("user", vec!["admin".to_string()], vec![]);
    let result = acl.check(Some("caller"), "target", Some(&ctx));
    assert!(result);
}

#[tokio::test]
async fn test_or_fails_when_none_match() {
    init_handlers();
    let acl = make_acl_with_condition(
        "$or",
        json!([
            {"roles": ["admin"]},
            {"identity_types": ["service"]},
        ]),
    );
    let ctx = make_context("user", vec!["viewer".to_string()], vec![]);
    let result = acl.check(Some("caller"), "target", Some(&ctx));
    assert!(!result);
}

#[tokio::test]
async fn test_or_empty_list_returns_false() {
    init_handlers();
    let acl = make_acl_with_condition("$or", json!([]));
    let ctx = make_context("user", vec![], vec![]);
    let result = acl.check(Some("caller"), "target", Some(&ctx));
    assert!(!result);
}

#[tokio::test]
async fn test_not_negates_conditions() {
    init_handlers();
    let acl = make_acl_with_condition("$not", json!({"identity_types": ["service"]}));
    let ctx_user = make_context("user", vec![], vec![]);
    let ctx_service = make_context("service", vec![], vec![]);
    assert!(acl.check(Some("caller"), "target", Some(&ctx_user)));
    assert!(!acl.check(Some("caller"), "target", Some(&ctx_service)));
}

#[tokio::test]
async fn test_not_non_dict_returns_false() {
    init_handlers();
    let acl = make_acl_with_condition("$not", json!("invalid"));
    let ctx = make_context("user", vec![], vec![]);
    assert!(!acl.check(Some("caller"), "target", Some(&ctx)));
}

// ---------------------------------------------------------------------------
// Fail-closed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_unknown_condition_fails_closed() {
    init_handlers();
    let acl = make_acl_with_condition("nonexistent", json!(true));
    let ctx = make_context("user", vec![], vec![]);
    assert!(!acl.check(Some("caller"), "target", Some(&ctx)));
}

// ---------------------------------------------------------------------------
// Empty callers fix (AC-033)
// ---------------------------------------------------------------------------

#[test]
fn test_empty_callers_matches_nothing() {
    let rule = ACLRule {
        callers: vec![],
        targets: vec!["*".to_string()],
        effect: "allow".to_string(),
        description: None,
        conditions: None,
    };
    let acl = ACL::new(vec![rule], "deny", None);
    assert!(!acl.check(Some("anyone"), "target", None));
}

#[test]
fn test_empty_targets_matches_nothing() {
    let rule = ACLRule {
        callers: vec!["*".to_string()],
        targets: vec![],
        effect: "allow".to_string(),
        description: None,
        conditions: None,
    };
    let acl = ACL::new(vec![rule], "deny", None);
    assert!(!acl.check(Some("anyone"), "target", None));
}

// ---------------------------------------------------------------------------
// audit_logger in constructor (AC-035)
// ---------------------------------------------------------------------------

#[test]
fn test_audit_logger_via_constructor() {
    let logged = Arc::new(std::sync::Mutex::new(Vec::new()));
    let logged_clone = logged.clone();
    let audit_fn = move |entry: &apcore::acl::AuditEntry| {
        logged_clone.lock().unwrap().push(entry.decision.clone());
    };
    let acl = ACL::new(
        vec![ACLRule {
            callers: vec!["*".to_string()],
            targets: vec!["*".to_string()],
            effect: "allow".to_string(),
            description: None,
            conditions: None,
        }],
        "deny",
        Some(Arc::new(audit_fn)),
    );
    acl.check(Some("a"), "b", None);
    let entries = logged.lock().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0], "allow");
}

// ---------------------------------------------------------------------------
// async_check
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_async_check_basic() {
    init_handlers();
    let acl = make_acl_with_condition("roles", json!(["admin"]));
    let ctx = make_context("user", vec!["admin".to_string()], vec![]);
    let result = acl.async_check(Some("caller"), "target", Some(&ctx)).await;
    assert!(result);
}

#[tokio::test]
async fn test_async_check_default_deny() {
    let acl = ACL::new(vec![], "deny", None);
    let result = acl.async_check(Some("caller"), "target", None).await;
    assert!(!result);
}

#[tokio::test]
async fn test_async_check_default_allow() {
    let acl = ACL::new(vec![], "allow", None);
    let result = acl.async_check(Some("caller"), "target", None).await;
    assert!(result);
}

// ---------------------------------------------------------------------------
// Sync/async path agreement (D5 regression)
// ---------------------------------------------------------------------------

/// Verify that ACL::check (sync, noop-waker path) and ACL::async_check produce
/// the same result for all built-in condition handlers that complete immediately.
/// Drift between the two paths is the failure mode flagged in the architecture review.
#[tokio::test]
async fn test_sync_and_async_check_agree_on_builtin_conditions() {
    init_handlers();

    // Case 1: identity_types matches → both allow
    let acl = make_acl_with_condition("identity_types", json!(["service"]));
    let ctx = make_context("service", vec![], vec![]);
    let sync_result = acl.check(Some("caller"), "target", Some(&ctx));
    let async_result = acl.async_check(Some("caller"), "target", Some(&ctx)).await;
    assert_eq!(
        sync_result, async_result,
        "sync check and async_check must agree for identity_types (match)"
    );

    // Case 2: identity_types no match → both deny
    let ctx_user = make_context("user", vec![], vec![]);
    let sync_result = acl.check(Some("caller"), "target", Some(&ctx_user));
    let async_result = acl
        .async_check(Some("caller"), "target", Some(&ctx_user))
        .await;
    assert_eq!(
        sync_result, async_result,
        "sync check and async_check must agree for identity_types (no match)"
    );

    // Case 3: roles condition
    let acl_roles = make_acl_with_condition("roles", json!(["admin"]));
    let ctx_admin = make_context("user", vec!["admin".to_string()], vec![]);
    let sync_result = acl_roles.check(Some("caller"), "target", Some(&ctx_admin));
    let async_result = acl_roles
        .async_check(Some("caller"), "target", Some(&ctx_admin))
        .await;
    assert_eq!(
        sync_result, async_result,
        "sync check and async_check must agree for roles condition"
    );
}
