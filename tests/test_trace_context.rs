//! Tests for TraceParent and TraceContext.

use apcore::trace_context::{TraceContext, TraceParent};

fn make_traceparent(trace_id: &str, parent_id: &str) -> TraceParent {
    TraceParent {
        version: 0,
        trace_id: trace_id.to_string(),
        parent_id: parent_id.to_string(),
        trace_flags: 1,
    }
}

// ---------------------------------------------------------------------------
// TraceParent
// ---------------------------------------------------------------------------

#[test]
fn test_traceparent_to_header() {
    let tp = make_traceparent("4bf92f3577b34da6a3ce929d0e0e4736", "00f067aa0ba902b7");
    let header = tp.to_header();
    assert_eq!(
        header,
        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
    );
}

#[test]
fn test_traceparent_version_zero_formatted_as_two_hex_digits() {
    let tp = make_traceparent("abc", "def");
    let header = tp.to_header();
    assert!(header.starts_with("00-"));
}

#[test]
fn test_traceparent_flags_formatted_as_two_hex_digits() {
    let tp = TraceParent {
        version: 0,
        trace_id: "aaa".to_string(),
        parent_id: "bbb".to_string(),
        trace_flags: 0,
    };
    let header = tp.to_header();
    assert!(header.ends_with("-00"));
}

#[test]
fn test_traceparent_serialization() {
    let tp = make_traceparent("trace-abc", "span-xyz");
    let json = serde_json::to_string(&tp).unwrap();
    let restored: TraceParent = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.trace_id, tp.trace_id);
    assert_eq!(restored.parent_id, tp.parent_id);
    assert_eq!(restored.version, tp.version);
    assert_eq!(restored.trace_flags, tp.trace_flags);
}

// ---------------------------------------------------------------------------
// TraceContext
// ---------------------------------------------------------------------------

#[test]
fn test_trace_context_new() {
    let tp = make_traceparent("trace-1", "parent-1");
    let ctx = TraceContext::new(tp);
    assert_eq!(ctx.traceparent.trace_id, "trace-1");
    assert!(ctx.tracestate.is_empty());
    assert!(ctx.baggage.is_empty());
}

#[test]
fn test_trace_context_baggage() {
    let tp = make_traceparent("t", "p");
    let mut ctx = TraceContext::new(tp);
    ctx.baggage
        .insert("user_id".to_string(), "alice".to_string());
    assert_eq!(ctx.baggage["user_id"], "alice");
}

#[test]
fn test_trace_context_tracestate() {
    let tp = make_traceparent("t", "p");
    let mut ctx = TraceContext::new(tp);
    ctx.tracestate
        .push(("vendor1".to_string(), "abc123".to_string()));
    assert_eq!(ctx.tracestate.len(), 1);
    assert_eq!(ctx.tracestate[0].0, "vendor1");
}

#[test]
fn test_trace_context_serialization() {
    let tp = make_traceparent("trace-abc", "span-xyz");
    let ctx = TraceContext::new(tp);
    let json = serde_json::to_string(&ctx).unwrap();
    let restored: TraceContext = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.traceparent.trace_id, "trace-abc");
}
