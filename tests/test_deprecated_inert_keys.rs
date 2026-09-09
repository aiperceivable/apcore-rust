//! PROTOCOL_SPEC §9.2.4 / §9.2.4.1 — the deprecation notice for the ten
//! declared configuration keys that reach no consumer in any SDK
//! (aiperceivable/apcore#118).
//!
//! Two warnings, one per document kind:
//!
//! | Document | Emitted by | Marker |
//! |---|---|---|
//! | `apcore.yaml` | `Config::warn_deprecated_inert_keys` | `DEPRECATION (spec §9.2.4)` |
//! | an ACL file | `ACL::load` | `DEPRECATION (spec §9.2.4.1)` |
//!
//! ## Every case here asserts BOTH halves, and the negative one is the point
//!
//! A test that only checks "a config declaring `logging.level` warns" would
//! have passed the **broken** first implementation of this warning, which fired
//! for every configuration ever loaded — including clean ones.
//!
//! The reason is specific to this SDK and is why these cases exist. The notice
//! MUST be driven by the **as-written** document (§9.2.4 requirement 2), and
//! neither of the two obvious sources can supply that: `observability` is a
//! typed struct field whose leaves always carry a value, so
//! `Config::get_declared("observability.tracing.enabled")` answers `Some(false)`
//! for a document that never mentions it, and `Config::data()` serialises the
//! same typed fields. Measured on the first attempt: a clean config reported
//! four declared keys. The implementation reads `user_namespaces` instead,
//! which retains the raw object of every typed section exactly as the file
//! wrote it — so [`a_clean_configuration_is_silent`] and
//! [`a_declared_section_with_no_deprecated_leaf_is_silent`] are the cases that
//! hold it to that source, and would go red the moment it went back to either
//! of the others.
//!
//! The ACL half has a third negative for a different reason. The notice lives
//! in the loader — the ACL file is parsed into a `serde_json::Value` whose
//! unrecognised root keys are dropped in silence, so deleting the `audit` block
//! from `acl-config.schema.json` would produce no signal at all — and it is
//! scoped to `audit` **deliberately**. It is a deprecation notice, not unknown-
//! key closure for ACL files, so
//! [`an_acl_file_with_an_unrelated_unknown_root_key_is_silent`] pins that every
//! other unrecognised root key keeps being ignored exactly as before.
//!
//! ## What is NOT asserted
//!
//! Behaviour. §9.2.4 requirement 3 keeps all ten keys parsing, validating,
//! answering `get()` and passing `_config.strict` for the whole 1.x line; the
//! `audit:` block keeps being ignored. Nothing in this file loads a
//! configuration that would fail before the warning existed, and nothing here
//! reads a value back — this is a diagnostic, and the diagnostic is the only
//! thing that changed.

use std::sync::{Arc, Mutex, PoisonError};

use apcore::acl::ACL;
use apcore::config::Config;

// ---------------------------------------------------------------------------
// The key set
// ---------------------------------------------------------------------------

/// PROTOCOL_SPEC §9.2.4's ten keys, in the order the notice reports them.
///
/// Spelled out rather than read from `src/config.rs` — `DEPRECATED_INERT_KEYS`
/// is private, and restating the spec's list here is what makes
/// [`all_ten_declared_at_once_are_named_in_spec_order`] an independent check on
/// the const rather than a mirror of it. The order is load-bearing: it is what
/// makes three SDKs name the same keys in the same sequence for the same file.
const DEPRECATED_INERT_KEYS: [&str; 10] = [
    "observability.tracing.enabled",
    "observability.tracing.sampling_rate",
    "observability.tracing.exporter",
    "observability.metrics.enabled",
    "observability.metrics.exporter",
    "logging.level",
    "logging.format",
    "acl.audit.enabled",
    "acl.audit.include_denied",
    "acl.audit.log_level",
];

/// The substring that identifies the `apcore.yaml` notice.
///
/// The trailing `)` is what keeps it distinct from the ACL file's
/// `§9.2.4.1` marker, so a config assertion can never be satisfied by an ACL
/// warning or the reverse.
const CONFIG_MARKER: &str = "DEPRECATION (spec §9.2.4)";

/// The substring that identifies the ACL-file notice.
const ACL_MARKER: &str = "DEPRECATION (spec §9.2.4.1)";

// ---------------------------------------------------------------------------
// Warning capture
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl tracing_subscriber::fmt::MakeWriter<'_> for CaptureWriter {
    type Writer = Self;
    fn make_writer(&self) -> Self::Writer {
        self.clone()
    }
}

