// APCore Protocol — Cancellation tokens
// Spec reference: Cooperative cancellation for module execution

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Error raised when an execution is cancelled mid-flight.
///
/// Mirrors `apcore-python.ExecutionCancelledError(Exception)` and carries the
/// same two fields: `module_id` (the module that was running) and `message`
/// (a human-readable cancellation reason).
#[derive(Debug, thiserror::Error)]
#[error("ExecutionCancelledError: module '{module_id}' — {message}")]
pub struct ExecutionCancelledError {
    /// ID of the module whose execution was cancelled.
    pub module_id: String,
    /// Human-readable reason or description for the cancellation.
    pub message: String,
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

    /// Check if cancelled and return error if so.
    pub fn check(&self) -> Result<(), crate::errors::ModuleError> {
        if self.is_cancelled() {
            Err(crate::errors::ModuleError::new(
                crate::errors::ErrorCode::ExecutionCancelled,
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
