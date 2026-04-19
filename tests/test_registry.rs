//! Tests for Registry — creation, read-only operations, and new methods.

use apcore::context::{Context, Identity};
use apcore::errors::ModuleError;
use apcore::module::{Module, ModuleAnnotations};
use apcore::registry::registry::{ModuleDescriptor, Registry};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct StubModule;

#[async_trait]
impl Module for StubModule {
    fn description(&self) -> &'static str {
        "stub"
    }
    fn input_schema(&self) -> Value {
        serde_json::json!({ "type": "object" })
    }
    fn output_schema(&self) -> Value {
        serde_json::json!({ "type": "object" })
    }
    async fn execute(&self, _inputs: Value, _ctx: &Context<Value>) -> Result<Value, ModuleError> {
        Ok(serde_json::json!({}))
    }
}

fn make_descriptor(name: &str) -> ModuleDescriptor {
    ModuleDescriptor {
        module_id: name.to_string(),
        name: None,
        description: String::new(),
        documentation: None,
        input_schema: serde_json::json!({ "type": "object" }),
        output_schema: serde_json::json!({ "type": "object" }),
        version: "1.0.0".to_string(),
        tags: vec![],
        annotations: Some(ModuleAnnotations::default()),
        examples: vec![],
        metadata: std::collections::HashMap::new(),
        sunset_date: None,
        dependencies: vec![],
        enabled: true,
    }
}

fn dummy_identity() -> Identity {
    Identity::new(
        "@test".to_string(),
        "test".to_string(),
        vec![],
        HashMap::default(),
    )
}

// ---------------------------------------------------------------------------
// Empty-registry read tests
// ---------------------------------------------------------------------------

#[test]
fn test_registry_new_is_empty() {
    let registry = Registry::new();
    assert!(registry.list(None, None).is_empty());
}

#[test]
fn test_registry_default_is_empty() {
    let registry = Registry::default();
    assert!(registry.list(None, None).is_empty());
}

#[test]
fn test_registry_get_unknown_module_returns_none() {
    let registry = Registry::new();
    assert!(registry.get("nonexistent").is_none());
}

#[test]
fn test_registry_contains_unknown_module_returns_false() {
    let registry = Registry::new();
    assert!(!registry.has("nonexistent"));
}

#[test]
fn test_registry_get_definition_unknown_returns_none() {
    let registry = Registry::new();
    assert!(registry.get_definition("nonexistent").is_none());
}

#[test]
fn test_registry_list_returns_vec_of_str() {
    let registry = Registry::new();
    let list: Vec<String> = registry.list(None, None);
    assert!(list.is_empty());
}

// ---------------------------------------------------------------------------
// export_schema tests (C-3)
// ---------------------------------------------------------------------------

#[test]
fn test_export_schema_returns_none_for_unregistered_module() {
    let registry = Registry::new();
    assert!(registry.export_schema("not.registered").is_none());
}

#[test]
fn test_export_schema_returns_schema_after_registration() {
    let registry = Registry::new();
    let descriptor = make_descriptor("math.add");
    registry
        .register_internal("math.add", Box::new(StubModule), descriptor)
        .expect("registration should succeed");

    let schema = registry.export_schema("math.add");
    assert!(
        schema.is_some(),
        "schema should be cached after registration"
    );
    let s = schema.unwrap();
    assert!(s.get("input").is_some(), "schema should have 'input' key");
    assert!(s.get("output").is_some(), "schema should have 'output' key");
}

// ---------------------------------------------------------------------------
// disable / enable / is_enabled tests (C-3)
// ---------------------------------------------------------------------------

#[test]
fn test_is_enabled_returns_none_for_unregistered_module() {
    let registry = Registry::new();
    assert!(registry.is_enabled("not.registered").is_none());
}

#[test]
fn test_disable_returns_error_for_unregistered_module() {
    let registry = Registry::new();
    let err = registry
        .disable("not.registered")
        .expect_err("should fail for unregistered module");
    assert!(
        err.message.contains("not found"),
        "error message should mention 'not found'"
    );
}

