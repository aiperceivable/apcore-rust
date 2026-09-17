//! D-89 — at most one `x-deprecation` warning per `(module_id, version)` per
//! registry instance.
//!
//! This SDK already had a unit test for the cadence, and it asserted the SIZE
//! OF THE PRIVATE `deprecation_warned` SET. "The set has one entry" and "one
//! warning reached the operator" are different claims: a `log_deprecation_warning`
//! that returned early, logged at `debug!`, or was dropped from the call site
//! entirely would leave the set exactly as the unit test requires. The set is
//! the artifact that records the decision; the emitted warning is the mechanism
//! it exists for. This file asserts the mechanism.
//!
//! Two dimensions are deliberately NOT asserted, because the three SDKs
//! disagree and D-89 settles neither:
//!
//!   * WHERE the warning fires — this SDK warns at REGISTRATION, apcore-python
//!     and apcore-typescript warn on the read (`get_definition`). D-89's own
//!     rationale ("`get_definition` is a read that hosts call in loops") is
//!     written about the read path.
//!   * What an unregister + re-register does — apcore-python forgets the marker
//!     and re-warns, this SDK and apcore-typescript stay silent. The unit test
//!     beside this one asserts the silence AS THE REQUIREMENT, and
//!     apcore-python's source comment asserts the opposite as the requirement.
//!
//! See the open item beside D-89 in the decision log.

use apcore::context::Context;
use apcore::errors::ModuleError;
use apcore::module::Module;
use apcore::registry::registry::Registry;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
struct Noop;

#[async_trait]
impl Module for Noop {
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn description(&self) -> &'static str {
        "a deprecated module"
    }
    async fn execute(&self, _i: Value, _c: &Context<Value>) -> Result<Value, ModuleError> {
        Ok(json!({}))
    }
}

fn deprecation_metadata() -> HashMap<String, Value> {
    let mut m = HashMap::new();
    m.insert(
        "x-deprecation".to_string(),
        json!({
            "deprecated_since": "1.0.0",
            "sunset_version": "3.0.0",
            "migration_guide": "Use mod.new instead.",
        }),
    );
    m
}

#[derive(Clone, Default)]
struct WarnCapture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for WarnCapture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl tracing_subscriber::fmt::MakeWriter<'_> for WarnCapture {
    type Writer = Self;
    fn make_writer(&self) -> Self::Writer {
        self.clone()
    }
}

/// Count `x-deprecation` warnings actually emitted while `f` runs.
fn deprecation_warnings(f: impl FnOnce()) -> usize {
    let buf = WarnCapture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buf.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    let bytes = buf
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    String::from_utf8_lossy(&bytes)
        .lines()
        .filter(|l| l.contains("is deprecated"))
        .count()
}

fn register(reg: &Registry, id: &str, version: &str) {
    reg.register_versioned(
        id,
        Box::new(Noop),
        Some(version),
        Some(deprecation_metadata()),
    )
    .expect("register");
}

#[test]
fn one_warning_reaches_the_operator_per_module_and_version() {
    let reg = Registry::new();
    assert_eq!(
        deprecation_warnings(|| register(&reg, "cadence.once", "1.0.0")),
        1,
        "registering a deprecated module must warn exactly once"
    );
    assert_eq!(
        deprecation_warnings(|| {
            for _ in 0..10 {
                let _ = reg.get_definition("cadence.once");
            }
        }),
        0,
        "the advisory must not ride the read path"
    );
}

#[test]
fn control_a_module_with_no_deprecation_block_never_warns() {
    // Without this, "exactly one warning" would also hold for a registry that
    // warns once for any module at all.
    let reg = Registry::new();
    assert_eq!(
        deprecation_warnings(|| {
            reg.register_versioned("cadence.plain", Box::new(Noop), Some("1.0.0"), None)
                .expect("register");
            let _ = reg.get_definition("cadence.plain");
        }),
        0
    );
}

#[test]
fn distinct_modules_warn_separately() {
    let reg = Registry::new();
    assert_eq!(
        deprecation_warnings(|| {
            register(&reg, "cadence.first", "1.0.0");
            register(&reg, "cadence.second", "1.0.0");
        }),
        2
    );
}

#[test]
fn the_dedupe_is_per_registry_instance_not_process_global() {
    // A process-global set would silence the second registry's advisory for a
    // module its operator has never been told about.
    let a = Registry::new();
    let b = Registry::new();
    assert_eq!(
        deprecation_warnings(|| {
            register(&a, "cadence.instance", "1.0.0");
            register(&b, "cadence.instance", "1.0.0");
        }),
        2
    );
}

#[test]
fn the_dedupe_key_includes_the_version() {
    // A newly registered version is a new deprecation notice with its own
    // sunset. Keying on the module id alone silences it.
    let reg = Registry::new();
    assert_eq!(
        deprecation_warnings(|| register(&reg, "cadence.versions", "1.0.0")),
        1
    );
    reg.unregister("cadence.versions").expect("unregister");
    assert_eq!(
        deprecation_warnings(|| register(&reg, "cadence.versions", "2.0.0")),
        1,
        "a different version is a different notice"
    );
}
