// APCore Protocol — Cancellation tokens
// Spec reference: Cooperative cancellation for module execution

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::errors::{ErrorCode, ModuleError};

/// Error raised when an execution is cancelled mid-flight.
///
/// Mirrors `apcore-python.ExecutionCancelledError(ModuleError)` and
/// `apcore-typescript ExecutionCancelledError extends ModuleError`. Carries
/// `module_id` (the module that was running) and `message` (a human-readable
/// cancellation reason).
#[derive(Debug, Clone, thiserror::Error)]
#[error("ExecutionCancelledError: module '{module_id}' — {message}")]
pub struct ExecutionCancelledError {
    /// ID of the module whose execution was cancelled.
    pub module_id: String,
    /// Human-readable reason or description for the cancellation.
    pub message: String,
}

impl ExecutionCancelledError {
    /// Build an `ExecutionCancelledError` with the given module ID and message.
    pub fn new(module_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            module_id: module_id.into(),
            message: message.into(),
        }
    }

    /// Convert into a generic [`ModuleError`] with code
    /// `ErrorCode::ExecutionCancelled`. Mirrors the `to_module_error()`
    /// helpers used by the other typed-error structs in `errors.rs`.
    #[must_use]
    pub fn to_module_error(&self) -> ModuleError {
        let mut err = ModuleError::new(ErrorCode::ExecutionCancelled, &self.message);
        err.details.insert(
            "module_id".to_string(),
            serde_json::Value::String(self.module_id.clone()),
        );
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
            Err(ExecutionCancelledError::new(
                "@unknown",
                "Execution was cancelled",
            ))
        } else {
            Ok(())
        }
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
