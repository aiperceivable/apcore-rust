// Issue #65 — Registry on_load ordering: deferred-publish pattern
// Tests that modules are NOT visible during on_load, concurrent same-ID
// registration is rejected, and failures emit load_failed callbacks.

use apcore::context::Context;
use apcore::errors::{ErrorCode, ModuleError};
use apcore::module::Module;
use apcore::registry::registry::{ModuleDescriptor, Registry};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn make_descriptor(id: &str) -> ModuleDescriptor {
    ModuleDescriptor {
        module_id: id.to_string(),
        name: None,
        description: String::new(),
        documentation: None,
        input_schema: json!({}),
        output_schema: json!({}),
        version: "1.0.0".to_string(),
        tags: vec![],
        annotations: None,
        examples: vec![],
        metadata: HashMap::new(),
        display: None,
        sunset_date: None,
        dependencies: vec![],
        enabled: true,
    }
}

// ---------------------------------------------------------------------------
// Module with configurable on_load delay and optional failure
// ---------------------------------------------------------------------------

struct DelayedModule {
    delay_ms: u64,
    fail: bool,
}

#[async_trait]
impl Module for DelayedModule {
    #[allow(clippy::unnecessary_literal_bound)]
    fn description(&self) -> &str {
        "delayed"
    }
    fn input_schema(&self) -> Value {
        json!({})
    }
    fn output_schema(&self) -> Value {
        json!({})
    }
    fn on_load(&self) -> Result<(), ModuleError> {
        if self.delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(self.delay_ms));
        }
        if self.fail {
            Err(ModuleError::new(
                ErrorCode::ModuleLoadError,
                "intentional on_load failure",
            ))
        } else {
            Ok(())
        }
    }
    async fn execute(&self, _: Value, _: &Context<Value>) -> Result<Value, ModuleError> {
        Ok(json!({}))
    }
}

// ---------------------------------------------------------------------------
// Module that checks its own visibility during on_load
// ---------------------------------------------------------------------------

struct VisibilityCheckModule {
    registry: Arc<Registry>,
    id: String,
    delay_ms: u64,
    visible_during_load: Arc<Mutex<Option<bool>>>,
}

