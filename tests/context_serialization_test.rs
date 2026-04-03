//! Tests for Context serialize() / deserialize() protocol.

use apcore::context::{Context, Identity};
use std::collections::HashMap;

fn make_identity() -> Identity {
    Identity::new(
        "user-1".to_string(),
        "user".to_string(),
        vec!["admin".to_string()],
        {
            let mut attrs = HashMap::new();
            attrs.insert("org".to_string(), serde_json::json!("acme"));
            attrs
        },
    )
}

fn make_ctx() -> Context<()> {
    let identity = make_identity();
    Context {
        trace_id: "trace-abc-123".to_string(),
        identity: Some(identity),
        services: (),
        caller_id: Some("api.users.get".to_string()),
        data: std::sync::Arc::new(std::sync::RwLock::new(HashMap::new())),
        call_chain: vec!["api.users.get".to_string()],
        redacted_inputs: None,
        cancel_token: None,
        global_deadline: None,
        executor: None,
    }
}

#[test]
fn test_serialize_includes_context_version() {
    // AC-003
    let ctx = make_ctx();
    let serialized = ctx.serialize();
    assert_eq!(serialized["_context_version"], 1);
}

#[test]
fn test_serialize_includes_required_fields() {
    let ctx = make_ctx();
    let serialized = ctx.serialize();
    let obj = serialized.as_object().unwrap();
    assert!(obj.contains_key("trace_id"));
    assert!(obj.contains_key("caller_id"));
    assert!(obj.contains_key("call_chain"));
    assert!(obj.contains_key("identity"));
    assert!(obj.contains_key("data"));
}

#[test]
fn test_serialize_identity_structure() {
    let ctx = make_ctx();
    let serialized = ctx.serialize();
    let identity = &serialized["identity"];
    assert_eq!(identity["id"], "user-1");
    assert_eq!(identity["type"], "user");
    assert_eq!(identity["roles"], serde_json::json!(["admin"]));
    assert_eq!(identity["attrs"]["org"], "acme");
}

#[test]
fn test_serialize_excludes_executor() {
    // AC-004
    let ctx = make_ctx();
    let serialized = ctx.serialize();
    let obj = serialized.as_object().unwrap();
    assert!(!obj.contains_key("executor"));
}

#[test]
fn test_serialize_excludes_services() {
    // AC-004
    let ctx = make_ctx();
    let serialized = ctx.serialize();
    let obj = serialized.as_object().unwrap();
    assert!(!obj.contains_key("services"));
}

#[test]
fn test_serialize_excludes_cancel_token() {
    // AC-004
    let ctx = make_ctx();
    let serialized = ctx.serialize();
    let obj = serialized.as_object().unwrap();
    assert!(!obj.contains_key("cancel_token"));
}

#[test]
fn test_serialize_excludes_global_deadline() {
    // AC-004
    let ctx = make_ctx();
    let serialized = ctx.serialize();
    let obj = serialized.as_object().unwrap();
    assert!(!obj.contains_key("global_deadline"));
}

#[test]
fn test_serialize_filters_underscore_data_keys() {
    // AC-005
    let ctx = make_ctx();
    {
        let mut data = ctx.data.write().unwrap();
        data.insert("_apcore.internal".to_string(), serde_json::json!("hidden"));
        data.insert("_secret_key".to_string(), serde_json::json!("hidden"));
        data.insert("public.counter".to_string(), serde_json::json!(42));
        data.insert("app.name".to_string(), serde_json::json!("test"));
    }
    let serialized = ctx.serialize();
    let data = serialized["data"].as_object().unwrap();
    assert!(!data.contains_key("_apcore.internal"));
    assert!(!data.contains_key("_secret_key"));
    assert_eq!(data["public.counter"], 42);
    assert_eq!(data["app.name"], "test");
}

#[test]
fn test_serialize_empty_data() {
    let ctx = make_ctx();
    {
        let mut data = ctx.data.write().unwrap();
        data.insert("_private".to_string(), serde_json::json!("hidden"));
    }
    let serialized = ctx.serialize();
    let data = serialized["data"].as_object().unwrap();
    assert!(data.is_empty());
}

#[test]
fn test_deserialize_roundtrip() {
    let ctx = make_ctx();
    {
        let mut data = ctx.data.write().unwrap();
        data.insert("app.counter".to_string(), serde_json::json!(42));
    }
    let serialized = ctx.serialize();
    let restored: Context<()> = Context::deserialize(serialized).unwrap();
    assert_eq!(restored.trace_id, ctx.trace_id);
    assert_eq!(restored.caller_id, ctx.caller_id);
    let data = restored.data.read().unwrap();
    assert_eq!(data.get("app.counter"), Some(&serde_json::json!(42)));
}

#[test]
fn test_deserialize_executor_is_none() {
    let ctx = make_ctx();
    let serialized = ctx.serialize();
    let restored: Context<()> = Context::deserialize(serialized).unwrap();
    assert!(restored.executor.is_none());
}

#[test]
fn test_deserialize_services_is_default() {
    let ctx = make_ctx();
    let serialized = ctx.serialize();
    let restored: Context<()> = Context::deserialize(serialized).unwrap();
    assert_eq!(restored.services, ());
}

#[test]
fn test_deserialize_cancel_token_is_none() {
    let ctx = make_ctx();
    let serialized = ctx.serialize();
    let restored: Context<()> = Context::deserialize(serialized).unwrap();
    assert!(restored.cancel_token.is_none());
}

#[test]
fn test_deserialize_global_deadline_is_none() {
    let ctx = make_ctx();
    let serialized = ctx.serialize();
    let restored: Context<()> = Context::deserialize(serialized).unwrap();
    assert!(restored.global_deadline.is_none());
}

#[test]
fn test_deserialize_future_version_does_not_crash() {
    // version > 1 should warn but succeed
    let data = serde_json::json!({
        "_context_version": 99,
        "trace_id": "abc-123",
        "caller_id": "test",
        "call_chain": [],
        "data": {}
    });
    let restored: Context<()> = Context::deserialize(data).unwrap();
    assert_eq!(restored.trace_id, "abc-123");
}

#[test]
fn test_deserialize_unknown_top_level_fields() {
    let data = serde_json::json!({
        "_context_version": 1,
        "trace_id": "abc-123",
        "caller_id": "test",
        "call_chain": [],
        "data": {"custom": "value"},
        "future_field": "should_not_crash"
    });
    let restored: Context<()> = Context::deserialize(data).unwrap();
    assert_eq!(restored.trace_id, "abc-123");
    let data_map = restored.data.read().unwrap();
    assert_eq!(data_map.get("custom"), Some(&serde_json::json!("value")));
}