/// Run `f` under a THREAD-LOCAL subscriber and return everything it logged.
///
/// Thread-local (not global) so these cases neither steal nor are polluted by
/// the output of the rest of the consolidated `it` binary running beside them.
fn capture_logs(f: impl FnOnce()) -> String {
    let buf = CaptureWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buf.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    let bytes = buf.0.lock().unwrap_or_else(PoisonError::into_inner).clone();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Write `yaml` to a real `apcore.yaml`, load it the way a deployment does, and
/// return everything the load logged.
///
/// Goes through `Config::load` from a file on disk rather than
/// `Config::from_defaults()` + `set(…)`: `set` writes straight into
/// `user_namespaces`, so it would report a key as declared whatever the
/// deserializer did with it, and the deserializer is exactly what decides
/// whether a typed section's raw object survives.
///
/// The `TempDir` is dropped on return — the notice fires during the load, and
/// nothing here reads the file again.
fn load_config_capturing(yaml: &str) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("apcore.yaml");
    std::fs::write(&path, yaml).expect("write apcore.yaml");
    capture_logs(|| {
        Config::load(&path)
            .expect("these documents must all load — the notice changes no behaviour");
    })
}

/// Write `yaml` to a real ACL file, load it, and return everything the load
/// logged.
fn load_acl_capturing(yaml: &str) -> String {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("global_acl.yaml");
    std::fs::write(&path, yaml).expect("write global_acl.yaml");
    let path = path.to_str().expect("utf-8 tempdir path").to_string();
    capture_logs(|| {
        ACL::load(&path).expect("these documents must all load — the notice changes no behaviour");
    })
}

/// A namespace-mode `apcore.yaml` carrying `sections` and nothing else.
fn document(sections: &str) -> String {
    format!("apcore:\n  version: \"1.0\"\n{sections}")
}

/// The smallest document that declares exactly `key`, and no other member of
/// the ten.
///
/// Nests one YAML level per dotted segment, so it handles both the
/// three-segment keys (`observability.tracing.enabled`) and the two-segment
/// ones (`logging.level`).
///
/// Values are chosen to be *valid* for the key so the load succeeds on its
/// merits — `observability.tracing.sampling_rate` has a `[0.0, 1.0]` constraint
/// the validator enforces, and a document rejected before the notice runs would
/// make the case pass for the wrong reason.
fn document_declaring(key: &str) -> String {
    let segments: Vec<&str> = key.split('.').collect();
    let (leaf, parents) = segments.split_last().expect("every key has a leaf");
    let value = match *leaf {
        "sampling_rate" => "0.25",
        "exporter" => "\"stdout\"",
        "level" | "log_level" => "\"info\"",
        "format" => "\"json\"",
        _ => "true",
    };

    let mut yaml = String::new();
    for (depth, segment) in parents.iter().enumerate() {
        yaml.push_str(&"  ".repeat(depth));
        yaml.push_str(segment);
        yaml.push_str(":\n");
    }
    yaml.push_str(&"  ".repeat(parents.len()));
    yaml.push_str(&format!("{leaf}: {value}\n"));
    document(&yaml)
}

// ---------------------------------------------------------------------------
// apcore.yaml — the ten keys warn
// ---------------------------------------------------------------------------

/// Each of the ten, declared alone, warns and is named in the notice.
///
/// One case per key rather than ten cases: the notice reports the keys it found
/// as a list, so a per-key document is the only shape that proves the *reported*
/// key is the *declared* one, and a loop over the whole set is what proves no
/// key is quietly missing from the traversal.
#[test]
fn each_deprecated_key_warns_and_the_notice_names_it() {
    for key in DEPRECATED_INERT_KEYS {
        let logs = load_config_capturing(&document_declaring(key));
        assert!(
            logs.contains(CONFIG_MARKER),
            "a configuration declaring `{key}` must emit the §9.2.4 notice — \
             the key reaches no consumer in any SDK and an operator has no other \
             way to find that out. Captured:\n{logs}"
        );
        assert!(
            logs.contains(key),
            "the §9.2.4 notice fired for `{key}` but does not name it — a notice \
             that does not say WHICH key is inert cannot be acted on. \
             Captured:\n{logs}"
        );
    }
}

/// All ten in one document are reported together, in the spec's order.
///
/// Pins the count and the ordering the const's own documentation claims: the
/// notice is a cross-SDK diagnostic, and three SDKs naming the same ten keys in
/// three different sequences is a diff an operator has to reconcile by hand.
#[test]
fn all_ten_declared_at_once_are_named_in_spec_order() {
    let logs = load_config_capturing(&document(
        "observability:\n  \
           tracing:\n    \
             enabled: true\n    \
             sampling_rate: 0.25\n    \
             exporter: \"stdout\"\n  \
           metrics:\n    \
             enabled: true\n    \
             exporter: \"prometheus\"\n\
         logging:\n  \
           level: \"info\"\n  \
           format: \"json\"\n\
         acl:\n  \
           audit:\n    \
             enabled: true\n    \
             include_denied: true\n    \
             log_level: \"info\"\n",
    ));

    assert!(
        logs.contains(CONFIG_MARKER),
        "a configuration declaring all ten keys must emit the §9.2.4 notice. \
         Captured:\n{logs}"
    );
    assert!(
        logs.contains("count=10"),
        "all ten keys are declared, so the notice must report ten. Captured:\n{logs}"
    );
    assert!(
        logs.contains(&DEPRECATED_INERT_KEYS.join(", ")),
        "the notice must list the ten keys in §9.2.4's order so every SDK names \
         them the same way for the same file. Captured:\n{logs}"
    );
}

