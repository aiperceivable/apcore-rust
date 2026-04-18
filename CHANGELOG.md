# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **Note on versioning:** This crate starts at `0.13.0` rather than `0.1.0` to stay in sync
> with the [apcore-python](https://github.com/aiperceivable/apcore-python) and
> [apcore-typescript](https://github.com/aiperceivable/apcore-typescript) packages.
> All three SDKs implement the same protocol specification and share a unified version line.

---

## [0.19.0] - 2026-04-19

### Added

- **`ErrorCode::DependencyVersionMismatch`** — new error code raised by `resolve_dependencies` when a declared `version` constraint is not satisfied by the registered version of the target module. `ModuleError` details include `module_id`, `dependency_id`, `required`, `actual`.
- **`resolve_dependencies(modules, known_ids, module_versions)`** — new third argument `Option<&HashMap<String, String>>` mapping `module_id → version`. When provided, declared dependency version constraints are enforced per PROTOCOL_SPEC §5.3. When absent, the `DepInfo.version` field is silently ignored.
- **Caret (`^`) and tilde (`~`) constraint support** in `matches_version_hint` / `select_best_version` (npm/Cargo semantics): `^1.2.3 → >=1.2.3,<2.0.0`, `^0.2.3 → >=0.2.3,<0.3.0`, `^0.0.3 → >=0.0.3,<0.0.4`, `~1.2.3 → >=1.2.3,<1.3.0`, `~1.2 → >=1.2.0,<1.3.0`, `~1 → >=1.0.0,<2.0.0`.
- **`TypedBindingHandler`** and **`typed_handler<I, O>()`** — Generic function that bundles an async handler with auto-derived JSON Schemas from `schemars::JsonSchema` trait bounds. When used with `register_into_with_typed_handlers()`, schemas from `schemars` are used for `auto_schema` bindings instead of the permissive `{"type":"object"}` fallback. No proc-macro crate needed. See DECLARATIVE_CONFIG_SPEC.md §6.5.
- **`auto_schema: true | permissive | strict`** — `AutoSchemaValue` enum accepts boolean or mode string. Strict mode reserved for OpenAI/Anthropic schema compliance (enforcement via `schemars` + post-processing tracked for 0.20.0).
- **New `ErrorCode` variants**: `BindingSchemaInferenceFailed`, `BindingSchemaModeConflict`, `BindingStrictSchemaIncompatible`, `BindingPolicyViolation`, `PipelineConfigInvalid`, `PipelineHandlerNotSupported`, `PipelineStepInsertionAmbiguous`, `EntryPointNotFound`, `EntryPointAmbiguous`, `EntryPointRuntimeUnsupported` (reserved). See DECLARATIVE_CONFIG_SPEC.md §7.1.
- **Pipeline `handler:` parse-time rejection** — `PipelineHandlerNotSupportedError` with remediation message pointing to `register_step_type()`. See DECLARATIVE_CONFIG_SPEC.md §4.4.
- **Pipeline metadata fields honored**: `match_modules`, `ignore_errors`, `pure`, `timeout_ms` now applied to resolved steps via `ConfiguredStep` wrapper. Previously silently dropped.
- **Pipeline `configure:` section** — Overlay metadata fields on existing built-in steps via `ExecutionStrategy::replace_with()`.
- **`ExecutionStrategy::replace_with(name, wrapper_fn)`** — Replace a step in-place by applying a wrapper function.
- **`schema_ref` loading implemented** — External YAML schema files referenced by `schema_ref` field now actually loaded and parsed (previously the field was stored but never processed).
- **`spec_version`** handling in binding YAML with deprecation warning when absent.
- **`schemars` dependency** (`0.8`) for JSON Schema generation from Rust types.
- **Cross-SDK conformance fixtures** in `apcore/conformance/fixtures/`.
- **Reintroduced `AsyncTaskManager`** (`src/async_task.rs`) — background task execution with `submit`, `get_status`, `get_result`, `cancel`, `list_tasks`, `cleanup`, `shutdown`; bounded by `max_concurrent` and `max_tasks`. 24 tests in `tests/test_async_task.rs`. Re-exported from crate root. Was temporarily removed in 0.18.0 pending `Executor` integration.
- **Reintroduced `ExtensionManager` / `ExtensionPoint`** (`src/extensions.rs`) — plugin registry with `register`, `get`, `get_all`, `unregister`, `list_points`, `apply`; supported extension points include `discoverer`, `module_validator`, `middleware`, `span_exporter`, `acl`, `approval_handler`. 21 tests in `tests/test_extensions.rs`. Re-exported from crate root. Was temporarily removed in 0.18.0.
- **`Context::builder()`** — New builder API supporting W3C trace_parent inheritance: `Context::builder().trace_parent(Option<TraceParent>).identity(id).services(s).build()`. The builder validates the incoming `trace_parent.trace_id` against `^[0-9a-f]{32}$` (rejecting all-zero and all-f per W3C), inheriting valid values verbatim and regenerating with `tracing::warn!` otherwise. See PROTOCOL_SPEC §10.5 `external_trace_parent_handling`. Existing `Context::new`, `Context::anonymous`, and `Context::create` constructors remain backward-compatible.

### Fixed

- **`resolve_dependencies` cycle path accuracy** — `extract_cycle` previously returned a phantom path (all remaining nodes plus the first one re-appended) when the arbitrarily-picked start node had no outgoing edge inside `remaining`. This could happen when a module is blocked on an external `known_ids` dependency while another subset contains a real cycle. Rewritten to DFS from each remaining node (sorted) and return a true back-edge cycle `[n0, ..., nk, n0]`; falls back to the sorted `remaining` set only when no back-edge exists.
- **`CircularDependencyError` now carries `cycle_path` in `ModuleError.details`** (as a JSON string array), matching the Python `details["cycle_path"]` / TypeScript `details.cyclePath` contract. Previously the path was only embedded in the message string, forcing downstream consumers to parse it.

### Changed

- **`Context` trace_id format** changed from 36-char UUID (with dashes) to **32-char lowercase hex** (aligned with W3C Trace Context `trace-id` field). Affects all internal constructors and external observability output. Downstream Jaeger/Tempo/Honeycomb/Datadog/OTLP consumers gain direct interoperability; the `TraceContext::inject()` dash-stripping workaround is now a no-op for freshly-created contexts but retained for backward compatibility with any persisted 36-char IDs.
- **`resolve_dependencies` signature** changed from `(modules, known_ids) -> Result<...>` to `(modules, known_ids, module_versions) -> Result<...>`. Pass `None` for `module_versions` to preserve prior behavior. All in-crate call sites updated.
- **Missing required dependencies now return `ErrorCode::DependencyNotFound` instead of `ErrorCode::ModuleLoadError`.** Brings Rust into compliance with PROTOCOL_SPEC §5.15.2. The error's `details` map now includes `module_id` and `dependency_id`. Upgrade path: match on `ErrorCode::DependencyNotFound` where you previously matched `ErrorCode::ModuleLoadError` for missing-dep cases.
- **Binding YAML format migrated to canonical** — Top-level `bindings:` list with `module_id` and string `target: "module:callable"`. Old format (`- name:` flat list, `target: {module_name, callable}`, `metadata:` wrapper) removed. See DECLARATIVE_CONFIG_SPEC.md §8.1 for migration guide.
- **`BindingDefinition` and `BindingTarget` removed** — Replaced by `BindingEntry` and `BindingsFile`. Public re-exports updated.
- **Handler-map key changed** from binding `name` to full `target` string (e.g., `"format_date:format_date_string"`).
- **`BindingSchemaMissing` ErrorCode variant deprecated** — Superseded by `BindingSchemaInferenceFailed`. Kept for backward-compatible deserialization.
- **`description`, `documentation`, `tags`, `version`, `annotations`, `display`, `metadata`** now parsed from top-level binding entry fields (previously some were nested under `metadata`).

## [0.18.1] - 2026-04-16

### Changed

- **`ModuleDescriptor` unified with the cross-language protocol shape.** The slim Rust-only descriptor and the auxiliary `FullModuleDescriptor` (previously in `src/registry/types.rs`) have been merged into a single `ModuleDescriptor` in `apcore::registry` that matches `apcore-python.ModuleDescriptor` and `apcore-typescript.ModuleDescriptor` field-for-field. Changes relative to v0.18.0:
  - `name: String` (previously the canonical module ID) is now `name: Option<String>` (human-readable display name).
  - New required field `module_id: String` carries the canonical identifier that used to live in `name`.
  - New optional fields: `description`, `documentation`, `version` (default `"1.0.0"`), `examples`, `metadata`, `sunset_date`.
  - `annotations: ModuleAnnotations` is now `annotations: Option<ModuleAnnotations>` (matches Python `None` / TS `null`).
  - `enabled: bool` (Rust-only runtime toggle) is kept but marked `#[serde(skip_serializing)]` so it never leaks into cross-language wire payloads; it still deserializes with a default of `true`.
  - `FullModuleDescriptor` is removed from the public API. All callers should use `ModuleDescriptor`.

  **Migration**: callers constructing `ModuleDescriptor` literals must rename `name` to `module_id`, wrap `annotations` in `Some(...)`, and supply the new fields (all have sensible defaults — `description: String::new()`, `documentation: None`, `version: "1.0.0".into()`, `examples: vec![]`, `metadata: HashMap::new()`, `sunset_date: None`).

### Fixed

- Clippy `unnecessary_map_or` warnings in `builtin_steps.rs`, `executor.rs`, and `sys_modules/manifest.rs` (11 sites) — replaced `.map_or(false, |a| a.field)` with `.is_some_and(|a| a.field)`.

---

## [0.18.0] - 2026-04-15

### Added

- **`APCore::from_path()` factory method** — Ergonomic shorthand: `APCore::from_path("apcore.yaml")?` is equivalent to `let config = Config::load("apcore.yaml")?; APCore::with_config(config)`. Returns `Result<APCore, ModuleError>`. Existing `APCore::with_config()` usage is unchanged.
- **`pub const MAX_MODULE_ID_LENGTH: usize = 192`** in `apcore::registry::registry`, re-exported from `apcore::registry` and the crate root (`apcore::MAX_MODULE_ID_LENGTH`). Tracks PROTOCOL_SPEC §2.7 EBNF constraint #1 and aligns with `apcore-python` / `apcore-typescript`.
- **`Registry::register` now enforces module ID length** per PROTOCOL_SPEC §2.7. Module IDs longer than `MAX_MODULE_ID_LENGTH` are rejected with `ErrorCode::GeneralInvalidInput` carrying the message `"Module ID exceeds maximum length of {N}: {actual}"`. **This was a previously undetected spec compliance gap** — the constraint is `MUST` in the protocol but the Rust SDK never validated it. Python and TypeScript SDKs have always enforced it.
- **`module_id_pattern()` function** returning `&'static Regex` (lazy `OnceLock<Regex>`) for the canonical EBNF pattern. Re-exported at the crate root as `apcore::module_id_pattern`.
- **`REGISTRY_EVENTS` constants** — `pub mod registry_events { pub const REGISTER, UNREGISTER }`, `pub struct RegistryEvents` with associated consts, and the `pub const REGISTRY_EVENTS: RegistryEvents` singleton. Closes a §12.2 MUST violation: all SDKs must export these event names as named constants. Aligned with apcore-python (`REGISTRY_EVENTS` dict) and apcore-typescript (`REGISTRY_EVENTS` frozen object).
- **Crate-root re-exports for parity with apcore-python and apcore-typescript:** `MiddlewareManager`, `Middleware`, `BeforeMiddleware`, `AfterMiddleware`, `LoggingMiddleware`, `RetryMiddleware`, `RetryConfig`, `PlatformNotifyMiddleware`, `ErrorHistoryMiddleware`, `MetricsMiddleware`, `UsageMiddleware`, `ObsLoggingMiddleware`, `ErrorFormatterRegistry`, `ErrorFormatter`, `build_minimal_strategy`, `BindingLoader`, `BindingDefinition`, `BindingTarget`, `CancelToken`, `FunctionModule`, `ErrorHistory`, `ErrorEntry`, `MetricsCollector`, `UsageCollector`, `UsageStats`, `Span`, `SpanExporter`, `StdoutExporter`, `InMemoryExporter`, `OTLPExporter`, `SchemaLoader`, `SchemaValidator`, `SchemaExporter`, `RefResolver`, `TraceContext`, `TraceParent`. All previously required `apcore::module_path::*` access; now reachable directly from `apcore::*`. Note: `Extension`, `ExtensionManager`, `ExtensionPoint`, `AsyncTaskManager`, and `TaskInfo` are **not** re-exported — see the "Cross-Language Feature Parity" note in the README.
- **`Registry::register_internal` now enforces empty / EBNF pattern / length / duplicate checks** via the shared `validate_module_id()` helper (was previously bypassing all validation). The reserved-word check is the only step skipped (so sys modules can use the `system.*` prefix). Aligned with apcore-typescript `registerInternal()`.
- **Boundary tests** in `tests/test_registry.rs`: `test_max_module_id_length_matches_spec`, `test_register_accepts_module_id_at_max_length`, `test_register_rejects_module_id_exceeding_max_length`, plus 6 `test_register_internal_*` parity tests.
- **`tests/test_crate_root_exports.rs`** — 7 regression tests asserting that every spec-required and Python/TS-parity symbol is reachable from `apcore::*`.
- **`test_validate_accepts_optional_context`** regression test in `tests/test_executor.rs`.
- **`TraceContext::inject()` and `TraceContext::extract()`** — W3C trace context propagation utilities, aligned with apcore-python and apcore-typescript. `inject` serializes a `Context`'s trace ID into a `traceparent` header map; `extract` parses a `traceparent` header back into a `TraceParent`. Includes 8 unit tests.
- **`Executor::register_strategy()` and `Executor::list_strategies()` associated functions** — Delegates to existing module-level functions. Spec places these on Executor; aligned with apcore-python (classmethod/instance method) and apcore-typescript (static methods).

### Changed

- **`Executor::describe_pipeline()` now returns `StrategyInfo` instead of `String`** — Provides structured access to pipeline metadata (`name`, `step_count`, `step_names`, `description`). `StrategyInfo` implements `Display` for `.to_string()` backward compatibility. Aligned with apcore-typescript `describePipeline() -> StrategyInfo` and apcore-python `describe_pipeline() -> StrategyInfo`.

- **`ACL::check()` and `ACL::async_check()` consolidated** via three shared private helpers (`finalize_no_rules`, `finalize_rule_match`, `finalize_default_effect`). Audit-entry construction, debug-logging, and default-effect mapping now live in exactly one place (was duplicated across sync and async paths). Added `check_conditions_async` helper so `matches_rule_async` no longer inlines conditions extraction. Aligned with apcore-python `_finalize_check` helper pattern.
- **README documents annotation overlay cross-SDK difference** — New section explaining that Rust does not implement YAML annotation overlays, with rationale (spec §4.13 is conditional, Rust favors explicit code annotations) and a serde workaround for users who need it.
- **`Executor::validate()` signature gained an optional context parameter** — `pub async fn validate(&self, module_id: &str, inputs: &Value, ctx: Option<&Context<Value>>) -> Result<PreflightResult, ModuleError>`. Aligns with PROTOCOL_SPEC §12.2 line 6405 and matches apcore-python / apcore-typescript. When `None` is passed, an anonymous `@external` context is synthesized internally for backward compatibility (existing behavior preserved). When a real context is passed, call-chain checks (depth limit, circular call detection) and ACL caller-identity matching see real caller state. **This is a source-incompatible change for any code calling `executor.validate(id, inputs)` — add a third `None` argument.**
- **`Registry::register` and `Registry::register_internal` duplicate-detection error code changed from `ErrorCode::ModuleLoadError` to `ErrorCode::GeneralInvalidInput`.** Aligns with apcore-python / apcore-typescript which use `InvalidInputError` (`GENERAL_INVALID_INPUT`) for the same condition. `ModuleLoadError` is reserved for actual module load failures (file I/O, parse errors); a duplicate ID is invalid input from the caller. **Clients catching errors by code in Rust will see a different code than before** — update any `match` arms.
- **Duplicate-registration error message canonicalized** to `"Module ID '<name>' is already registered"` (was `"Module '<name>' is already registered"` — added the "ID" word). Both `register()` and `register_internal()` emit the same string. Now byte-identical to apcore-python and apcore-typescript.
- **README installation snippet bumped from `apcore = "0.16"` to `apcore = "0.18"`** — was 2 minor versions stale and would have given new users a broken install of the v0.16 surface.

### Compatibility (in addition to the BREAKING items below)

- **`Executor::validate()` is source-incompatible** for callers that passed only two arguments — add `None` as the third argument. The semantics with `None` are identical to the previous two-arg behavior.
- **Duplicate-error consumers in Rust must update error-code matches** from `ErrorCode::ModuleLoadError` to `ErrorCode::GeneralInvalidInput` for the registration-duplicate path.
- **`register_internal()` is source-compatible** but stricter at runtime. Existing in-tree callers (`apcore::sys_modules::*`) all use canonical-shape IDs and are unaffected. External adapter authors who used `register_internal()` as a generic escape hatch for non-canonical IDs should review.
- **New rejection path for over-length IDs.** Code that previously registered module IDs longer than 192 characters in Rust will now fail at `register()`. Python and TypeScript already rejected such IDs at the previous 128-char threshold; no consistent cross-language behavior was possible before this fix.

### Changed (BREAKING)

- **`Config` struct restructured to namespaced form per PROTOCOL_SPEC §9.1.** Executor and observability settings now live under nested sub-structs `ExecutorConfig` and `ObservabilityConfig` instead of being flat fields on `Config`. The mapping is:

  | Before (≤ 0.17.x)              | After (0.18.0+)                          |
  |--------------------------------|------------------------------------------|
  | `config.max_call_depth`        | `config.executor.max_call_depth`         |
  | `config.max_module_repeat`     | `config.executor.max_module_repeat`      |
  | `config.default_timeout_ms`    | `config.executor.default_timeout`        |
  | `config.global_timeout_ms`     | `config.executor.global_timeout`         |
  | `config.enable_tracing`        | `config.observability.tracing.enabled`   |
  | `config.enable_metrics`        | `config.observability.metrics.enabled`   |
  | `config.settings`              | `config.user_namespaces`                 |

  The `_ms` suffix is dropped from timeout fields to align with spec §9.1 and the Python/TypeScript SDKs (units stay milliseconds; documented in field doc comments).

  **Wire format (YAML/JSON) is also breaking.** Producers MUST use the canonical nested form:

  ```yaml
  executor:
    max_call_depth: 32
    default_timeout: 30000
  observability:
    tracing:
      enabled: true
  ```

  Loading a v0.17.x-style config with root-level `max_call_depth`, `default_timeout_ms`, etc. now produces a hard error pointing at `MIGRATION-v0.18.md`. There is no silent migration. See the migration guide for the rationale.

- **`Config::get()` / `Config::set()` no longer accept legacy bare field names** — `config.get("max_call_depth")` returns `None` in v0.18.0; use `config.get("executor.max_call_depth")` instead. Cross-language parity with Python/TypeScript.

- **`Config::bind()` now special-cases canonical namespaces** — `config.bind::<ExecutorConfig>("executor")` returns the typed sub-struct directly without going through `user_namespaces`.

### Fixed

- **README `Identity` struct literal replaced with `Identity::new()` constructor.** The greet example used a struct literal `Identity { id: ..., ... }` but Identity fields are private — the correct constructor is `Identity::new(id, identity_type, roles, attrs)`.
- **`Config` no longer silently ignores spec-conformant YAML.** A YAML file using the canonical `executor: { max_call_depth: 100 }` shape would previously be captured into the unused `settings` HashMap and the typed `max_call_depth` field would remain at default 32. Discovered during the v0.18.0 cross-language audit.

- **`ModuleAnnotations.extra` wire format aligned with PROTOCOL_SPEC §4.4.1** — Removed `#[serde(flatten)]` on the `extra` field. The struct now serializes `extra` as a nested `"extra"` object, matching `apcore-python` and `apcore-typescript`. This fixes a silent cross-language data-loss bug where Python/TypeScript payloads carrying nested `extra` would deserialize on the Rust side as `extra["extra"] = {...}` (one level too deep). The custom `Deserialize` impl tolerates legacy flattened input from `apcore-rust ≤ 0.17.1` for one MINOR backward-compat cycle. When the same key appears in both forms, the nested value wins per spec rule 7.

### Changed

- **`ModuleAnnotations` Deserialize is now hand-rolled** — Replaced `#[derive(Deserialize)]` with a custom `Visitor` to support both nested and legacy flattened wire forms with deterministic precedence. The public API of the struct is unchanged; only the on-the-wire format is corrected.

### Added

- **`ExecutorConfig`, `ObservabilityConfig`, `TracingConfig`, `MetricsConfig`** — New public sub-structs for canonical namespace binding. Available via `apcore::config::*` and re-exported from the crate root.

### Removed

- **`Config.settings` field** → renamed to `Config.user_namespaces` to clarify intent (it captures user-defined namespaces only, not canonical ones).
- **`default_true` and `default_pagination_style` private helpers** in `module.rs` — No longer needed now that `Deserialize` for `ModuleAnnotations` is custom; defaults flow through `Default::default()`.
- **`AsyncTaskManager`** and **`ExtensionManager` / `ExtensionPoint`** — The Rust implementations were non-functional stubs and have been removed from the crate root and all re-export paths. Python and TypeScript SDKs retain working implementations. See the [Cross-Language Feature Parity](./README.md#cross-language-feature-parity) section of the README for the tracked gap and reintroduction plan.

## [0.17.1] - 2026-04-06

### Added

- **`build_minimal_strategy()`** — 4-step pipeline (context → lookup → execute → return) for pre-validated internal hot paths.
- **`resolve_strategy_by_name()`** — Resolves preset strategy names (`"standard"`, `"internal"`, `"testing"`, `"performance"`, `"minimal"`) to `ExecutionStrategy` instances. Cross-language parity with Python/TypeScript string-based resolution.
- **`Executor::with_strategy_name()`** — Constructor accepting a strategy name string instead of an `ExecutionStrategy` instance.
- **`requires()` / `provides()` on `Step` trait** — Optional advisory methods declaring step dependencies. `ExecutionStrategy` validates dependency chains at construction and insertion, emitting `tracing::warn!` for unmet requirements.
- **`Module::stream()` / `Module::supports_stream()`** — Default trait methods enabling streaming module execution. Returns `Option<Result<Vec<Value>>>` — `None` signals fallback to `execute()`.

### Fixed

- **`BuiltinExecute` global deadline clamp** — Effective timeout is now `min(default_timeout_ms, remaining_global_deadline)`, matching Python/TypeScript dual-timeout model. Returns `ModuleTimeout` immediately if global deadline already exceeded.
- **Streaming support** — `Executor::stream()` now implements the three-phase streaming protocol (pipeline → chunk collection → post-stream validation) instead of wrapping `call()` in a `vec![]`.

---

## [0.17.0] - 2026-04-05

### Added

- **Step Metadata**: Four default trait methods on `Step`: `match_modules()`, `ignore_errors()`, `pure()`, `timeout_ms()` with sensible defaults.
- **YAML Pipeline Configuration**: `register_step_type()`, `unregister_step_type()`, `registered_step_types()`, `build_strategy_from_config()` in new `pipeline_config` module. Uses `OnceLock<RwLock<HashMap>>` global registry.
- **PipelineContext fields**: `dry_run`, `version_hint`, `executed_middlewares`, plus executor resource injection (`registry`, `config`, `acl`, `approval_handler`, `middleware_manager` as `Arc`).
- **StepTrace**: `skip_reason: Option<String>`.
- **Builtin steps with real execution logic**: All 11 steps now contain production-grade logic (was macro-generated no-ops).

### Changed

- **Step order**: `BuiltinMiddlewareBefore` now runs BEFORE `BuiltinInputValidation`.
- **Executor delegation**: `call()` and `validate()` fully delegate to `PipelineEngine::run()`. Removed ~740 lines of inline step code. `strategy` field is now non-optional (`ExecutionStrategy`, not `Option<ExecutionStrategy>`).
- **Renamed**: `safety_check` → `call_chain_guard`, `BuiltinSafetyCheck` → `BuiltinCallChainGuard`.
- **Registry**: Module storage changed from `Box<dyn Module>` to `Arc<dyn Module>` for sharing with pipeline context.

### Fixed

- Middleware input transforms were never validated against schema.
- `validate()` now uses pipeline dry-run mode.

---

## [0.16.0] - 2026-04-05

### Added

- **Config Bus**: `EnvStyle` enum (Auto/Nested/Flat), `max_depth`, `env_prefix` auto-derivation, `env_map: Option<HashMap<String, String>>`, `Config::env_map()`, `ConfigEnvMapConflict` error code.
- **Context**: `ContextKey<T>` with `Cow<'static, str>` for zero-alloc static keys and `scoped()` for per-module sub-keys. Built-in key constants. `Context.serialize()`/`deserialize()` with `_context_version: 1`.
- **Annotations**: `extra: HashMap<String, Value>` with `#[serde(flatten)]` for unknown key capture.
- **ACL**: `ACLConditionHandler` async trait. `ACL::register_condition()` with global `RwLock` registry. `$or`/`$not` compound operators. `async_check()` returning `Result<bool, ModuleError>`. Fail-closed for unknown conditions.
- **Pipeline**: `Step` async trait, `StepResult`, `PipelineContext`, `PipelineTrace`, `ExecutionStrategy`, `PipelineEngine`. 11 `BuiltinStep` structs (via macro). Preset strategies (standard/internal/testing/performance). `Executor::with_strategy()`, `call_with_trace()`, `describe_pipeline()`.

### Changed

- `system.control` module extracted into dedicated `control.rs` file.

### Fixed

- **`ApprovalRequest` spec alignment** — Added required `context: Option<Context<Value>>` field and changed `annotations` from `HashMap<String, Value>` to `ModuleAnnotations` per spec §7.3.1.
- **`DependencyInfo` field rename** — Renamed `name` to `module_id` for cross-SDK consistency with Python/TypeScript.
- **Config env fallback path** — Fixed namespace-mode `APCORE_*` env var fallback to resolve to top-level dot-paths instead of incorrectly prepending `apcore.` namespace prefix.
- **`config_env` conformance test** — Added missing `config_env.json` conformance test (was 9/10, now 10/10 fixtures).
- Removed non-spec Context fields: `created_at`, `parent_trace_id`, `trace_context`.
- `global_deadline` changed from `Option<Instant>` to `Option<f64>` (epoch seconds).
- `Identity` fields made private with pub getters (`id()`, `identity_type()`, `roles()`, `attrs()`). Serde compat via `IdentityRaw` deserialization pattern.
- Empty `callers` list in ACL rules now matches none (aligned with Python/TypeScript).

---

## [0.15.1] - 2026-03-31

### Changed

- **Env prefix convention simplified** — Removed the `^APCORE_[A-Z0-9]` reservation rule from `Config::register_namespace()`. Sub-packages now use single-underscore prefixes (`APCORE_MCP`, `APCORE_OBSERVABILITY`, `APCORE_SYS`) instead of the double-underscore form. Only the exact `APCORE` prefix is reserved for the core namespace.
- Built-in namespace env prefixes: `APCORE__OBSERVABILITY` → `APCORE_OBSERVABILITY`, `APCORE__SYS` → `APCORE_SYS`.

---

## [0.14.0] - 2026-03-24

### Breaking Changes
- Middleware default priority changed from `0` to `100` per PROTOCOL_SPEC §11.2. Middleware without explicit priority will now execute before priority-0 middleware.
- `use_middleware()` now returns `Result<(), ModuleError>` (previously returned nothing)
- Metric names changed: `apcore_calls_total` → `apcore_module_calls_total`, `apcore_errors_total` → `apcore_module_errors_total`, `apcore_duration_seconds` → `apcore_module_duration_seconds`

### Added
- **Middleware priority** — `Middleware` trait now has `fn priority(&self) -> u16` (default 0). Higher priority executes first; equal priority preserves registration order.
- **Input validation (Step 6)** — JSON Schema validation of inputs against `module.input_schema()` using `jsonschema` crate
- **Output validation (Step 9)** — JSON Schema validation of outputs against `module.output_schema()`
- **Dual-timeout enforcement** — `global_timeout_ms` now propagated via `Context.global_deadline`; effective timeout is `min(per_module, remaining_global)`
- **Approval error differentiation** — `rejected` → `ApprovalDenied`, `timeout` → `ApprovalTimeout`, `pending` → `ApprovalPending` (previously all mapped to `ApprovalDenied`)
- **`_approval_token` Phase B** — Token stripped from inputs, `check_approval()` called instead of `request_approval()`; non-string tokens rejected with error
- **Sensitive field redaction** — `redact_sensitive()` function handles `x-sensitive` schema fields and `_secret_` prefix keys; populates `context.redacted_inputs`
- **`LoggingMiddleware`** — New middleware (priority 700) with configurable `log_inputs`/`log_outputs`/`log_errors` flags, duration tracking, and redacted input support
- **`ContextLogger` `_secret_` redaction** — Keys prefixed with `_secret_` are now redacted in JSON log output
- **Priority range validation** — `add()` returns `Result` and rejects priority > 1000

### Fixed
- **TracingMiddleware rewrite** — Replaced `HashMap<String, Span>` with stack-based `Vec<Span>` per trace_id for correct nested module-to-module parent-child span linking; merged dual mutexes into single `TraceState` to eliminate TOCTOU race
- **`increment_errors` signature** — Added `error_code` parameter to match Python/TypeScript/spec
- **Sampling strategy naming** — Added explicit serde renames: `Always` → `"full"`, `Probabilistic` → `"proportional"`, `ErrorFirst` → `"error_first"`, `Never` → `"off"` to match cross-language convention
- **Preflight in validate()** — `validate()` now calls `module.preflight()` and returns `ValidationResult` with warnings (diagnostic, non-blocking), matching Python behavior
- **Metric names** — Renamed `apcore_calls_total` → `apcore_module_calls_total`, `apcore_errors_total` → `apcore_module_errors_total`, `apcore_duration_seconds` → `apcore_module_duration_seconds` to match cross-language convention
- **Step numbering** — Fixed duplicate step numbers in `call()` and `validate()` executor methods

## [0.13.1] - 2026-03-22

### Changed
- Rebrand: aipartnerup → aiperceivable

## [0.13.0] - 2026-03-12

Initial Rust release. Implements the full apcore protocol specification in Rust,
feature-aligned with `apcore-python` 0.13.0.

### Added

#### Core
- **`Module` trait** — Async `execute` with `input_schema` / `output_schema`, `description`, `annotations`, `preflight`
- **`ModuleAnnotations`** — Behavioral metadata: `readonly`, `destructive`, `idempotent`, `cacheable`, `cache_ttl`, `cache_key_fields`, `paginated`, `pagination_style`, `sunset_date`, `tags`, `examples`, `metadata`
- **`ModuleExample`** — Named input/output pair for AI-perceivable documentation
- **`APCore`** client — `register`, `unregister`, `call`, `stream`, `use_middleware`
- **`Config`** — Load from YAML / JSON file, `get` / `set` values
- **`Context<T>`** — Request context with `trace_id`, `identity`, `call_chain`, `cancel_token`, `metadata`
- **`ContextFactory`** — Builder for execution contexts
- **`Identity`** — Caller identity with `id`, `name`, `roles`, `attributes`
- **`Executor`** — Execution engine with middleware pipeline, ACL enforcement, approval gate, call-depth guard, timeout

#### Access Control & Approval
- **`ACL`** — Pattern-based, first-match-wins rules with wildcard support
- **`ACLRule`** — Rule entry with caller patterns, target patterns, effect (`allow`/`deny`), and priority
- **`ApprovalHandler`** trait — Pluggable async approval gate
- **`AutoApproveHandler`** / **`AlwaysDenyHandler`** / **`CallbackApprovalHandler`** (planned) — Built-in handlers
- **`ApprovalRequest`** / **`ApprovalResult`** — Request/response types for the approval pipeline

#### Middleware
- **`Middleware`** trait — `before` / `after` / `on_error` pipeline hooks
- **`BeforeMiddleware`** / **`AfterMiddleware`** — Single-phase adapter types
- **`MiddlewareManager`** — Ordered middleware chain execution
- **`ObsLoggingMiddleware`** — Structured context-aware logging
- **`RetryMiddleware`** — Automatic retry with configurable backoff
- **`ErrorHistoryMiddleware`** — Records errors into `ErrorHistory` ring buffer
- **`PlatformNotifyMiddleware`** — Emits events on error-rate / latency threshold breaches

#### Observability
- **`TracingMiddleware`** — Distributed tracing with span lifecycle and pluggable `SpanExporter`
- **`Span`** / **`SpanExporter`** trait — W3C-compatible span model
- **`StdoutExporter`** / **`InMemoryExporter`** — Built-in exporters
- **`MetricsCollector`** / **`MetricsMiddleware`** — Call count, latency, and error-rate metrics
- **`ContextLogger`** — Context-aware structured log sink (`info`, `warn`, `error`)
- **`ObsLoggingMiddleware`** — Middleware wrapper around `ContextLogger`
- **`ErrorHistory`** — Fixed-capacity ring buffer with per-error-code querying
- **`UsageCollector`** / **`UsageMiddleware`** — Per-module call statistics and hourly trend data

#### Schema
- **`SchemaLoader`** — Load schemas from YAML files or inline `serde_json::Value`
- **`SchemaValidator`** — Validate data against JSON Schema (strict / lenient modes)
- **`SchemaExporter`** — Export schemas for MCP, OpenAI, Anthropic, and generic targets via `ExportProfile`
- **`RefResolver`** — Resolve `$ref` references in JSON Schema documents

#### Registry
- **`Registry`** — Module storage with `register`, `unregister`, `get`, `list`, `watch`
- **`ModuleDescriptor`** — Metadata envelope: id, version, tags, source path, `sunset_date`
- **`Discoverer`** trait — Pluggable module discovery backends

#### Events & Extensions
- **`EventEmitter`** — Async event bus with pattern-based subscribe / emit / flush
- **`ApCoreEvent`** — Typed event (module lifecycle, errors, config changes)
- **`WebhookSubscriber`** / **`A2ASubscriber`** (planned) — Built-in event delivery subscribers
- **`ExtensionManager`** — Unified extension point registry for discoverers, middleware, ACL, approval, exporters, and validators

#### Async Tasks & Cancellation
- **`AsyncTaskManager`** — Background module execution with status tracking, cancellation, and concurrency limiting
- **`TaskInfo`** / **`TaskStatus`** — Task lifecycle state machine
- **`CancelToken`** — Cloneable, shared cooperative cancellation signal

#### Bindings & Utilities
- **`BindingLoader`** — Declarative YAML module registration without modifying source code
- **`BindingDefinition`** — Schema + metadata for a YAML-bound module
- **`TraceParent`** / **`TraceContext`** — W3C `traceparent` header injection and extraction for distributed tracing interop
- **`ErrorCode`** enum — 37 variants covering the full protocol error taxonomy
- **`ModuleError`** — Structured error with code, message, and optional cause chain

#### Developer Experience
- 8 integration test files covering `CancelToken`, `Identity`, `Context`, `Module` trait, `ACL`, `Registry`, `TraceContext`, `ErrorCode`
- 5 runnable examples: `simple_client`, `greet`, `get_user`, `send_email`, `cancel_token`