#[test]
fn test_enable_returns_error_for_unregistered_module() {
    let registry = Registry::new();
    let err = registry
        .enable("not.registered")
        .expect_err("should fail for unregistered module");
    assert!(err.message.contains("not found"));
}

#[test]
fn test_disable_sets_enabled_to_false() {
    let registry = Registry::new();
    registry
        .register_internal(
            "email.send",
            Box::new(StubModule),
            make_descriptor("email.send"),
        )
        .expect("registration should succeed");

    assert_eq!(registry.is_enabled("email.send"), Some(true));

    registry
        .disable("email.send")
        .expect("disable should succeed");
    assert_eq!(registry.is_enabled("email.send"), Some(false));
}

#[test]
fn test_enable_restores_enabled_to_true() {
    let registry = Registry::new();
    registry
        .register_internal("greet", Box::new(StubModule), make_descriptor("greet"))
        .expect("registration should succeed");

    registry.disable("greet").expect("disable should succeed");
    assert_eq!(registry.is_enabled("greet"), Some(false));

    registry.enable("greet").expect("enable should succeed");
    assert_eq!(registry.is_enabled("greet"), Some(true));
}

#[test]
fn test_module_enabled_by_default_after_registration() {
    let registry = Registry::new();
    registry
        .register_internal(
            "util.noop",
            Box::new(StubModule),
            make_descriptor("util.noop"),
        )
        .expect("registration should succeed");

    assert_eq!(
        registry.is_enabled("util.noop"),
        Some(true),
        "newly registered module should be enabled"
    );
}

// ---------------------------------------------------------------------------
// Reserved word validation tests
// ---------------------------------------------------------------------------

#[test]
fn test_register_rejects_reserved_first_segment() {
    let registry = Registry::new();
    let result = registry.register(
        "system.health",
        Box::new(StubModule),
        make_descriptor("system.health"),
    );
    assert!(result.is_err(), "registering 'system.health' should fail");
    let err = result.unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("reserved word"),
        "error should mention reserved word, got: {msg}"
    );
}

#[test]
fn test_register_allows_reserved_word_in_middle_segment() {
    // PROTOCOL_SPEC §2.7: reserved words are only checked against the first
    // segment. Middle/last segments may contain reserved words.
    // Aligned with apcore-python and apcore-typescript.
    let registry = Registry::new();
    let result = registry.register(
        "email.system",
        Box::new(StubModule),
        make_descriptor("email.system"),
    );
    assert!(
        result.is_ok(),
        "registering 'email.system' should succeed — 'system' is not the first segment"
    );
}

#[test]
fn test_register_allows_normal_module_id() {
    let registry = Registry::new();
    let result = registry.register(
        "email.send",
        Box::new(StubModule),
        make_descriptor("email.send"),
    );
    assert!(result.is_ok(), "registering 'email.send' should succeed");
}

#[test]
fn test_register_rejects_all_reserved_words() {
    use apcore::registry::RESERVED_WORDS;
    for word in RESERVED_WORDS {
        let registry = Registry::new();
        let module_id = format!("{word}.something");
        let result = registry.register(
            &module_id,
            Box::new(StubModule),
            make_descriptor(&module_id),
        );
        assert!(
            result.is_err(),
            "registering '{module_id}' should fail — '{word}' is reserved"
        );
    }
}

#[test]
fn test_register_module_rejects_reserved_first_segment() {
    let registry = Registry::new();
    let result = registry.register_module("core.utils", Box::new(StubModule));
    assert!(
        result.is_err(),
        "register_module with 'core.utils' should fail"
    );
}

// ---------------------------------------------------------------------------
// Module ID length boundary tests (PROTOCOL_SPEC §2.7 EBNF constraint #1)
// ---------------------------------------------------------------------------

