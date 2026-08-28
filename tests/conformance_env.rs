//! Canonical conformance-fixture locator, shared by every conformance driver.
//!
//! The fixtures and schemas are the single source of truth in the apcore spec
//! repo; this SDK reads them in place rather than vendoring a copy, so a
//! spec-side edit reaches Rust on the next test run.
//!
//! Search order:
//!   1. `$CONFORMANCE_SPEC_REPO` — the spec repo root (set by CI).
//!   2. `../apcore/` beside this repo — the standard workspace layout.
//!
//! This file is not a test target of its own (`autotests = false`); it is
//! included by `tests/it.rs` and `tests/conformance_test.rs`, each of which
//! uses a subset of it — hence the crate-wide `dead_code` allow.

#![allow(dead_code)]

use std::path::PathBuf;

const SPEC_REPO_ENV: &str = "CONFORMANCE_SPEC_REPO";

/// Transitional fallback (apcore#86). The locator used to be
/// `APCORE_SPEC_REPO`, but PROTOCOL_SPEC §9.2 makes *every* `APCORE_*`
/// variable a config override: the suffix is lowercased and split into a dot
/// path, so `APCORE_SPEC_REPO=/path` injected `spec.repo` into the declared
/// config document that §9.1's required-field check runs against. The locator
/// is test infrastructure, not configuration, so it moved out of the claimed
/// prefix. Reading the old name keeps a developer who still exports it
/// working. REMOVE once all three SDK CI workflows are on
/// `CONFORMANCE_SPEC_REPO`.
const LEGACY_SPEC_REPO_ENV: &str = "APCORE_SPEC_REPO";

/// Return the spec-repo override as `(variable_name, value)`, if set.
///
/// The name travels with the value so a panic message can name the variable
/// the developer actually set rather than the one they did not.
fn spec_repo_env() -> Option<(&'static str, String)> {
    for name in [SPEC_REPO_ENV, LEGACY_SPEC_REPO_ENV] {
        match std::env::var(name) {
            Ok(value) if !value.is_empty() => return Some((name, value)),
            _ => {}
        }
    }
    None
}

/// Resolve `<spec repo>/<relative>`, panicking with a fix-it message if absent.
fn spec_repo_subdir(relative: &[&str], what: &str) -> PathBuf {
    if let Some((name, value)) = spec_repo_env() {
        let dir = relative
            .iter()
            .fold(PathBuf::from(&value), |acc, part| acc.join(part));
        if dir.is_dir() {
            return dir;
        }
        panic!("{name}={value} does not contain {what}");
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let apcore = manifest_dir.parent().unwrap().join("apcore");
    let sibling = relative
        .iter()
        .fold(apcore.clone(), |acc, part| acc.join(part));
    if sibling.is_dir() {
        return sibling;
    }

    panic!(
        "Cannot find apcore {what}\n\
         Fix one of:\n\
         1. Set {SPEC_REPO_ENV} to the apcore spec repo path\n\
         2. Clone apcore as a sibling at {}",
        apcore.display()
    );
}

/// Locate the canonical `conformance/fixtures/` directory.
pub fn find_fixtures_root() -> PathBuf {
    spec_repo_subdir(&["conformance", "fixtures"], "conformance/fixtures/")
}

/// Locate a fixture that is deliberately **staged** outside
/// `conformance/fixtures/` while a cross-SDK rollout is in flight.
///
/// A fixture whose expectations changed cannot land in the canonical directory
/// until every SDK driver implements the new behaviour, or CI goes red in each
/// repository for the duration. The spec repo therefore stages it under
/// `planning/<topic>/staged-fixtures/`, and a driver that has already converged
/// reads it from there.
///
/// Returns `None` when the staged copy is absent — which is what happens once
/// the fixture is promoted, so the driver falls back to the canonical location
/// with no code change.
pub fn find_staged_fixture(topic: &str, file_name: &str) -> Option<PathBuf> {
    let roots: Vec<PathBuf> = match spec_repo_env() {
        Some((_, value)) => vec![PathBuf::from(value)],
        None => vec![PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("apcore")],
    };
    roots
        .into_iter()
        .map(|root| {
            root.join("planning")
                .join(topic)
                .join("staged-fixtures")
                .join(file_name)
        })
        .find(|path| path.is_file())
}

/// Locate the canonical `schemas/` directory.
pub fn find_schemas_root() -> PathBuf {
    spec_repo_subdir(&["schemas"], "schemas/")
}
