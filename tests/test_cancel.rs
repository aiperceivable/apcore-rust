//! Tests for CancelToken — cooperative cancellation primitives.

use apcore::cancel::CancelToken;

#[test]
fn test_new_token_is_not_cancelled() {
    let token = CancelToken::new();
    assert!(!token.is_cancelled());
}

#[test]
fn test_cancel_sets_flag() {
    let token = CancelToken::new();
    token.cancel();
    assert!(token.is_cancelled());
}

#[test]
fn test_cancel_is_idempotent() {
    let token = CancelToken::new();
    token.cancel();
    token.cancel(); // second call must not panic
    assert!(token.is_cancelled());
}

#[test]
fn test_clone_shares_state() {
    let token = CancelToken::new();
    let clone = token.clone();

    assert!(!clone.is_cancelled());
    token.cancel();
    // clone sees the same cancellation
    assert!(clone.is_cancelled());
}

#[test]
fn test_clone_cancels_original() {
    let token = CancelToken::new();
    let clone = token.clone();

    clone.cancel();
    assert!(token.is_cancelled());
}

#[test]
fn test_default_is_not_cancelled() {
    let token = CancelToken::default();
    assert!(!token.is_cancelled());
}

#[test]
fn test_multiple_clones_share_state() {
    let t1 = CancelToken::new();
    let t2 = t1.clone();
    let t3 = t2.clone();

    assert!(!t3.is_cancelled());
    t1.cancel();
    assert!(t2.is_cancelled());
    assert!(t3.is_cancelled());
}

// ---------------------------------------------------------------------------
// Sync CANCEL-001 — typed Result from check()
// ---------------------------------------------------------------------------

#[test]
fn test_check_returns_typed_execution_cancelled_error() {
    use apcore::cancel::ExecutionCancelledError;
    use apcore::errors::{ErrorCode, ModuleError};

    let token = CancelToken::new();
    // Before cancellation: Ok.
    assert!(token.check().is_ok());

    token.cancel();
    // After cancellation: a typed ExecutionCancelledError that we can match on
    // — pattern proves the return type is `Result<(), ExecutionCancelledError>`.
    match token.check() {
        Err(ExecutionCancelledError {
            ref message,
            ref module_id,
        }) => {
            assert!(!message.is_empty());
            // A bare `check()` knows no module, so it reports none rather than
            // fabricating the "@unknown" sentinel. Parity with apcore-python
            // and apcore-typescript, whose `check()` produces `details == {}`.
            assert!(module_id.is_none());
        }
        Ok(()) => panic!("expected typed cancel error after cancel()"),
    }

    // Widening back to ModuleError must work via the From impl.
    let cancelled = token.check().unwrap_err();
    let me: ModuleError = cancelled.into();
    assert_eq!(me.code, ErrorCode::ExecutionCancelled);
}

#[test]
fn test_check_for_carries_module_id() {
    use apcore::cancel::ExecutionCancelledError;

    let token = CancelToken::new();
    token.cancel();
    let err: ExecutionCancelledError = token.check_for("ns.target").unwrap_err();
    assert_eq!(err.module_id.as_deref(), Some("ns.target"));
}

#[test]
fn test_check_produces_no_module_id_detail() {
    // PROTOCOL_SPEC names no "@unknown" module. `check()` used to invent it and
    // write it into `details`, so an external caller following the spec's own
    // Rust example emitted a wire payload no other SDK emits — apcore-python and
    // apcore-typescript both produce `details == {}` here.
    use apcore::errors::ModuleError;

    let token = CancelToken::new();
    token.cancel();
    let err: ModuleError = token.check().unwrap_err().into();
    assert!(
        err.details.is_empty(),
        "check() must not fabricate a module_id: {:?}",
        err.details
    );

    // check_for() still carries it.
    let err: ModuleError = token.check_for("ns.target").unwrap_err().into();
    assert_eq!(
        err.details.get("module_id").and_then(|v| v.as_str()),
        Some("ns.target")
    );
}