// ---------------------------------------------------------------------------
// apcore.yaml — the negative half
// ---------------------------------------------------------------------------

/// A configuration declaring none of the ten is SILENT.
///
/// This is the requirement, not a nicety. Every one of the ten has a canonical
/// default and four of them are typed struct leaves that always carry a value,
/// so a notice driven by the merged view — `get_declared`, or `data()` — fires
/// here, for every configuration ever loaded. That is the blanket warning
/// §9.2.2 rejects and §9.2.4 requirement 2 forbids, and it is what the first
/// implementation of this notice actually did.
#[test]
fn a_clean_configuration_is_silent() {
    let logs = load_config_capturing(&document(""));
    assert!(
        !logs.contains(CONFIG_MARKER),
        "a configuration that mentions none of the ten keys must NOT emit the \
         §9.2.4 notice. It fired, which means the notice is reading the MERGED \
         view (`get_declared` / `data()` answer `Some(false)` for a typed \
         `observability` leaf no document declares) instead of the as-written \
         `user_namespaces` tree. Captured:\n{logs}"
    );
}

/// A document that declares the *sections* and the *groups*, but none of the
/// ten leaves, is silent.
///
/// The sharper half of the negative: `a_clean_configuration_is_silent` is
/// satisfied by any check that first asks "is this section present at all",
/// while this one is not. `observability.tracing` and `acl` are both written
/// here — `acl` even carries live, non-deprecated keys — and the traversal has
/// to reach the leaf before it may report anything.
#[test]
fn a_declared_section_with_no_deprecated_leaf_is_silent() {
    let logs = load_config_capturing(&document(
        "observability:\n  \
           tracing: {}\n  \
           metrics: {}\n\
         acl:\n  \
           default_effect: deny\n  \
           root: \"/srv/apcore/acl\"\n",
    ));
    assert!(
        !logs.contains(CONFIG_MARKER),
        "`observability.tracing`, `observability.metrics` and `acl` are declared \
         but not one of the ten leaves under them is — the notice must reach the \
         LEAF before it reports, or every deployment that configures \
         `acl.default_effect` gets a warning about keys it never wrote. \
         Captured:\n{logs}"
    );
}

// ---------------------------------------------------------------------------
// ACL files — §9.2.4.1
// ---------------------------------------------------------------------------

const ACL_RULES: &str = "default_effect: deny\nrules: []\n";

/// An `audit:` block in an ACL file warns.
#[test]
fn an_acl_file_with_an_audit_block_warns() {
    let logs = load_acl_capturing(&format!(
        "{ACL_RULES}audit:\n  enabled: true\n  include_denied: true\n  log_level: \"info\"\n"
    ));
    assert!(
        logs.contains(ACL_MARKER),
        "an ACL file declaring an `audit:` block must emit the §9.2.4.1 notice. \
         No SDK has ever read that block and the loader drops unknown root keys \
         in silence, so this notice is the only signal that exists. \
         Captured:\n{logs}"
    );
    assert!(
        logs.contains("audit"),
        "the §9.2.4.1 notice must name the `audit:` block it is about. \
         Captured:\n{logs}"
    );
}

/// An ACL file with no `audit:` block is SILENT.
#[test]
fn an_acl_file_without_an_audit_block_is_silent() {
    let logs = load_acl_capturing(ACL_RULES);
    assert!(
        !logs.contains(ACL_MARKER),
        "an ACL file that declares no `audit:` block must NOT emit the §9.2.4.1 \
         notice — a deprecation notice on every ACL load names nothing an \
         operator can remove. Captured:\n{logs}"
    );
}

/// An ACL file carrying some OTHER unrecognised root key is silent.
///
/// The notice is scoped to `audit` deliberately. ACL files have never been
/// closed to unknown root keys — the loader parses into a `serde_json::Value`
/// and takes the fields it wants — and this notice must not become that closure
/// by the back door: it opens a removal window for one block, and changes
/// nothing else about how the file is read.
#[test]
fn an_acl_file_with_an_unrelated_unknown_root_key_is_silent() {
    let logs = load_acl_capturing(&format!(
        "{ACL_RULES}metadata:\n  owner: \"platform-team\"\n  reviewed: \"2026-09-09\"\n"
    ));
    assert!(
        !logs.contains(ACL_MARKER),
        "`metadata:` is an unrecognised root key, not the deprecated `audit:` \
         block. The §9.2.4.1 notice is a deprecation notice for ONE block, not \
         unknown-key closure for ACL files — every other root key keeps being \
         ignored exactly as before. Captured:\n{logs}"
    );
}
