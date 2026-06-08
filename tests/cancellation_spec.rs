//! Spec-traced contract tests for the cancellation feature (Rust SDK).
//!
//! Mirrors the canonical Python suite
//! `apcore-python/tests/test_cancellation_spec.py` and the feature spec
//! `apcore/docs/features/cancellation.md`.
//!
//! The spec declares 2 `## Contract:` blocks:
//!   - CancelToken.cancel
//!   - CancelToken.raise_if_cancelled
//!
//! Each test fn carries a verbatim clause id of the form
//! `cancellation.<method>.<kind>.<detail>` in a leading `// clause:` comment so
//! cross-language diffs line up.
//!
//! Cross-language gap: the contract block names the second method
//! `CancelToken.raise_if_cancelled`, but the Rust SDK (`src/cancel.rs`)
//! implements the cancellation check as `check()` (returning
//! `Result<(), ExecutionCancelledError>`). There is no `raise_if_cancelled`
//! symbol — identical to the Python SDK, which implements it as `check()`. Per
//! the missing-symbol rule, every clause that targets `raise_if_cancelled` by
//! name is emitted as `#[ignore]` documenting the gap. A separate error-type
//! guard asserts the real `check()` path emits the spec'd code so the gap is
//! purely a method-naming mismatch.

use apcore::cancel::{CancelToken, ExecutionCancelledError};
use apcore::errors::{ErrorCode, ModuleError};

// ---------------------------------------------------------------------------
// Contract: CancelToken.cancel
// ---------------------------------------------------------------------------

// clause: cancellation.cancel.property.thread_safe
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_cancel_property_thread_safe() {
    // Launch >=8 concurrent cancel() calls on distinct tokens; assert no panic
    // and every token ends in a consistent cancelled state.
    let tokens: Vec<CancelToken> = (0..16).map(|_| CancelToken::new()).collect();

    let mut handles = Vec::new();
    for tok in &tokens {
        let tok = tok.clone();
        handles.push(tokio::spawn(async move {
            // Yield so calls genuinely interleave across worker threads.
            tokio::task::yield_now().await;
            tok.cancel();
        }));
    }

    for h in handles {
        h.await.expect("cancel task must not panic");
    }

    // Final state must be consistent: all tokens cancelled.
    assert!(tokens.iter().all(CancelToken::is_cancelled));
}

// clause: cancellation.cancel.property.thread_safe
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_cancel_property_thread_safe_shared_token() {
    // Concurrent cancel() of a single shared token from many tasks must
    // converge to exactly one consistent cancelled state with no panic.
    let shared = CancelToken::new();

    let mut handles = Vec::new();
    for _ in 0..16 {
        let tok = shared.clone();
        handles.push(tokio::spawn(async move {
            tokio::task::yield_now().await;
            tok.cancel();
        }));
    }

    for h in handles {
        h.await.expect("shared cancel task must not panic");
    }

    assert!(shared.is_cancelled());
}

// clause: cancellation.cancel.property.idempotent
#[test]
fn cancellation_cancel_property_idempotent() {
    // Call cancel() twice; assert identical observable outcome and state
    // (is_cancelled stays true, no panic). check() behaves identically after.
    let token = CancelToken::new();

    token.cancel();
    let first_state = token.is_cancelled();
    token.cancel(); // Second call must be a safe no-op.
    let second_state = token.is_cancelled();

    assert!(first_state);
    assert!(second_state);
    assert_eq!(first_state, second_state);

    // check() must still report cancellation after the repeated cancel.
    assert!(token.check().is_err());
}

// ---------------------------------------------------------------------------
// Contract: CancelToken.raise_if_cancelled
//
// MISSING SYMBOL: the Rust SDK has no `raise_if_cancelled` method on
// CancelToken (the equivalent behavior is `check()`). These clauses are
// recorded as ignored so the cross-language naming gap is documented as a skip
// rather than a coarse compile failure — identical to the Python suite.
// ---------------------------------------------------------------------------

// clause: cancellation.raise_if_cancelled.error.EXECUTION_CANCELLED
#[test]
#[ignore = "cancellation.raise_if_cancelled.error.EXECUTION_CANCELLED: missing symbol CancelToken::raise_if_cancelled (contract gap) — Rust SDK implements this as CancelToken::check()"]
fn cancellation_raise_if_cancelled_error_execution_cancelled() {
    unreachable!("missing symbol: CancelToken::raise_if_cancelled");
}

// clause: cancellation.raise_if_cancelled.property.thread_safe
#[test]
#[ignore = "cancellation.raise_if_cancelled.property.thread_safe: missing symbol CancelToken::raise_if_cancelled (contract gap) — Rust SDK implements this as CancelToken::check()"]
fn cancellation_raise_if_cancelled_property_thread_safe() {
    unreachable!("missing symbol: CancelToken::raise_if_cancelled");
}

// clause: cancellation.raise_if_cancelled.property.pure
#[test]
#[ignore = "cancellation.raise_if_cancelled.property.pure: missing symbol CancelToken::raise_if_cancelled (contract gap) — Rust SDK implements this as CancelToken::check()"]
fn cancellation_raise_if_cancelled_property_pure() {
    unreachable!("missing symbol: CancelToken::raise_if_cancelled");
}

// ---------------------------------------------------------------------------
// Error-type guard: ensure the declared error type/code referenced by the
// raise_if_cancelled contract actually exists with the spec'd code, so the gap
// above is purely a method-name mismatch (not a missing error type). The live
// check() path is the Rust equivalent of raise_if_cancelled.
// ---------------------------------------------------------------------------

// clause: cancellation.raise_if_cancelled.error.EXECUTION_CANCELLED
#[test]
fn cancellation_execution_cancelled_error_code_matches_spec() {
    // The contract requires ExecutionCancelledError(code=EXECUTION_CANCELLED).
    // Verify the error TYPE and CODE field match exactly via the live check()
    // path, confirming the gap is method-naming only.
    let token = CancelToken::new();
    token.cancel();

    // Typed error variant proves the declared error type exists.
    let err: ExecutionCancelledError = token.check().expect_err("expected cancel error");
    assert!(!err.message.is_empty());

    // Widen to ModuleError and assert the code matches the spec exactly.
    let module_err: ModuleError = err.into();
    assert_eq!(module_err.code, ErrorCode::ExecutionCancelled);

    // The wire-string code MUST be exactly "EXECUTION_CANCELLED"
    // (SCREAMING_SNAKE_CASE serialization of the ErrorCode variant).
    let code_str = serde_json::to_value(module_err.code).expect("serialize code");
    assert_eq!(
        code_str,
        serde_json::Value::String("EXECUTION_CANCELLED".to_string())
    );
}
