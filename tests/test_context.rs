//! Tests for Context and Identity.

use apcore::cancel::CancelToken;
use apcore::context::{Context, Identity};
use serde_json::Value;
use std::collections::HashMap;

fn make_identity(id: &str, identity_type: &str, roles: Vec<&str>) -> Identity {
    Identity {
        id: id.to_string(),
        identity_type: identity_type.to_string(),
        roles: roles.iter().map(|s| s.to_string()).collect(),
        attrs: HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

#[test]
fn test_identity_basic_fields() {
    let id = make_identity("user-1", "Alice", vec!["admin", "user"]);
    assert_eq!(id.id, "user-1");
    assert_eq!(id.identity_type, "Alice");
    assert_eq!(id.roles.len(), 2);
    assert!(id.roles.contains(&"admin".to_string()));
}

#[test]
fn test_identity_empty_roles() {
    let id = make_identity("svc-1", "Service", vec![]);
    assert!(id.roles.is_empty());
}

#[test]
fn test_identity_with_attrs() {
    let mut id = make_identity("user-2", "Bob", vec![]);
    id.attrs
        .insert("tier".to_string(), serde_json::json!("premium"));
    assert_eq!(id.attrs["tier"], "premium");
}

#[test]
fn test_identity_serialization() {
    let id = make_identity("user-1", "Alice", vec!["admin"]);
    let json = serde_json::to_string(&id).unwrap();
    let restored: Identity = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.id, id.id);
    assert_eq!(restored.identity_type, id.identity_type);
    assert_eq!(restored.roles, id.roles);
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

#[test]
fn test_context_new_has_unique_trace_id() {
    let id = make_identity("user-1", "Alice", vec![]);
    let ctx1: Context<Value> = Context::new(id.clone());
    let ctx2: Context<Value> = Context::new(id);
    assert_ne!(ctx1.trace_id, ctx2.trace_id);
}

#[test]
fn test_context_initial_call_depth_is_zero() {
    let id = make_identity("user-1", "Alice", vec![]);
    let ctx: Context<Value> = Context::new(id);
    assert_eq!(ctx.call_depth, 0);
}

#[test]
fn test_context_initial_call_chain_is_empty() {
    let id = make_identity("user-1", "Alice", vec![]);
    let ctx: Context<Value> = Context::new(id);
    assert!(ctx.call_chain.is_empty());
}

#[test]
fn test_context_no_parent_by_default() {
    let id = make_identity("user-1", "Alice", vec![]);
    let ctx: Context<Value> = Context::new(id);
    assert!(ctx.parent_trace_id.is_none());
}

#[test]
fn test_context_identity_is_preserved() {
    let id = make_identity("svc-42", "MyService", vec!["reader"]);
    let ctx: Context<Value> = Context::new(id);
    assert_eq!(ctx.identity.id, "svc-42");
    assert_eq!(ctx.identity.roles, vec!["reader"]);
}

#[test]
fn test_context_no_cancel_token_by_default() {
    let id = make_identity("user-1", "Alice", vec![]);
    let ctx: Context<Value> = Context::new(id);
    assert!(ctx.cancel_token.is_none());
}

#[test]
fn test_context_with_cancel_token() {
    let id = make_identity("user-1", "Alice", vec![]);
    let mut ctx: Context<Value> = Context::new(id);
    let token = CancelToken::new();
    ctx.cancel_token = Some(token);
    assert!(!ctx.cancel_token.as_ref().unwrap().is_cancelled());
}

#[test]
fn test_context_data_starts_empty() {
    let id = make_identity("user-1", "Alice", vec![]);
    let ctx: Context<Value> = Context::new(id);
    assert!(ctx.data.is_empty());
}