#[async_trait]
impl Module for VisibilityCheckModule {
    #[allow(clippy::unnecessary_literal_bound)]
    fn description(&self) -> &str {
        "visibility-check"
    }
    fn input_schema(&self) -> Value {
        json!({})
    }
    fn output_schema(&self) -> Value {
        json!({})
    }
    fn on_load(&self) -> Result<(), ModuleError> {
        // Check whether the module is visible during on_load.
        let visible = self.registry.get(&self.id).unwrap_or(None).is_some();
        *self.visible_during_load.lock() = Some(visible);
        if self.delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(self.delay_ms));
        }
        Ok(())
    }
    async fn execute(&self, _: Value, _: &Context<Value>) -> Result<Value, ModuleError> {
        Ok(json!({}))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn successful_on_load_makes_module_visible_after_register() {
    let registry = Registry::new();
    let result = registry.register(
        "executor.test.success",
        Box::new(DelayedModule {
            delay_ms: 0,
            fail: false,
        }),
        make_descriptor("executor.test.success"),
    );
    assert!(result.is_ok());
    // After register() returns Ok, the module MUST be visible.
    assert!(
        registry.get("executor.test.success").unwrap().is_some(),
        "module must be visible after successful registration"
    );
    assert!(
        registry
            .list(None, None)
            .contains(&"executor.test.success".to_string()),
        "module must appear in list() after successful registration"
    );
}

#[test]
fn module_not_visible_during_on_load() {
    let registry = Arc::new(Registry::new());
    let visible_during_load = Arc::new(Mutex::new(None));
    let module = VisibilityCheckModule {
        registry: Arc::clone(&registry),
        id: "executor.test.check".to_string(),
        delay_ms: 0,
        visible_during_load: Arc::clone(&visible_during_load),
    };
    registry
        .register(
            "executor.test.check",
            Box::new(module),
            make_descriptor("executor.test.check"),
        )
        .unwrap();
    let was_visible = visible_during_load.lock().unwrap();
    assert!(
        !was_visible,
        "module must NOT be visible during on_load (deferred-publish invariant)"
    );
}

#[test]
fn failing_on_load_blocks_visibility_and_emits_callback() {
    let registry = Registry::new();
    let load_failed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let lf = Arc::clone(&load_failed);
    registry.on_load_failed(Arc::new(move |id, _err| {
        lf.lock().push(id.to_string());
    }));

    let err = registry
        .register(
            "executor.test.failing",
            Box::new(DelayedModule {
                delay_ms: 0,
                fail: true,
            }),
            make_descriptor("executor.test.failing"),
        )
        .unwrap_err();

    // Error propagated directly (not wrapped in ModuleLoadError unless on_load says so).
    assert_eq!(err.code, ErrorCode::ModuleLoadError);

    // Module must NOT be visible after a failed on_load.
    assert!(
        registry.get("executor.test.failing").unwrap().is_none(),
        "module must not be visible after failed on_load"
    );
    assert!(
        !registry
            .list(None, None)
            .contains(&"executor.test.failing".to_string()),
        "module must not appear in list() after failed on_load"
    );

    // on_load_failed callback was invoked.
    let failed_ids = load_failed.lock();
    assert_eq!(
        failed_ids.len(),
        1,
        "load_failed callback must be invoked once"
    );
    assert_eq!(
        failed_ids[0], "executor.test.failing",
        "load_failed callback receives correct module_id"
    );
}

#[test]
fn concurrent_same_id_one_ok_one_err() {
    let registry = Arc::new(Registry::new());
    let r1 = Arc::clone(&registry);
    let r2 = Arc::clone(&registry);

    let h1 = std::thread::spawn(move || {
        r1.register(
            "executor.test.concurrent",
            Box::new(DelayedModule {
                delay_ms: 20,
                fail: false,
            }),
            make_descriptor("executor.test.concurrent"),
        )
    });
    let h2 = std::thread::spawn(move || {
        r2.register(
            "executor.test.concurrent",
            Box::new(DelayedModule {
                delay_ms: 20,
                fail: false,
            }),
            make_descriptor("executor.test.concurrent"),
        )
    });

    let r1 = h1.join().unwrap();
    let r2 = h2.join().unwrap();

    let ok_count = [&r1, &r2].iter().filter(|r| r.is_ok()).count();
    let err_count = [&r1, &r2].iter().filter(|r| r.is_err()).count();
    assert_eq!(ok_count, 1, "exactly one registration must succeed");
    assert_eq!(err_count, 1, "exactly one registration must fail");

    // The failing one should be a duplicate error.
    let err_result = [r1, r2].into_iter().find(Result::is_err).unwrap();
    let err = err_result.unwrap_err();
    assert!(
        err.code == ErrorCode::DuplicateModuleId || err.code == ErrorCode::GeneralInvalidInput,
        "duplicate error code expected, got {:?}",
        err.code
    );

    // Exactly one module registered.
    assert_eq!(
        registry.count(),
        1,
        "registry must contain exactly one module after concurrent registration"
    );
}

#[test]
fn concurrent_distinct_ids_run_in_parallel() {
    let registry = Arc::new(Registry::new());
    let r1 = Arc::clone(&registry);
    let r2 = Arc::clone(&registry);

    let start = Instant::now();
    let h1 = std::thread::spawn(move || {
        r1.register(
            "executor.test.parallel_x",
            Box::new(DelayedModule {
                delay_ms: 50,
                fail: false,
            }),
            make_descriptor("executor.test.parallel_x"),
        )
    });
    let h2 = std::thread::spawn(move || {
        r2.register(
            "executor.test.parallel_y",
            Box::new(DelayedModule {
                delay_ms: 50,
                fail: false,
            }),
            make_descriptor("executor.test.parallel_y"),
        )
    });
    h1.join().unwrap().unwrap();
    h2.join().unwrap().unwrap();
    let elapsed = start.elapsed().as_millis();

    assert_eq!(registry.count(), 2, "both modules must be registered");
    assert!(
        elapsed < 90,
        "wall clock was {elapsed}ms; expected < 90ms (proves per-module parallelism — \
         distinct IDs must run on_load concurrently, not sequentially)"
    );
}

#[test]
fn load_failed_callback_receives_error_details() {
    let registry = Registry::new();
    let captured_err: Arc<Mutex<Option<ModuleError>>> = Arc::new(Mutex::new(None));
    let cap = Arc::clone(&captured_err);
    registry.on_load_failed(Arc::new(move |_id, err| {
        *cap.lock() = Some(err.clone());
    }));

    registry
        .register(
            "executor.test.err_details",
            Box::new(DelayedModule {
                delay_ms: 0,
                fail: true,
            }),
            make_descriptor("executor.test.err_details"),
        )
        .unwrap_err();

    let err = captured_err
        .lock()
        .clone()
        .expect("callback must have been called");
    assert_eq!(err.code, ErrorCode::ModuleLoadError);
    assert!(
        err.message.contains("on_load"),
        "error message should contain 'on_load'"
    );
}
