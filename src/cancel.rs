// APCore Protocol — Cancellation tokens
// Spec reference: Cooperative cancellation for module execution

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::errors::{ErrorCode, ModuleError};

/// Error raised when an execution is cancelled mid-flight.
///
/// Mirrors `apcore-python.ExecutionCancelledError(ModuleError)` and
/// `apcore-typescript ExecutionCancelledError extends ModuleError`. Carries
/// `message` (a human-readable cancellation reason) and, when the caller knew
/// it, `module_id` (the module that was running).
///
/// `module_id` is `Option` because a bare [`CancelToken::check`] has no module
/// in hand. It used to fabricate the sentinel `"@unknown"` — a string that
/// appears nowhere in `protocol-spec.md` — and write it into the error
/// `details`, so an external caller following the spec's own Rust example
/// produced a payload no other SDK produces (`check()` yields `details == {}`
/// in both peers). `check_for` still populates it.
#[derive(Debug, Clone, thiserror::Error)]
pub struct ExecutionCancelledError {
    /// ID of the module whose execution was cancelled, when known.
    pub module_id: Option<String>,
    /// Human-readable reason or description for the cancellation.
    pub message: String,
}

impl std::fmt::Display for ExecutionCancelledError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.module_id {
            Some(module_id) => write!(
                f,
                "ExecutionCancelledError: module '{}' — {}",
                module_id, self.message
            ),
            None => write!(f, "ExecutionCancelledError: {}", self.message),
        }
    }
}

impl ExecutionCancelledError {
    /// Build an `ExecutionCancelledError` with the given module ID and message.
    pub fn new(module_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            module_id: Some(module_id.into()),
            message: message.into(),
        }
    }

    /// Build an `ExecutionCancelledError` with no module ID — the shape a bare
    /// [`CancelToken::check`] produces, matching its Python and TypeScript
    /// counterparts, which carry no `module_id` detail either.
    pub fn without_module(message: impl Into<String>) -> Self {
        Self {
            module_id: None,
            message: message.into(),
        }
    }

    /// Convert into a generic [`ModuleError`] with code
    /// `ErrorCode::ExecutionCancelled`. Mirrors the `to_module_error()`
    /// helpers used by the other typed-error structs in `errors.rs`.
    ///
    /// `module_id` reaches `details` only when it is known; an unknown module
    /// leaves `details` empty rather than inventing a placeholder value.
    #[must_use]
    pub fn to_module_error(&self) -> ModuleError {
        let mut err = ModuleError::new(ErrorCode::ExecutionCancelled, &self.message);
        if let Some(module_id) = &self.module_id {
            err.details.insert(
                "module_id".to_string(),
                serde_json::Value::String(module_id.clone()),
            );
        }
        err
    }
}

impl From<ExecutionCancelledError> for ModuleError {
    fn from(value: ExecutionCancelledError) -> Self {
        value.to_module_error()
    }
}

/// Token used to signal cancellation to a running execution.
#[derive(Debug, Clone)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
}

impl CancelToken {
    /// Create a new cancel token in the non-cancelled state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Signal cancellation.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Check whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Check if cancelled and return [`ExecutionCancelledError`] if so.
    ///
    /// Sync CANCEL-001 (BREAKING): the return type was previously
    /// `Result<(), ModuleError>`. The typed variant matches Python's
    /// `ExecutionCancelledError` subclass and TS's `extends ModuleError`
    /// hierarchy. Use `.into()` (or `?` against a `ModuleError` context)
    /// to widen back to `ModuleError`:
    ///
    /// ```rust,ignore
    /// fn run(token: &CancelToken) -> Result<(), ModuleError> {
    ///     token.check()?; // ExecutionCancelledError → ModuleError via From impl
    ///     Ok(())
    /// }
    /// ```
    pub fn check(&self) -> Result<(), ExecutionCancelledError> {
        if self.is_cancelled() {
            // No module is in hand here, so none is reported — see
            // `ExecutionCancelledError` for why no sentinel is substituted.
            Err(ExecutionCancelledError::without_module(
                "Execution was cancelled",
            ))
        } else {
            Ok(())
        }
    }

    /// Check if cancelled and return [`ExecutionCancelledError`] if so.
    ///
    /// The spec's canonical name for this check (`docs/features/cancellation.md`
    /// "## Contract: CancelToken.raise_if_cancelled") — identical behavior to
    /// [`Self::check`], which this crate added first and keeps: `check()` and
    /// `check_for()` are used internally throughout this crate and documented
    /// elsewhere in this file, so neither is deprecated or removed. Prefer this
    /// name when writing spec-traced or cross-language code; either name is
    /// equally correct.
    pub fn raise_if_cancelled(&self) -> Result<(), ExecutionCancelledError> {
        self.check()
    }

    /// Check with an explicit `module_id`. Returns the typed error so
    /// callers can match on cancellation specifically before widening.
    pub fn check_for(&self, module_id: &str) -> Result<(), ExecutionCancelledError> {
        if self.is_cancelled() {
            Err(ExecutionCancelledError::new(
                module_id,
                "Execution was cancelled",
            ))
        } else {
            Ok(())
        }
    }

    /// Reset the cancellation flag.
    pub fn reset(&self) {
        self.cancelled.store(false, Ordering::SeqCst);
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}
