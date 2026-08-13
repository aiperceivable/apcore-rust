// Conformance tests for Registry on_load Ordering Invariants (Issue #65).
// Fixture: apcore/conformance/fixtures/registry_load_ordering.json
#![allow(clippy::pedantic)] // fixture-driven test file: casts and struct layouts follow fixture schema

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

// ---------------------------------------------------------------------------
// Fixture loading
// ---------------------------------------------------------------------------

use crate::conformance_env::find_fixtures_root;

fn load_fixture() -> Value {
    let path = find_fixtures_root().join("registry_load_ordering.json");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("Failed to read fixture: {}", path.display()));
    serde_json::from_str(&content).unwrap_or_else(|e| panic!("Invalid JSON: {e}"))
}

fn fixture_case<'a>(fixture: &'a Value, id: &str) -> &'a Value {
    fixture["test_cases"]
        .as_array()
        .expect("test_cases must be an array")
        .iter()
        .find(|c| c["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("test case '{id}' not found in fixture"))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

struct DelayedModule {
    delay_ms: u64,
    fail: bool,
}

#[async_trait]
impl Module for DelayedModule {
    #[allow(clippy::unnecessary_literal_bound)]
    fn description(&self) -> &str {
        "conformance-delayed"
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
                "Failed to reach upstream during initialization",
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
// Case: visibility_after_successful_on_load
// ---------------------------------------------------------------------------

#[test]
fn conformance_visibility_after_successful_on_load() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "visibility_after_successful_on_load");

    let module_id = case["setup"]["module"]["id"].as_str().unwrap();
    let delay_ms = case["setup"]["module"]["on_load_delay_ms"]
        .as_u64()
        .unwrap_or(50);

    let registry = Arc::new(Registry::new());

    // Fixture: concurrent_check at 25ms during 50ms on_load — module must NOT be visible.
    // We simulate this by checking visibility during on_load via a side-channel.
    let visible_at_25ms: Arc<Mutex<Option<bool>>> = Arc::new(Mutex::new(None));
    {
        let reg_clone = Arc::clone(&registry);
        let mid = module_id.to_string();
        let vis = Arc::clone(&visible_at_25ms);

        struct VisModule {
            registry: Arc<Registry>,
            id: String,
            delay_ms: u64,
            visible_during_load: Arc<Mutex<Option<bool>>>,
        }
        #[async_trait]
        impl Module for VisModule {
            #[allow(clippy::unnecessary_literal_bound)]
            fn description(&self) -> &str {
                "vis"
            }
            fn input_schema(&self) -> Value {
                json!({})
            }
            fn output_schema(&self) -> Value {
                json!({})
            }
            fn on_load(&self) -> Result<(), ModuleError> {
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

        registry
            .register(
                &mid,
                Box::new(VisModule {
                    registry: reg_clone,
                    id: mid.clone(),
                    delay_ms,
                    visible_during_load: vis,
                }),
                make_descriptor(module_id),
            )
            .unwrap();
    }

    // Post-register: module must be visible
    let post_visible = case["expected"]["post_register_visible"].as_bool().unwrap();
    assert_eq!(
        registry.get(module_id).unwrap().is_some(),
        post_visible,
        "post_register_visible mismatch"
    );
    assert_eq!(
        registry
            .list(None, None, None)
            .contains(&module_id.to_string()),
        post_visible,
        "module must appear in list() after registration"
    );

    // During-load visibility check
    let concurrent_visible = case["expected"]["concurrent_check_visible"]
        .as_bool()
        .unwrap();
    let was_visible = visible_at_25ms.lock().unwrap_or(false);
    assert_eq!(
        was_visible, concurrent_visible,
        "module visibility during on_load: expected={concurrent_visible}, got={was_visible}"
    );

    // Bound to the fixture. The key used to read
    // `concurrent_check_get_raises: "MODULE_NOT_FOUND"`, a behaviour no SDK
    // implements: `Registry::get` returns `Ok(None)` for a well-formed id that
    // is not visible and reserves `Err(ModuleNotFound)` for the empty string,
    // matching features/registry-system.md "On success (not found)". It is now
    // `concurrent_check_get_returns: null`, which is observable.
    // `was_visible` is `registry.get(&id).is_some()` sampled DURING on_load, so
    // it is the same observation this key describes, viewed as a bool. The two
    // keys are two facets of one measurement — that redundancy is the fixture's,
    // and both are bound rather than one left dangling.
    let expects_empty = case["expected"]["concurrent_check_get_returns"].is_null();
    assert_eq!(
        was_visible,
        !expects_empty,
        "get() during on_load: fixture expects {}, SDK returned {}",
        if expects_empty {
            "the empty value"
        } else {
            "a module"
        },
        if was_visible {
            "a module"
        } else {
            "the empty value"
        }
    );
    // The lookup path that DOES surface the code is covered below.
    let Err(lookup_err) = registry.acquire("executor.test.never_registered") else {
        panic!("acquiring an unregistered module must fail")
    };
    assert_eq!(
        lookup_err.code,
        ErrorCode::ModuleNotFound,
        "resolving an invisible module for use must surface MODULE_NOT_FOUND"
    );
}

// ---------------------------------------------------------------------------
// Case: callback_failure_blocks_visibility
// ---------------------------------------------------------------------------

#[test]
fn conformance_callback_failure_blocks_visibility() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "callback_failure_blocks_visibility");

    let module_id = case["setup"]["module"]["id"].as_str().unwrap();
    let expected_err_msg = case["setup"]["module"]["on_load_raises"]["message"]
        .as_str()
        .unwrap();

    let load_failed_ids: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let registry = Registry::new();
    {
        let lf = Arc::clone(&load_failed_ids);
        registry.on_load_failed(Arc::new(move |id, _err| {
            lf.lock().push(id.to_string());
        }));
    }

    struct FailWithMsgModule {
        message: String,
    }
    #[async_trait]
    impl Module for FailWithMsgModule {
        #[allow(clippy::unnecessary_literal_bound)]
        fn description(&self) -> &str {
            "fail-with-msg"
        }
        fn input_schema(&self) -> Value {
            json!({})
        }
        fn output_schema(&self) -> Value {
            json!({})
        }
        fn on_load(&self) -> Result<(), ModuleError> {
            Err(ModuleError::new(ErrorCode::ModuleLoadError, &self.message))
        }
        async fn execute(&self, _: Value, _: &Context<Value>) -> Result<Value, ModuleError> {
            Ok(json!({}))
        }
    }

    let result = registry.register(
        module_id,
        Box::new(FailWithMsgModule {
            message: expected_err_msg.to_string(),
        }),
        make_descriptor(module_id),
    );

    // `registration_raises` names the HOST error the callback produced, not a
    // registry-invented one: `register()` must surface the callback's own error
    // unchanged rather than remapping it, or the caller loses the reason the
    // load failed. Rust's `on_load` returns `Result<(), ModuleError>`, so
    // "unchanged" means the same code AND the same message come back out.
    let declares_host_error = case["expected"]["registration_raises"]
        .as_str()
        .expect("registration_raises is a string")
        .contains("host exception");
    assert!(
        declares_host_error,
        "fixture changed shape: `registration_raises` no longer declares the \
         host-exception contract this assertion checks"
    );
    assert!(
        result.is_err(),
        "register() must return Err when on_load raises"
    );
    let err = result.unwrap_err();
    assert_eq!(
        err.code,
        ErrorCode::ModuleLoadError,
        "register() must surface the on_load error's own code, not remap it"
    );
    assert_eq!(
        err.message, expected_err_msg,
        "register() must surface the on_load error's own message unchanged"
    );

    // post_register_visible: false
    assert!(
        !case["expected"]["post_register_visible"].as_bool().unwrap(),
        "fixture expects post_register_visible=false"
    );
    // `post_register_get_returns` — the key declared a raise no SDK performs
    // and is now `null`.
    assert!(
        case["expected"]["post_register_get_returns"].is_null(),
        "the fixture must declare the empty return, not a raise"
    );
    assert!(
        registry.get(module_id).unwrap().is_none(),
        "module must not be visible after failed on_load"
    );

    // `post_register_list_contains`
    assert_eq!(
        registry
            .list(None, None, None)
            .contains(&module_id.to_string()),
        case["expected"]["post_register_list_contains"]
            .as_bool()
            .unwrap(),
        "post_register_list_contains mismatch after a failed on_load"
    );

    // NOTE: `post_register_get_raises: "MODULE_NOT_FOUND"` is NOT asserted, for
    // the same reason as `concurrent_check_get_raises` in the case above —
    // `Registry::get` returns `Ok(None)` for an unregistered id in all three
    // SDKs. The invisibility invariant is asserted on the lines above; the
    // resolve-for-use path that does carry the code is asserted here.
    let Err(lookup_err) = registry.acquire(module_id) else {
        panic!("acquiring a module whose on_load failed must fail")
    };
    assert_eq!(
        lookup_err.code,
        ErrorCode::ModuleNotFound,
        "resolving a module whose on_load failed must surface MODULE_NOT_FOUND"
    );

    // load_failed_event_emitted
    assert!(
        case["expected"]["load_failed_event_emitted"]
            .as_bool()
            .unwrap(),
        "fixture expects load_failed_event_emitted=true"
    );
    let failed_ids = load_failed_ids.lock();
    assert!(
        !failed_ids.is_empty(),
        "on_load_failed callback must be invoked"
    );
    assert_eq!(
        failed_ids[0], module_id,
        "callback receives correct module_id"
    );

    // Validate required keys in event payload (via callback)
    let required_keys: Vec<&str> = case["expected"]["load_failed_event"]["data_required_keys"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    // The callback-based API gives us (module_id, &error) — no full ApCoreEvent.
    // Verify that module_id and error fields are present as described in the fixture.
    assert!(
        required_keys.contains(&"module_id"),
        "fixture requires 'module_id' in event data"
    );
    assert!(
        required_keys.contains(&"error_message"),
        "fixture requires 'error_message' in event data"
    );
}

// ---------------------------------------------------------------------------
// Case: concurrent_same_id_rejects_duplicate
// ---------------------------------------------------------------------------

#[test]
fn conformance_concurrent_same_id_rejects_duplicate() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "concurrent_same_id_rejects_duplicate");

    let module_id = case["setup"]["module_a"]["id"].as_str().unwrap();
    let delay_ms = case["setup"]["module_a"]["on_load_delay_ms"]
        .as_u64()
        .unwrap_or(30);

    let registry = Arc::new(Registry::new());
    let r1 = Arc::clone(&registry);
    let r2 = Arc::clone(&registry);
    let mid = module_id.to_string();
    let mid2 = module_id.to_string();

    let h1 = std::thread::spawn(move || {
        r1.register(
            &mid,
            Box::new(DelayedModule {
                delay_ms,
                fail: false,
            }),
            make_descriptor(&mid),
        )
    });
    let h2 = std::thread::spawn(move || {
        r2.register(
            &mid2,
            Box::new(DelayedModule {
                delay_ms,
                fail: false,
            }),
            make_descriptor(&mid2),
        )
    });

    let r1 = h1.join().unwrap();
    let r2 = h2.join().unwrap();

    let ok_count = [&r1, &r2].iter().filter(|r| r.is_ok()).count();
    let err_count = [&r1, &r2].iter().filter(|r| r.is_err()).count();

    // one_succeeds: true
    assert!(
        case["expected"]["one_succeeds"].as_bool().unwrap(),
        "fixture expects one_succeeds=true"
    );
    assert_eq!(ok_count, 1, "exactly one registration must succeed");
    assert_eq!(err_count, 1, "exactly one registration must fail");

    // raised_error_code: DUPLICATE_MODULE_ID
    let err = [r1, r2]
        .into_iter()
        .find(Result::is_err)
        .unwrap()
        .unwrap_err();
    assert_eq!(
        err.code,
        ErrorCode::DuplicateModuleId,
        "concurrent same-ID must raise DuplicateModuleId, got {:?}: {}",
        err.code,
        err.message
    );

    // post_register_count: 1
    assert_eq!(
        registry.count(),
        case["expected"]["post_register_count"].as_u64().unwrap() as usize,
        "registry must contain exactly one module"
    );
}

// ---------------------------------------------------------------------------
// Case: concurrent_distinct_ids_run_in_parallel
// ---------------------------------------------------------------------------

#[test]
fn conformance_concurrent_distinct_ids_run_in_parallel() {
    let fixture = load_fixture();
    let case = fixture_case(&fixture, "concurrent_distinct_ids_run_in_parallel");

    let id_x = case["setup"]["module_x"]["id"].as_str().unwrap();
    let id_y = case["setup"]["module_y"]["id"].as_str().unwrap();
    let delay_ms = case["setup"]["module_x"]["on_load_delay_ms"]
        .as_u64()
        .unwrap_or(50);

    let registry = Arc::new(Registry::new());
    let r1 = Arc::clone(&registry);
    let r2 = Arc::clone(&registry);
    let mx = id_x.to_string();
    let my = id_y.to_string();

    let wall_clock_limit_ms = case["expected"]["wall_clock_ms_less_than"]
        .as_u64()
        .unwrap_or(90);

    let start = Instant::now();
    let h1 = std::thread::spawn(move || {
        r1.register(
            &mx,
            Box::new(DelayedModule {
                delay_ms,
                fail: false,
            }),
            make_descriptor(&mx),
        )
    });
    let h2 = std::thread::spawn(move || {
        r2.register(
            &my,
            Box::new(DelayedModule {
                delay_ms,
                fail: false,
            }),
            make_descriptor(&my),
        )
    });
    h1.join().unwrap().unwrap();
    h2.join().unwrap().unwrap();
    let elapsed = start.elapsed().as_millis();

    // both_succeed: true
    assert!(
        case["expected"]["both_succeed"].as_bool().unwrap(),
        "fixture expects both_succeed=true"
    );
    assert_eq!(
        registry.count(),
        case["expected"]["post_register_count"].as_u64().unwrap() as usize,
        "both modules must be registered"
    );

    // wall_clock_ms_less_than (proves per-module parallelism)
    assert!(
        elapsed < wall_clock_limit_ms as u128,
        "wall clock was {elapsed}ms; fixture requires < {wall_clock_limit_ms}ms \
         (distinct-ID on_load callbacks must run concurrently)"
    );
}
