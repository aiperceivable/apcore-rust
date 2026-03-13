// APCore Protocol — Middleware module
// Spec reference: Middleware pipeline

pub mod adapters;
pub mod base;
pub mod manager;
pub mod retry;

pub use adapters::{AfterMiddleware, BeforeMiddleware};
pub use base::Middleware;
pub use manager::MiddlewareManager;
pub use retry::{RetryConfig, RetryMiddleware};

/// Platform notification middleware stub.
#[derive(Debug)]
pub struct PlatformNotifyMiddleware {
    // TODO: event_emitter, metrics_collector, thresholds
}

impl PlatformNotifyMiddleware {
    pub fn new() -> Self {
        todo!("PlatformNotifyMiddleware::new()")
    }
}
