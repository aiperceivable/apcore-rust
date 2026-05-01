// APCore Protocol — Events module
// Spec reference: Event emission and subscription

pub mod circuit_breaker;
pub mod emitter;
pub mod subscribers;

pub use circuit_breaker::{
    CircuitBreakerWrapper, CircuitEventSink, CircuitState, DEFAULT_OPEN_THRESHOLD,
    DEFAULT_RECOVERY_WINDOW_MS, DEFAULT_TIMEOUT_MS,
};
pub use emitter::{ApCoreEvent, EventEmitter};
pub use subscribers::{
    EventSubscriber, FileSubscriber, FilterSubscriber, OutputFormat, StdoutSubscriber,
    WebhookSubscriber,
};
