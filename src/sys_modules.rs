// APCore Protocol — System modules registration
// Spec reference: Built-in system modules

use std::collections::HashMap;

use crate::config::Config;
use crate::errors::ModuleError;
use crate::executor::Executor;
use crate::observability::metrics::MetricsCollector;
use crate::registry::registry::Registry;

/// Register built-in system modules (e.g. health check, introspection) into the registry and executor.
/// Returns a map of module_id → module descriptor info.
/// `metrics_collector` is optional; if provided, metrics modules are wired up.
pub fn register_sys_modules(
    registry: &mut Registry,
    executor: &mut Executor,
    config: &Config,
    metrics_collector: Option<MetricsCollector>,
) -> HashMap<String, serde_json::Value> {
    // TODO: Implement — register __health, __list_modules, __describe, etc.
    todo!()
}