#[test]
fn test_max_module_id_length_matches_spec() {
    // Per PROTOCOL_SPEC §2.7. Bumped from 128 to 192 in spec 1.6.0-draft.
    // Filesystem-safe: 192 + ".binding.yaml".len()=13 = 205 < 255-byte filename limit.
    use apcore::registry::MAX_MODULE_ID_LENGTH;
    assert_eq!(MAX_MODULE_ID_LENGTH, 192);
}

#[test]
fn test_register_accepts_module_id_at_max_length() {
    use apcore::registry::MAX_MODULE_ID_LENGTH;
    let registry = Registry::new();
    // Pure 'a' run satisfies the EBNF pattern [a-z][a-z0-9_]*.
    let exact_id = "a".repeat(MAX_MODULE_ID_LENGTH);
    let result = registry.register(&exact_id, Box::new(StubModule), make_descriptor(&exact_id));
    assert!(
        result.is_ok(),
        "registering an ID at exactly MAX_MODULE_ID_LENGTH should succeed"
    );
}

#[test]
fn test_register_rejects_module_id_exceeding_max_length() {
    use apcore::registry::MAX_MODULE_ID_LENGTH;
    let registry = Registry::new();
    let overlong_id = "a".repeat(MAX_MODULE_ID_LENGTH + 1);
    let result = registry.register(
        &overlong_id,
        Box::new(StubModule),
        make_descriptor(&overlong_id),
    );
    assert!(
        result.is_err(),
        "registering an ID longer than MAX_MODULE_ID_LENGTH should fail"
    );
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("maximum length"),
        "error should mention maximum length, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// PROTOCOL_SPEC §2.7 EBNF compliance — empty / pattern checks
// (parity with apcore-python and apcore-typescript)
// ---------------------------------------------------------------------------

#[test]
fn test_register_rejects_empty_module_id() {
    let registry = Registry::new();
    let result = registry.register("", Box::new(StubModule), make_descriptor(""));
    assert!(result.is_err(), "registering empty ID must fail");
    let msg = format!("{}", result.unwrap_err());
    assert!(
        msg.contains("non-empty"),
        "error should mention non-empty, got: {msg}"
    );
}

#[test]
fn test_register_rejects_invalid_pattern() {
    let registry = Registry::new();
    for bad_id in [
        "INVALID-ID", // hyphens not allowed
        "1abc",       // starts with digit
        "Module",     // uppercase
        "a..b",       // consecutive dots
        ".leading",   // leading dot
        "trailing.",  // trailing dot
        "has space",  // space
        "has!bang",   // special char
    ] {
        let result = registry.register(bad_id, Box::new(StubModule), make_descriptor(bad_id));
        assert!(
            result.is_err(),
            "registering pattern-invalid ID '{bad_id}' must fail"
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("Invalid module ID") || msg.contains("Must match pattern"),
            "error for '{bad_id}' should mention pattern, got: {msg}"
        );
    }
}

// ---------------------------------------------------------------------------
// register_internal — bypasses ONLY reserved word check
// (parity with apcore-python and apcore-typescript)
// ---------------------------------------------------------------------------

#[test]
fn test_register_internal_accepts_reserved_first_segment() {
    let registry = Registry::new();
    let result = registry.register_internal(
        "system.health",
        Box::new(StubModule),
        make_descriptor("system.health"),
    );
    assert!(
        result.is_ok(),
        "register_internal must accept reserved first segment 'system'"
    );
}

#[test]
fn test_register_internal_accepts_reserved_any_segment() {
    let registry = Registry::new();
    let result = registry.register_internal(
        "myapp.system.config",
        Box::new(StubModule),
        make_descriptor("myapp.system.config"),
    );
    assert!(
        result.is_ok(),
        "register_internal must accept reserved word in any segment"
    );
}

#[test]
fn test_register_internal_still_rejects_empty() {
    let registry = Registry::new();
    let result = registry.register_internal("", Box::new(StubModule), make_descriptor(""));
    assert!(
        result.is_err(),
        "register_internal must still reject empty IDs"
    );
}

#[test]
fn test_register_internal_still_rejects_invalid_pattern() {
    let registry = Registry::new();
    let result = registry.register_internal(
        "INVALID-ID",
        Box::new(StubModule),
        make_descriptor("INVALID-ID"),
    );
    assert!(
        result.is_err(),
        "register_internal must still enforce EBNF pattern"
    );
}

#[test]
fn test_register_internal_still_rejects_over_length() {
    use apcore::registry::MAX_MODULE_ID_LENGTH;
    let registry = Registry::new();
    let overlong = "a".repeat(MAX_MODULE_ID_LENGTH + 1);
    let result =
        registry.register_internal(&overlong, Box::new(StubModule), make_descriptor(&overlong));
    assert!(
        result.is_err(),
        "register_internal must still enforce length limit"
    );
}

#[test]
fn test_register_internal_rejects_duplicate() {
    let registry = Registry::new();
    registry
        .register_internal(
            "system.dup",
            Box::new(StubModule),
            make_descriptor("system.dup"),
        )
        .expect("first register_internal should succeed");
    let result = registry.register_internal(
        "system.dup",
        Box::new(StubModule),
        make_descriptor("system.dup"),
    );
    assert!(
        result.is_err(),
        "register_internal must reject duplicate IDs"
    );
}

// Suppress unused-import warning — dummy_identity is available for future async tests.
#[allow(dead_code)]
fn _use_identity() -> Identity {
    dummy_identity()
}

#[test]
fn test_on_returns_unique_handles() {
    let registry = Registry::new();

    let h1 = registry.on(
        "register",
        Box::new(|_: &str, _: &dyn apcore::module::Module| {}),
    );
    let h2 = registry.on(
        "register",
        Box::new(|_: &str, _: &dyn apcore::module::Module| {}),
    );

    assert_ne!(h1, h2, "each on() call must return a distinct handle");
}

#[test]
fn test_off_removes_callback_by_handle() {
    use std::sync::{Arc, Mutex};
    let registry = Registry::new();
    let counter = Arc::new(Mutex::new(0u32));

    let c = counter.clone();
    let handle = registry.on(
        "register",
        Box::new(move |_: &str, _: &dyn apcore::module::Module| {
            *c.lock().unwrap() += 1;
        }),
    );

    // Register a module to trigger the callback once
    registry
        .register_module("math.add", Box::new(StubModule))
        .unwrap();
    assert_eq!(*counter.lock().unwrap(), 1, "callback should fire once");

    // Remove the callback
    let removed = registry.off(handle);
    assert!(removed, "off() should return true when callback exists");

    // Register another module — callback should NOT fire again
    registry
        .register_module("math.sub", Box::new(StubModule))
        .unwrap();
    assert_eq!(
        *counter.lock().unwrap(),
        1,
        "callback should not fire after off()"
    );
}

#[test]
fn test_off_returns_false_for_unknown_handle() {
    let registry = Registry::new();
    let removed = registry.off(99999);
    assert!(!removed, "off() with unknown handle should return false");
}

// ---------------------------------------------------------------------------
// Discoverer — cross-language parity tests
// ---------------------------------------------------------------------------

mod discoverer_tests {
    use super::*;
    use apcore::module::ValidationResult;
    use apcore::registry::registry::{DiscoveredModule, Discoverer, ModuleValidator};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };

    struct OnLoadCountingModule {
        counter: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Module for OnLoadCountingModule {
        fn description(&self) -> &'static str {
            "on_load counter"
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({ "type": "object" })
        }
        fn output_schema(&self) -> Value {
            serde_json::json!({ "type": "object" })
        }
        fn on_load(&self) -> Result<(), ModuleError> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn execute(
            &self,
            _inputs: Value,
            _ctx: &Context<Value>,
        ) -> Result<Value, ModuleError> {
            Ok(serde_json::json!({}))
        }
    }

    struct FixedDiscoverer {
        entries: Mutex<Option<Vec<DiscoveredModule>>>,
    }

    impl FixedDiscoverer {
        fn new(entries: Vec<DiscoveredModule>) -> Self {
            Self {
                entries: Mutex::new(Some(entries)),
            }
        }
    }

    #[async_trait]
    impl Discoverer for FixedDiscoverer {
        async fn discover(&self, _roots: &[String]) -> Result<Vec<DiscoveredModule>, ModuleError> {
            Ok(self.entries.lock().unwrap().take().unwrap_or_default())
        }
    }

    struct RejectAllValidator {
        called: Arc<AtomicUsize>,
    }

    impl ModuleValidator for RejectAllValidator {
        fn validate(
            &self,
            _module: &dyn Module,
            _descriptor: Option<&ModuleDescriptor>,
        ) -> ValidationResult {
            self.called.fetch_add(1, Ordering::SeqCst);
            ValidationResult {
                valid: false,
                errors: vec!["rejected by test validator".to_string()],
                warnings: vec![],
            }
        }
    }

    fn dm(name: &str, module: Arc<dyn Module>) -> DiscoveredModule {
        DiscoveredModule {
            name: name.to_string(),
            source: "test".to_string(),
            descriptor: make_descriptor(name),
            module,
        }
    }

    fn stub() -> Arc<dyn Module> {
        Arc::new(StubModule)
    }

    #[tokio::test]
    async fn registers_instance_and_fires_on_load() {
        let counter = Arc::new(AtomicUsize::new(0));
        let module: Arc<dyn Module> = Arc::new(OnLoadCountingModule {
            counter: Arc::clone(&counter),
        });
        let registry = Registry::new();
        let discoverer = FixedDiscoverer::new(vec![dm("math.add", module)]);

        let count = registry.discover(&discoverer).await.unwrap();

        assert_eq!(count, 1);
        assert!(registry.has("math.add"));
        assert!(registry.get_definition("math.add").is_some());
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "on_load called exactly once"
        );
    }

    #[tokio::test]
    async fn invalid_module_id_is_skipped_and_does_not_abort_batch() {
        let registry = Registry::new();
        let discoverer = FixedDiscoverer::new(vec![
            dm("Invalid-ID", stub()),    // uppercase + hyphen — EBNF fail
            dm("", stub()),              // empty
            dm("system.hacker", stub()), // reserved first segment
            dm("good.one", stub()),
        ]);

        let count = registry.discover(&discoverer).await.unwrap();

        assert_eq!(count, 1, "only the single valid entry should register");
        assert!(registry.has("good.one"));
        assert!(registry.get_definition("Invalid-ID").is_none());
        assert!(registry.get_definition("").is_none());
        assert!(registry.get_definition("system.hacker").is_none());
    }

    #[tokio::test]
    async fn duplicate_entry_within_batch_is_skipped() {
        let registry = Registry::new();
        let discoverer = FixedDiscoverer::new(vec![
            dm("math.add", stub()),
            dm("math.add", stub()), // duplicate
        ]);

        let count = registry.discover(&discoverer).await.unwrap();

        assert_eq!(count, 1, "second duplicate should be skipped");
        assert!(registry.has("math.add"));
    }

    #[tokio::test]
    async fn custom_validator_rejects_entry() {
        let called = Arc::new(AtomicUsize::new(0));
        let registry = Registry::new();
        registry.set_validator(Box::new(RejectAllValidator {
            called: Arc::clone(&called),
        }));
        let discoverer = FixedDiscoverer::new(vec![dm("math.add", stub())]);

        let count = registry.discover(&discoverer).await.unwrap();

        assert_eq!(count, 0);
        assert!(!registry.has("math.add"));
        assert!(registry.get_definition("math.add").is_none());
        assert_eq!(called.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn register_callback_fires_once_per_entry() {
        let callback_count = Arc::new(std::sync::Mutex::new(0usize));
        let cc = Arc::clone(&callback_count);
        let registry = Registry::new();
        registry.on(
            "register",
            Box::new(move |_: &str, _: &dyn Module| {
                *cc.lock().unwrap() += 1;
            }),
        );

        let discoverer =
            FixedDiscoverer::new(vec![dm("math.add", stub()), dm("math.subtract", stub())]);
        let count = registry.discover(&discoverer).await.unwrap();

        assert_eq!(count, 2);
        assert_eq!(
            *callback_count.lock().unwrap(),
            2,
            "register callback fires once per registered entry"
        );
    }

    #[tokio::test]
    async fn discover_internal_without_discoverer_returns_error() {
        let registry = Registry::new();
        let err = registry.discover_internal().await.unwrap_err();
        assert_eq!(
            err.code,
            apcore::errors::ErrorCode::NoDiscovererConfigured,
            "discover_internal must return the dedicated NoDiscovererConfigured \
             error so real load failures surfaced as ModuleLoadError are not \
             masked by APCore::discover's swallow policy"
        );
    }

    #[tokio::test]
    async fn discovered_module_with_invalid_descriptor_schema_is_skipped() {
        // Regression: register_discovered must validate descriptor schema shapes
        // before inserting into schema_cache. A discoverer returning non-object
        // schemas (e.g., a string) must be rejected, not silently cached.
        fn dm_with_bad_schema(name: &str, module: Arc<dyn Module>) -> DiscoveredModule {
            let mut desc = make_descriptor(name);
            desc.input_schema = serde_json::json!("not-an-object"); // invalid schema shape
            DiscoveredModule {
                name: name.to_string(),
                source: "test".to_string(),
                descriptor: desc,
                module,
            }
        }

        let registry = Registry::new();
        let discoverer = FixedDiscoverer::new(vec![
            dm_with_bad_schema("bad.schema", stub()),
            dm("good.one", stub()), // valid entry in the same batch
        ]);

        let count = registry.discover(&discoverer).await.unwrap();

        assert_eq!(count, 1, "only the valid module should register");
        assert!(
            !registry.has("bad.schema"),
            "module with non-object schema must be rejected"
        );
        assert!(registry.has("good.one"), "valid module must still register");
    }

    #[tokio::test]
    async fn discoverer_is_restored_even_when_discover_panics() {
        // Regression: previously, if a custom Discoverer's discover().await
        // panicked, the RAII-less restore block was unreachable and the
        // discoverer was permanently lost.
        struct PanickingDiscoverer;
        #[async_trait]
        impl Discoverer for PanickingDiscoverer {
            async fn discover(
                &self,
                _roots: &[String],
            ) -> Result<Vec<DiscoveredModule>, ModuleError> {
                panic!("simulated discoverer failure");
            }
        }

        let registry = Arc::new(Registry::new());
        registry.set_discoverer(Box::new(PanickingDiscoverer));

        // First call panics; catch_unwind isolates the panic from the test harness.
        let r = Arc::clone(&registry);
        let first = tokio::spawn(async move { r.discover_internal().await }).await;
        assert!(first.is_err(), "panicking discoverer must propagate panic");

        // The Drop guard should have restored the discoverer; a second call
        // should find it still present (and panic again, not return
        // NoDiscovererConfigured).
        let r2 = Arc::clone(&registry);
        let second = tokio::spawn(async move { r2.discover_internal().await }).await;
        assert!(
            second.is_err(),
            "discoverer must still be present after first panic — it should panic again, \
             not disappear into 'NoDiscovererConfigured'"
        );
    }
}

// ---------------------------------------------------------------------------
// Lifecycle + conflict-detection regression tests
// ---------------------------------------------------------------------------

mod lifecycle_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Module that records the sequence of `on_load` / `on_unload` calls and
    /// tracks whether `execute` has ever been called — used to verify that
    /// `on_unload` never runs before the module is removed from the registry's
    /// live map (otherwise a concurrent `call()` could dispatch to an
    /// already-torn-down module).
    struct LifecycleModule {
        load_count: Arc<AtomicUsize>,
        unload_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Module for LifecycleModule {
        fn description(&self) -> &'static str {
            "lifecycle"
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({ "type": "object" })
        }
        fn output_schema(&self) -> Value {
            serde_json::json!({ "type": "object" })
        }
        async fn execute(
            &self,
            _inputs: Value,
            _ctx: &Context<Value>,
        ) -> Result<Value, ModuleError> {
            Ok(serde_json::json!({}))
        }
        fn on_load(&self) -> Result<(), ModuleError> {
            self.load_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn on_unload(&self) {
            self.unload_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn register_rejects_exact_duplicate() {
        let registry = Registry::new();
        registry
            .register("foo.bar", Box::new(StubModule), make_descriptor("foo.bar"))
            .expect("first registration succeeds");

        let err = registry
            .register("foo.bar", Box::new(StubModule), make_descriptor("foo.bar"))
            .unwrap_err();
        assert_eq!(err.code, apcore::errors::ErrorCode::GeneralInvalidInput);
        assert!(err.message.contains("already registered"));
    }

    #[test]
    fn on_load_is_skipped_when_registration_is_rejected_as_duplicate() {
        // Regression: previously on_load ran BEFORE the duplicate check, so
        // a rejected duplicate would leak resources opened by on_load.
        let registry = Registry::new();
        let load_count = Arc::new(AtomicUsize::new(0));
        let unload_count = Arc::new(AtomicUsize::new(0));

        registry
            .register(
                "foo.bar",
                Box::new(LifecycleModule {
                    load_count: Arc::clone(&load_count),
                    unload_count: Arc::clone(&unload_count),
                }),
                make_descriptor("foo.bar"),
            )
            .unwrap();
        assert_eq!(load_count.load(Ordering::SeqCst), 1);

        // Second register for same ID: rejected. A *new* LifecycleModule
        // instance's on_load must NOT run because the ID is a duplicate.
        let rejected_load_count = Arc::new(AtomicUsize::new(0));
        let rejected_unload_count = Arc::new(AtomicUsize::new(0));
        let err = registry.register(
            "foo.bar",
            Box::new(LifecycleModule {
                load_count: Arc::clone(&rejected_load_count),
                unload_count: Arc::clone(&rejected_unload_count),
            }),
            make_descriptor("foo.bar"),
        );
        assert!(err.is_err());
        assert_eq!(
            rejected_load_count.load(Ordering::SeqCst),
            0,
            "on_load MUST NOT fire for a registration rejected due to duplicate ID"
        );
    }

    #[test]
    fn unregister_removes_module_before_calling_on_unload() {
        // Regression: previously on_unload ran BEFORE the core-map removal,
        // so a concurrent `get()` could still dispatch to a module whose
        // resources had already been freed.
        let registry = Arc::new(Registry::new());
        let load_count = Arc::new(AtomicUsize::new(0));
        let unload_count = Arc::new(AtomicUsize::new(0));

        registry
            .register(
                "foo.bar",
                Box::new(LifecycleModule {
                    load_count: Arc::clone(&load_count),
                    unload_count: Arc::clone(&unload_count),
                }),
                make_descriptor("foo.bar"),
            )
            .unwrap();

        // Install a callback that runs DURING unregister (after remove,
        // before on_unload) and observes the registry state — the module
        // must already be gone from `get()` at this point.
        let present_at_callback = Arc::new(AtomicUsize::new(0));
        let pac_clone = Arc::clone(&present_at_callback);
        let registry_weak = Arc::downgrade(&registry);
        registry.on(
            "unregister",
            Box::new(move |name, _module| {
                if let Some(reg) = registry_weak.upgrade() {
                    if reg.get(name).is_some() {
                        pac_clone.store(1, Ordering::SeqCst);
                    }
                }
            }),
        );

        registry.unregister("foo.bar").unwrap();

        assert_eq!(
            present_at_callback.load(Ordering::SeqCst),
            0,
            "by the time the 'unregister' callback fires, the module must \
             already be gone from the registry's live map"
        );
        assert_eq!(
            unload_count.load(Ordering::SeqCst),
            1,
            "on_unload runs exactly once, after removal"
        );
    }

    #[test]
    fn validator_is_invoked_without_registry_lock_held() {
        // Regression: validator was previously called while holding the
        // validator read guard, so a validator that re-registered itself
        // would deadlock (parking_lot guards are non-reentrant). With the
        // Arc-snapshot fix the validator sees no lock held.
        use apcore::module::ValidationResult;
        use apcore::registry::registry::ModuleValidator;

        struct ReentrantValidator {
            registry: Arc<Registry>,
        }
        impl ModuleValidator for ReentrantValidator {
            fn validate(
                &self,
                _module: &dyn Module,
                _descriptor: Option<&ModuleDescriptor>,
            ) -> ValidationResult {
                // Re-entering the registry during validation must NOT deadlock.
                // We replace the validator — this takes the validator write lock.
                self.registry.set_validator(Box::new(PermissiveValidator));
                ValidationResult {
                    valid: true,
                    errors: vec![],
                    warnings: vec![],
                }
            }
        }

        struct PermissiveValidator;
        impl ModuleValidator for PermissiveValidator {
            fn validate(
                &self,
                _module: &dyn Module,
                _descriptor: Option<&ModuleDescriptor>,
            ) -> ValidationResult {
                ValidationResult {
                    valid: true,
                    errors: vec![],
                    warnings: vec![],
                }
            }
        }

        let registry = Arc::new(Registry::new());
        registry.set_validator(Box::new(ReentrantValidator {
            registry: Arc::clone(&registry),
        }));

        registry
            .register("foo.bar", Box::new(StubModule), make_descriptor("foo.bar"))
            .expect("validator that re-enters set_validator must not deadlock");
    }
}

mod on_load_rollback_tests {
    use super::*;
    use apcore::errors::ErrorCode;

    struct FailingOnLoadModule;

    #[async_trait]
    impl Module for FailingOnLoadModule {
        fn description(&self) -> &'static str {
            "fails on_load"
        }
        fn input_schema(&self) -> Value {
            serde_json::json!({ "type": "object" })
        }
        fn output_schema(&self) -> Value {
            serde_json::json!({ "type": "object" })
        }
        fn on_load(&self) -> Result<(), ModuleError> {
            Err(ModuleError::new(
                ErrorCode::ModuleLoadError,
                "simulated on_load failure".to_string(),
            ))
        }
        async fn execute(
            &self,
            _inputs: Value,
            _ctx: &Context<Value>,
        ) -> Result<Value, ModuleError> {
            Ok(serde_json::json!({}))
        }
    }

    #[test]
    fn register_rolls_back_when_on_load_returns_err() {
        let registry = Registry::new();
        let err = registry
            .register(
                "foo.bar",
                Box::new(FailingOnLoadModule),
                make_descriptor("foo.bar"),
            )
            .unwrap_err();

        assert_eq!(
            err.code,
            ErrorCode::ModuleLoadError,
            "register must propagate on_load error"
        );
        assert!(err.message.contains("on_load"), "error message: {}", err.message);
        assert!(
            registry.get("foo.bar").is_none(),
            "module must not remain in registry after on_load failure"
        );
        assert_eq!(
            registry.list(None, None).len(),
            0,
            "registry must be empty after failed registration"
        );
    }

    #[test]
    fn register_succeeding_module_after_failed_on_load_works() {
        let registry = Registry::new();

        // First registration fails due to on_load
        let _ = registry.register(
            "foo.bad",
            Box::new(FailingOnLoadModule),
            make_descriptor("foo.bad"),
        );

        // Registry must still accept a valid module with the same ID slot
        registry
            .register("foo.bad", Box::new(StubModule), make_descriptor("foo.bad"))
            .expect(
                "registry must accept registration after a prior failed on_load for the same id",
            );

        assert!(registry.get("foo.bad").is_some());
    }
}
