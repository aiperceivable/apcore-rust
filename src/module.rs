// APCore Protocol — Module trait and related types
// Spec reference: Module definition, annotations, preflight checks

use async_trait::async_trait;
use futures_core::Stream;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::pin::Pin;
use std::sync::LazyLock;

use crate::context::Context;
use crate::errors::ModuleError;

/// A stream of output chunks from a streaming module.
///
/// Each item is a `Result<Value, ModuleError>` so a module can fail mid-stream.
/// The stream is `Send + 'static` (no borrows from the producing module) so the
/// executor can drive it across `.await` points without lifetime issues.
pub type ChunkStream = Pin<Box<dyn Stream<Item = Result<serde_json::Value, ModuleError>> + Send>>;

/// Core trait that all `APCore` modules must implement.
#[async_trait]
pub trait Module: Send + Sync {
    /// Returns the JSON Schema describing this module's input.
    fn input_schema(&self) -> serde_json::Value;

    /// Returns the JSON Schema describing this module's output.
    fn output_schema(&self) -> serde_json::Value;

    /// Returns a human-readable description of this module.
    fn description(&self) -> &str;

    /// Execute the module with the given inputs and context.
    async fn execute(
        &self,
        inputs: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<serde_json::Value, ModuleError>;

    /// Stream execution — returns an async `Stream` of output chunks.
    ///
    /// Returns `None` if the module does not support streaming, signaling
    /// the executor to fall back to `execute()`. Modules that support
    /// streaming should override this to yield chunks incrementally — each
    /// `yield` is delivered to the caller as soon as it is produced (true
    /// streaming, no buffering).
    ///
    /// Note: this method is *not* `async` even though it returns a stream.
    /// The returned `ChunkStream` is itself an async iterator; constructing
    /// it must be cheap and synchronous so the executor can wire it into
    /// its own pipeline before the first chunk is awaited.
    ///
    /// **Validation contract:** `Executor::stream` validates the module's
    /// *merged* output (all chunks deep-merged) against `output_schema` only
    /// *after* the stream is exhausted (Phase 3). Individual chunks are **not**
    /// validated as they are yielded. Callers performing incremental chunk
    /// processing must tolerate receiving chunks that may not independently
    /// satisfy `output_schema`. If per-chunk schema guarantees are required,
    /// validate each chunk inside this method before yielding it.
    fn stream(
        &self,
        _inputs: serde_json::Value,
        _ctx: &Context<serde_json::Value>,
    ) -> Option<ChunkStream> {
        None
    }

    /// Return a structured description of this module for AI/LLM consumption (spec §5.6).
    /// Default: builds description from `input_schema`, `output_schema`, and description.
    fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "description": self.description(),
            "input_schema": self.input_schema(),
            "output_schema": self.output_schema(),
        })
    }

    /// Run preflight checks before execution.
    ///
    /// Cross-language alignment (D11-009): mirrors apcore-python
    /// `Module.preflight(inputs, context) -> list[str]` and apcore-typescript
    /// `preflight(inputs, context): string[]`. Returns a list of advisory
    /// warning strings; an empty list means "no concerns". Modules that need
    /// to gate execution should return errors from `execute()` directly
    /// instead — preflight is non-fatal.
    ///
    /// `ctx` is `Option<&Context>` because `Executor::validate(module_id,
    /// inputs, ctx)` accepts a `None` context for call-chain-free preflight
    /// (matching Python's `executor.validate(..., context=None)`); modules
    /// that need a context for their checks must handle the `None` case.
    ///
    /// The default implementation returns an empty warning list. Modules
    /// override this to inspect inputs (e.g., flag oversize payloads, warn
    /// on deprecated argument shapes) without rejecting the call.
    fn preflight(
        &self,
        _inputs: &serde_json::Value,
        _ctx: Option<&Context<serde_json::Value>>,
    ) -> Vec<String> {
        Vec::new()
    }

    /// Module-instance tags (D11-003).
    ///
    /// Cross-language alignment with apcore-python (`registry.py:1027`,
    /// reads `getattr(mod, 'tags', [])`) and apcore-typescript
    /// (`registry.ts:689`, reads `mod['tags']`). Modules MAY override
    /// this to participate in `Registry::list(tags=...)` filtering even
    /// when registered without an explicit `ModuleDescriptor` (e.g. via
    /// `register_module(name, module)`). The Rust `Registry::list`
    /// unions these instance tags with `descriptor.tags`.
    ///
    /// The default returns an empty Vec.
    fn tags(&self) -> Vec<String> {
        Vec::new()
    }

    /// Called after the module is registered.
    ///
    /// Returns `Err` to signal that the module failed to initialise; the
    /// registry rolls back the insertion so no half-initialised module remains
    /// registered. Aligns with `apcore-python Registry._invoke_on_load`.
    ///
    /// Default: no-op (`Ok(())`).
    fn on_load(&self) -> Result<(), ModuleError> {
        Ok(())
    }

    /// Called before the module is unregistered. Default: no-op.
    fn on_unload(&self) {}

    /// Called before hot-reload to capture state. Returns state dict for `on_resume()`.
    /// Default: returns None (no state to preserve).
    fn on_suspend(&self) -> Option<serde_json::Value> {
        None
    }

    /// Called after hot-reload to restore state from `on_suspend()`.
    /// Default: no-op.
    fn on_resume(&self, _state: serde_json::Value) {}
}

