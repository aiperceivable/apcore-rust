// APCore Protocol — Pipeline YAML configuration: step type registry and strategy builder.
// Spec reference: design-execution-pipeline.md (Section 8)

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;

use crate::builtin_steps::build_standard_strategy;
use crate::errors::{ErrorCode, ModuleError};
use crate::pipeline::{ExecutionStrategy, Step};

// ---------------------------------------------------------------------------
// Step factory type
// ---------------------------------------------------------------------------

/// Factory function that creates a `Step` from a config dict.
pub(crate) type StepFactory =
    Box<dyn Fn(&Value) -> Result<Box<dyn Step>, ModuleError> + Send + Sync>;

// ---------------------------------------------------------------------------
// Global step type registry (OnceLock + RwLock pattern)
// ---------------------------------------------------------------------------

fn global_step_factories() -> &'static RwLock<HashMap<String, StepFactory>> {
    static FACTORIES: OnceLock<RwLock<HashMap<String, StepFactory>>> = OnceLock::new();
    FACTORIES.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Register a step type for YAML pipeline configuration.
///
/// The `name` is the type name referenced in YAML `type` fields.
/// The `factory` is a callable that receives `&Value` config and returns a `Box<dyn Step>`.
///
/// Returns an error if the name is empty, contains whitespace, or is already registered.
pub fn register_step_type(name: &str, factory: StepFactory) -> Result<(), ModuleError> {
    if name.is_empty() || name.contains(' ') {
        return Err(ModuleError::new(
            ErrorCode::GeneralInvalidInput,
            format!("Invalid step type name: '{name}'"),
        ));
    }
    let mut map = global_step_factories().write();
    if map.contains_key(name) {
        return Err(ModuleError::new(
            ErrorCode::GeneralInvalidInput,
            format!("Step type '{name}' is already registered"),
        ));
    }
    map.insert(name.to_string(), factory);
    Ok(())
}

/// Remove a registered step type. Returns `true` if found and removed.
pub fn unregister_step_type(name: &str) -> bool {
    let mut map = global_step_factories().write();
    map.remove(name).is_some()
}

/// Return a list of all registered step type names.
pub fn registered_step_types() -> Vec<String> {
    let map = global_step_factories().read();
    map.keys().cloned().collect()
}

/// Clear the step type registry (for testing only).
#[cfg(test)]
pub(crate) fn reset_step_registry() {
    let mut map = global_step_factories().write();
    map.clear();
}

// ---------------------------------------------------------------------------
// Step resolution from config dict
// ---------------------------------------------------------------------------

/// Resolve a single step definition dict into a `Box<dyn Step>`.
///
/// The dict must contain a `"type"` field whose value matches a registered step type.
fn resolve_step(step_def: &Value) -> Result<Box<dyn Step>, ModuleError> {
    let step_name = step_def.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let type_name = step_def.get("type").and_then(|v| v.as_str());
    let config = step_def
        .get("config")
        .cloned()
        .unwrap_or(Value::Object(serde_json::Map::new()));

    let type_name = type_name.ok_or_else(|| {
        ModuleError::new(
            ErrorCode::GeneralInvalidInput,
            format!("Step '{step_name}' has no 'type' field"),
        )
    })?;

    let map = global_step_factories().read();
    let factory = map.get(type_name).ok_or_else(|| {
        ModuleError::new(
            ErrorCode::GeneralInvalidInput,
            format!(
                "Step type '{type_name}' not registered. \
                 Register with: register_step_type(\"{type_name}\", factory)"
            ),
        )
    })?;

    factory(&config)
}

// ---------------------------------------------------------------------------
// Build strategy from YAML config
// ---------------------------------------------------------------------------

