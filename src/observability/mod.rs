// APCore Protocol — Observability module
// Spec reference: Tracing, metrics, logging, error history, and usage

pub mod error_history;
pub mod exporters;
pub mod logging;
pub mod metrics;
pub mod span;
pub mod tracing_middleware;
pub mod usage;

pub use error_history::{ErrorEntry, ErrorHistory, ErrorHistoryMiddleware};
pub use exporters::{InMemoryExporter, OTLPExporter, StdoutExporter};
pub use logging::{ContextLogger, ObsLoggingMiddleware};
pub use metrics::{MetricsCollector, MetricsMiddleware};
pub use span::{Span, SpanExporter};
pub use tracing_middleware::{SamplingStrategy, TracingMiddleware};
pub use usage::{UsageCollector, UsageMiddleware, UsageStats};