/// Metadata annotations attached to a module.
/// Describes behavioral characteristics of the module.
///
/// **Wire format (`PROTOCOL_SPEC` §4.4.1):** the `extra` field is serialized as a
/// nested JSON object under the key `"extra"`. Extension keys MUST NOT be
/// flattened to the annotations root. The custom `Deserialize` impl below
/// accepts both the canonical nested form and the legacy flattened form
/// (apcore-rust ≤ 0.17.1) for one MINOR backward-compat cycle.
#[derive(Debug, Clone, Serialize)]
#[allow(clippy::struct_excessive_bools)] // spec-defined annotation flags; consolidating into bitflags would break the public API
pub struct ModuleAnnotations {
    pub readonly: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub requires_approval: bool,
    pub open_world: bool,
    pub streaming: bool,
    pub cacheable: bool,
    pub cache_ttl: u64,
    pub cache_key_fields: Option<Vec<String>>,
    pub paginated: bool,
    pub pagination_style: String, // "cursor" | "offset" | "page"
    /// Extension map for ecosystem package metadata.
    /// Serialized as a nested `"extra"` object per spec §4.4.1.
    pub extra: HashMap<String, serde_json::Value>,
    // Legacy fields moved to ModuleDescriptor:
    // name, version, author, description, tags, category, deprecated,
    // deprecated_message, since, hidden, examples, dependencies, metadata
}

impl<'de> Deserialize<'de> for ModuleAnnotations {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct AnnotationsVisitor;

        impl<'de> Visitor<'de> for AnnotationsVisitor {
            type Value = ModuleAnnotations;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a ModuleAnnotations JSON object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<ModuleAnnotations, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut ann = ModuleAnnotations::default();
                let mut explicit_extra: Option<HashMap<String, serde_json::Value>> = None;
                let mut overflow: HashMap<String, serde_json::Value> = HashMap::new();

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "readonly" => ann.readonly = map.next_value()?,
                        "destructive" => ann.destructive = map.next_value()?,
                        "idempotent" => ann.idempotent = map.next_value()?,
                        "requires_approval" => ann.requires_approval = map.next_value()?,
                        "open_world" => ann.open_world = map.next_value()?,
                        "streaming" => ann.streaming = map.next_value()?,
                        "cacheable" => ann.cacheable = map.next_value()?,
                        "cache_ttl" => ann.cache_ttl = map.next_value()?,
                        "cache_key_fields" => ann.cache_key_fields = map.next_value()?,
                        "paginated" => ann.paginated = map.next_value()?,
                        "pagination_style" => ann.pagination_style = map.next_value()?,
                        "extra" => {
                            // Tolerate `null` extra → empty map.
                            let v: serde_json::Value = map.next_value()?;
                            explicit_extra = Some(match v {
                                serde_json::Value::Null => HashMap::new(),
                                serde_json::Value::Object(obj) => obj.into_iter().collect(),
                                _ => {
                                    return Err(serde::de::Error::custom(
                                        "ModuleAnnotations.extra must be an object",
                                    ))
                                }
                            });
                        }
                        _ => {
                            // Legacy flattened form (apcore-rust ≤ 0.17.1):
                            // unknown keys at the root are captured into overflow
                            // and later normalized into `extra`.
                            let v: serde_json::Value = map.next_value()?;
                            overflow.insert(key, v);
                        }
                    }
                }

                // §4.4.1 rule 7: nested explicit `extra` wins over legacy
                // top-level overflow. Build the merged map by writing overflow
                // first and then explicit_extra so the latter overwrites on
                // collision.
                let mut merged = overflow;
                if let Some(ex) = explicit_extra {
                    for (k, v) in ex {
                        merged.insert(k, v);
                    }
                }
                ann.extra = merged;
                Ok(ann)
            }
        }

        deserializer.deserialize_map(AnnotationsVisitor)
    }
}