/// Build an `ExecutionStrategy` from a YAML pipeline configuration section.
///
/// Starts with `build_standard_strategy()`, then applies:
///   1. `remove` -- remove named steps
///   2. `steps` -- resolve and insert custom steps (via `after` or `before` anchors)
///
/// # Example config shape (as JSON)
///
/// ```json
/// {
///   "remove": ["acl_check", "approval_gate"],
///   "steps": [
///     {
///       "name": "rate_limit",
///       "type": "rate_limiter",
///       "after": "call_chain_guard",
///       "config": { "max_rps": 100 }
///     }
///   ]
/// }
/// ```
pub fn build_strategy_from_config(
    pipeline_config: &Value,
) -> Result<ExecutionStrategy, ModuleError> {
    let mut strategy = build_standard_strategy();

    // (1) Remove steps
    if let Some(remove_list) = pipeline_config.get("remove").and_then(|v| v.as_array()) {
        for entry in remove_list {
            if let Some(step_name) = entry.as_str() {
                // Log warning on failure but continue (matches Python behavior).
                if let Err(e) = strategy.remove(step_name) {
                    tracing::warn!(step = step_name, error = %e, "Cannot remove step");
                }
            }
        }
    }

    // (2) Resolve and insert custom steps
    if let Some(steps_list) = pipeline_config.get("steps").and_then(|v| v.as_array()) {
        for step_def in steps_list {
            let step = resolve_step(step_def)?;
            let after = step_def.get("after").and_then(|v| v.as_str());
            let before = step_def.get("before").and_then(|v| v.as_str());

            if let Some(anchor) = after {
                strategy.insert_after(anchor, step)?;
            } else if let Some(anchor) = before {
                strategy.insert_before(anchor, step)?;
            } else {
                let name = step_def
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                tracing::warn!(
                    step = name,
                    "Step has neither 'after' nor 'before' — skipping"
                );
            }
        }
    }

    Ok(strategy)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{PipelineContext, StepResult};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    /// Minimal configurable step for testing.
    struct ConfigurableStep {
        name: String,
        description: String,
    }

    impl ConfigurableStep {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                description: format!("Configurable step: {name}"),
            }
        }
    }

    #[async_trait]
    impl Step for ConfigurableStep {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            &self.description
        }
        fn removable(&self) -> bool {
            true
        }
        fn replaceable(&self) -> bool {
            true
        }
        fn pure(&self) -> bool {
            true
        }
        async fn execute(&self, _ctx: &mut PipelineContext) -> Result<StepResult, ModuleError> {
            Ok(StepResult::continue_step())
        }
    }

    // Serialize tests that mutate the global registry to avoid races.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_register_step_type_success() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset_step_registry();
        let result = register_step_type(
            "my_step",
            Box::new(|_config| Ok(Box::new(ConfigurableStep::new("my_step")))),
        );
        assert!(result.is_ok());
        assert!(registered_step_types().contains(&"my_step".to_string()));
    }

    #[test]
    fn test_register_step_type_empty_name_rejected() {
        let _guard = TEST_LOCK.lock().unwrap();
        let result = register_step_type("", Box::new(|_| Ok(Box::new(ConfigurableStep::new("x")))));
        assert!(result.is_err());
    }

    #[test]
    fn test_register_step_type_whitespace_name_rejected() {
        let _guard = TEST_LOCK.lock().unwrap();
        let result = register_step_type(
            "my step",
            Box::new(|_| Ok(Box::new(ConfigurableStep::new("x")))),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_register_step_type_duplicate_rejected() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset_step_registry();
        register_step_type(
            "dup_step",
            Box::new(|_| Ok(Box::new(ConfigurableStep::new("dup_step")))),
        )
        .unwrap();
        let result = register_step_type(
            "dup_step",
            Box::new(|_| Ok(Box::new(ConfigurableStep::new("dup_step")))),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_unregister_step_type_returns_true_if_found() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset_step_registry();
        register_step_type(
            "removable",
            Box::new(|_| Ok(Box::new(ConfigurableStep::new("removable")))),
        )
        .unwrap();
        assert!(unregister_step_type("removable"));
        assert!(!registered_step_types().contains(&"removable".to_string()));
    }

    #[test]
    fn test_unregister_step_type_returns_false_if_not_found() {
        let _guard = TEST_LOCK.lock().unwrap();
        assert!(!unregister_step_type("__nonexistent__"));
    }

    #[test]
    fn test_registered_step_types_after_reset() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset_step_registry();
        assert!(registered_step_types().is_empty());
    }

    #[test]
    fn test_build_strategy_from_config_remove_steps() {
        // Does not use the step type registry — no lock needed.
        let config = json!({
            "remove": ["acl_check", "approval_gate"]
        });
        let strategy = build_strategy_from_config(&config).unwrap();
        let names = strategy.step_names();
        assert!(!names.contains(&"acl_check".to_string()));
        assert!(!names.contains(&"approval_gate".to_string()));
        assert!(names.contains(&"module_lookup".to_string()));
        assert!(names.contains(&"execute".to_string()));
    }

    #[test]
    fn test_build_strategy_from_config_insert_custom_step() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset_step_registry();
        register_step_type(
            "rate_limiter",
            Box::new(|_config| Ok(Box::new(ConfigurableStep::new("rate_limit")))),
        )
        .unwrap();

        let config = json!({
            "steps": [{
                "name": "rate_limit",
                "type": "rate_limiter",
                "after": "call_chain_guard",
                "config": {}
            }]
        });
        let strategy = build_strategy_from_config(&config).unwrap();
        let names = strategy.step_names();
        let guard_idx = names.iter().position(|n| n == "call_chain_guard").unwrap();
        let rate_idx = names.iter().position(|n| n == "rate_limit").unwrap();
        assert_eq!(rate_idx, guard_idx + 1);
    }

    #[test]
    fn test_build_strategy_from_config_insert_before() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset_step_registry();
        register_step_type(
            "before_type",
            Box::new(|_config| Ok(Box::new(ConfigurableStep::new("custom_before")))),
        )
        .unwrap();

        let config = json!({
            "steps": [{
                "name": "custom_before",
                "type": "before_type",
                "before": "execute"
            }]
        });
        let strategy = build_strategy_from_config(&config).unwrap();
        let names = strategy.step_names();
        let custom_idx = names.iter().position(|n| n == "custom_before").unwrap();
        let exec_idx = names.iter().position(|n| n == "execute").unwrap();
        assert_eq!(custom_idx + 1, exec_idx);
    }

    #[test]
    fn test_build_strategy_from_config_unknown_type_returns_error() {
        let _guard = TEST_LOCK.lock().unwrap();
        reset_step_registry();
        let config = json!({
            "steps": [{
                "name": "bad",
                "type": "__nonexistent_type__",
                "after": "execute"
            }]
        });
        let result = build_strategy_from_config(&config);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("__nonexistent_type__"));
    }

    #[test]
    fn test_build_strategy_from_config_empty_config() {
        let config = json!({});
        let strategy = build_strategy_from_config(&config).unwrap();
        assert_eq!(strategy.step_names().len(), 11);
    }

    #[test]
    fn test_resolve_step_missing_type_returns_error() {
        let _guard = TEST_LOCK.lock().unwrap();
        let step_def = json!({"name": "no_type"});
        let result = resolve_step(&step_def);
        assert!(result.is_err());
    }
}
