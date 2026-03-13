// APCore Protocol — Cancellation tokens
// Spec reference: Cooperative cancellation for module execution

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Token used to signal cancellation to a running execution.
#[derive(Debug, Clone)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
}

impl CancelToken {
    /// Create a new cancel token in the non-cancelled state.
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