impl Default for ModuleAnnotations {
    fn default() -> Self {
        Self {
            readonly: false,
            destructive: false,
            idempotent: false,
            requires_approval: false,
            open_world: true,
            streaming: false,
            cacheable: false,
            cache_ttl: 0,
            cache_key_fields: None,
            paginated: false,
            pagination_style: "cursor".to_string(),
            extra: HashMap::new(),
        }
    }
}

/// Default annotations instance — all fields at their spec defaults.
pub static DEFAULT_ANNOTATIONS: LazyLock<ModuleAnnotations> =
    LazyLock::new(ModuleAnnotations::default);

/// An example input/output pair for documentation.
///
/// Marked `#[non_exhaustive]` (issue #24) so future spec extensions can add
/// fields without breaking downstream struct-literal construction. Construct
/// via `..Default::default()` or a builder pattern.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModuleExample {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub inputs: serde_json::Value,
    pub output: serde_json::Value,
}

/// Result of validating a single aspect (used by `SchemaValidator` and `ModuleValidator`).
///
/// Marked `#[non_exhaustive]` (issue #24) so future spec extensions can add
/// fields without breaking downstream struct-literal construction. Construct
/// via `..Default::default()` or a builder pattern.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ValidationResult {
    pub valid: bool,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Result of a single preflight check (spec §12.8.4).
///
/// Marked `#[non_exhaustive]` (issue #24) so future spec extensions can add
/// fields without breaking downstream struct-literal construction. Construct
/// via `..Default::default()` or a builder pattern.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PreflightCheckResult {
    /// Check name (e.g., "`module_id`", "`module_lookup`", "`call_chain`", "acl", "schema", "`module_preflight`").
    pub check: String,
    /// Whether the check passed.
    pub passed: bool,
    /// Error details when `passed` is false; None when passed is true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<serde_json::Value>,
    /// Non-fatal advisory messages (default: empty).
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Aggregated preflight results returned by `Executor::validate()` (spec §12.8.3).
///
/// Marked `#[non_exhaustive]` (issue #24) so future spec extensions can add
/// fields (e.g. `predicted_changes` per the upstream `preview()` RFC) without
/// breaking downstream struct-literal construction. Construct via
/// `..Default::default()` or a builder pattern.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PreflightResult {
    /// True only if ALL checks passed.
    pub valid: bool,
    /// Ordered list of check results.
    pub checks: Vec<PreflightCheckResult>,
    /// True if the module has `requires_approval` annotation.
    #[serde(default)]
    pub requires_approval: bool,
}

impl PreflightResult {
    /// Computed view: failed checks as typed refs (idiomatic Rust accessor).
    #[must_use]
    pub fn errors(&self) -> Vec<&PreflightCheckResult> {
        self.checks.iter().filter(|c| !c.passed).collect()
    }

    /// Computed view: failed checks serialized to JSON Value maps.
    ///
    /// Cross-language parity with apcore-python PreflightResult.errors
    /// (returns list[dict]) and apcore-typescript PreflightResult.errors
    /// (returns array of objects) — sync finding A-014.
    #[must_use]
    pub fn errors_as_json(&self) -> Vec<serde_json::Value> {
        self.checks
            .iter()
            .filter(|c| !c.passed)
            .filter_map(|c| serde_json::to_value(c).ok())
            .collect()
    }
}
