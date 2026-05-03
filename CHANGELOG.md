# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **Note on versioning:** This crate starts at `0.13.0` rather than `0.1.0` to stay in sync
> with the [apcore-python](https://github.com/aiperceivable/apcore-python) and
> [apcore-typescript](https://github.com/aiperceivable/apcore-typescript) packages.
> All three SDKs implement the same protocol specification and share a unified version line.

---

## [Unreleased]

### Added

- `UsageExporter` async trait + `NoopUsageExporter` + `PeriodicUsageExporter` for push-style usage summary export (#45 §3, parity with PY+TS).

### Changed

- `DEFAULT_SENSITIVE_KEYS` expanded to canonical 16-entry superset matching Python+TS (#43 §5, D-54).

### Cross-SDK Sync Alignment

#### Added

- **`OverridesStore` trait + `InMemoryOverridesStore` / `FileOverridesStore`
  reference impls** (sync finding CRITICAL #1). The runtime overrides layer
  is now a pluggable async trait with `load()` and `save()` methods, allowing
  callers to swap in custom backends (Redis, S3, in-memory test fakes).
  `SysModulesOptions::overrides_store: Option<Arc<dyn OverridesStore>>` is
  threaded through `register_sys_modules_with_options` and wired into
  `UpdateConfigModule` / `ToggleFeatureModule`. When set, the store takes
  precedence over the legacy `overrides_path`. Aligns with `apcore-python`
  and `apcore-typescript`. Re-exported from the crate root as
  `OverridesStore`, `InMemoryOverridesStore`, `FileOverridesStore`,
  `OverridesError`.
- **`RetryConfig::compute_delay_ms`** (sync finding CRITICAL #2 / D-08).
  Canonical name for the retry-delay calculation, matching PY/TS. The legacy
  `delay_for_attempt` is retained as a `#[deprecated(since = "0.21.0")]`
  alias that delegates to `compute_delay_ms`; it will be removed in the next
  minor.
- **`TraceContext::inject_checked`** (sync finding W-6). Validating variant
  of `inject_with_options` that returns `ErrorCode::GeneralInvalidInput` when
  a caller-supplied `parent_id` does not match `^[0-9a-f]{16}$`, instead of
  silently falling back to a random value. Matches PY/TS behaviour.
- **`TRACE_FLAGS_KEY` constant** (sync finding CRITICAL #3). Public string
  constant `"_apcore.trace.flags"` that names the context-data slot used for
  inbound `trace-flags` propagation.
- **`ErrorCode::ConfigurationError`** (sync finding W-7). Distinct error
  code for structural pipeline-configuration errors (missing-step in
  `remove`/`configure`, missing `after`/`before` anchor) — keeps these cases
  disambiguated from `PipelineDependencyError` (reserved for `requires` /
  `provides` graph violations).

#### Changed

- **`TraceContext::inject` propagates inbound `trace-flags`** (sync finding
  CRITICAL #3). When `context.data` carries a value at `TRACE_FLAGS_KEY`
  (e.g. `"00"` or `"01"`), `inject` uses that as the outbound `trace-flags`
  byte instead of always emitting `0x01`. The default of `0x01` (sampled) is
  preserved when no inbound flags are present. `ContextBuilder::build` seeds
  the key automatically when a `trace_parent` is supplied, so transports
  that build contexts via the canonical builder get propagation for free.
  Matches `apcore-python._TRACE_FLAGS_KEY` semantics.
- **Pipeline configuration errors use `ConfigurationError`** instead of
  `PipelineConfigInvalid` for missing-step / missing-anchor cases (sync
  finding W-7). Tests that previously matched `PipelineConfigInvalid` should
  also accept `ConfigurationError`.

### Event-Naming Standardization & Contextual Auditing

#### Changed

- **Canonical event names (Issue #36).** The four threshold / registry
  events that previously emitted under bare names now also emit under their
  canonical `apcore.<subsystem>.<event>` names. Both names are dispatched on
  every occurrence so existing subscribers continue to fire while consumers
  migrate; the legacy events carry `deprecated: true` and a
  `canonical_event` pointer in their payload.
  | Legacy (still emitted)         | Canonical                                       |
  | ------------------------------ | ----------------------------------------------- |
  | `module_registered`            | `apcore.registry.module_registered`             |
  | `module_unregistered`          | `apcore.registry.module_unregistered`           |
  | `error_threshold_exceeded`     | `apcore.health.error_threshold_exceeded`        |
  | `latency_threshold_exceeded`   | `apcore.health.latency_threshold_exceeded`      |
- **Registry hooks now emit `ApCoreEvent`s.** `module_registered` /
  `module_unregistered` were previously logged via `tracing::info!` only;
  they are now full `ApCoreEvent`s dispatched through the `EventEmitter`,
  so subscribers can pattern-match against `apcore.registry.*` (Issue #36).
- **Audit-event payloads include caller identity (Issue #45.2).** The
  `apcore.config.updated`, `apcore.module.toggled`, and
  `apcore.module.reloaded` events now embed `caller_id` (defaulted to
  `"@external"` when absent) and `actor_id` / `actor_type` extracted from
  the `Context`. Aligns the Rust SDK with `apcore-python` and
  `apcore-typescript`'s contextual-audit behaviour.

### Pipeline StepMiddleware + fail-fast configuration (Issue #33)

#### Added

- **`StepMiddleware` trait** — formal step-scoped interceptor with default-method
  async hooks `before_step`, `after_step`, and `on_step_error`. Multiple
  middlewares run in registration order in the before phase and may recover
  from a step failure by returning `Ok(Some(value))` from `on_step_error`.
  Register via `ExecutionStrategy::add_step_middleware(Arc::new(...))`. Mirrors
  apcore-python `step_middleware` (Issue #33 §2.2). The trait is re-exported
  from the crate root.
- **`ErrorCode::PipelineDependencyError`** — new error variant returned from
  `ExecutionStrategy::new` / `insert_after` / `insert_before` when a step's
  declared `requires` field is not produced by any preceding step's `provides`
  (Issue #33 §2.1). Cross-language: Python/TS `PIPELINE_DEPENDENCY_ERROR`.

#### Changed

- **`ExecutionStrategy` dependency validation** is now **fail-fast at
  construction** rather than emitting a `tracing::warn!`. Strategies with
  unmet `requires`/`provides` declarations now return
  `Err(PipelineDependencyError)` from the constructor. Pipelines that already
  satisfy their declarations are unaffected (Issue #33 §2.1).
- **`build_strategy_from_config`** now returns
  `Err(PipelineConfigInvalid)` when the YAML `pipeline.remove`,
  `pipeline.configure`, or `pipeline.steps` section references a step that
  does not exist or omits both `after`/`before` anchors. Previously these
  conditions logged a warning and silently dropped the directive (Issue #33
  §1.2). This is a behaviour change: configurations that previously
  surfaced only a log warning will now refuse to construct the strategy.

### Cross-Language Sync — Storage Backend & Multi-Alignment Fixes

This batch introduces the protocol-canonical `StorageBackend` abstraction
(Issue #43 §1), wires it through the three observability collectors as an
optional persistence surface, and resolves five additional cross-language
alignment findings (D-14, D-19, D-25, D-27, D-28). Behavior of the streaming
chunk-merge path now surfaces a structured `STREAM_CHUNK_NOT_OBJECT` error,
the runtime config-key policy emits a distinct `CONFIG_KEY_RESTRICTED`
ErrorCode, and `ContextLogger`/`ObsLoggingMiddleware` align on lowercase
levels, nested-`extra` schema, and the `module_id` / `inputs` field names.

#### Added

- **`observability::storage::StorageBackend`** trait + `InMemoryStorageBackend`
  default — namespace/key/value persistence surface for cross-process
  durability, per observability.md §1 (Issue #43 §1). The default in-memory
  implementation is thread-safe (`RwLock`-guarded), namespace-isolated, and
  treats `delete` as idempotent. Re-exported from the crate root.
- **`ErrorHistory::with_storage_backend(per_module, total, backend)`** and
  **`ErrorHistory::with_storage(backend)`** — optional `StorageBackend`
  wiring; every recorded `ErrorEntry` is also persisted under namespace
  `"error_history"` keyed by fingerprint when the backend is supplied.
- **`UsageCollector::with_storage_backend(backend)`** — same pattern; usage
  records persist under namespace `"usage"`.
- **`MetricsCollector::with_storage_backend(backend)` / `.with_storage(...)`**
  — metric points persist under namespace `"metrics"` with a key derived
  from `(name, timestamp)`.
- **`UsageCollector::record_at(...)`** and
  **`UsageCollector::get_summary_for_period(period)`** (D-27) — `record_at`
  honors an explicit `DateTime<Utc>` timestamp, and `get_summary_for_period`
  filters records by recency. Trend is now derived from current-vs-previous
  sample counts (`stable` / `rising` / `declining` / `new` / `inactive`),
  matching `apcore-python` and `apcore-typescript` exactly.
- **`executor::deep_merge_chunks_checked(chunks)`** (D-19) — public helper
  that merges streaming chunks while enforcing each chunk is a JSON object;
  a non-object chunk yields `ModuleError::GeneralInvalidInput` with
  `details["code"] = "STREAM_CHUNK_NOT_OBJECT"` (cross-language: Python's
  `_deep_merge` AttributeError, TypeScript's TypeError). The streaming
  pipeline now uses this checked helper in Phase 3.
- **`ErrorCode::ConfigKeyRestricted`** (D-25) — distinct wire-format
  `CONFIG_KEY_RESTRICTED` code so callers can match the policy-deny case
  separately from value-shape errors. Emitted by
  `system.control.update_config` for keys in `RESTRICTED_KEYS`.
- **`ContextLogger::set_writer(...)`** — substitute the output sink (default
  stderr) so tests can capture emitted records.

#### Changed

- **`RetryConfig::default().max_retries`** is now `0` (was `3`) (D-14) —
  retries are explicitly opt-in, matching `apcore-python` and
  `apcore-typescript`. Behavior change: callers that were relying on the
  silent `3` default must now opt in by setting `max_retries` explicitly.
- **`ContextLogger`** JSON output schema (D-28):
  * `level` field is now lowercase (`"info"`), not uppercase (`"INFO"`).
  * User-supplied extras are nested under a single `"extra"` object instead
    of flattened to the top level.
- **`ObsLoggingMiddleware`** extras (D-28):
  * `module_id` (was `module`).
  * `inputs` (was `input`).
- **`UsageRecord.timestamp`** is now `DateTime<Utc>` (was `String`); the
  field was crate-private so the public surface is unaffected.

### Reload & Observability Hardening (Rust)

#### Added

- Granular reload via `path_filter` input in `ReloadModule` (#45.4).
- `Config::reload_from_disk()` for refreshing static config without binary restart (#45.5).
- Error fingerprinting in `ErrorHistory` — dedup by (error_code, top-frame hash, sanitized template) (#43 §4).
- Configurable redaction via `obs.redaction.regex_patterns` / `obs.redaction.sensitive_keys` Config keys (#43 §5).

### Cross-Language Sync — Review-Mode Hardening

This release applies the next batch of cross-language audit findings, focused
on ACL TOCTOU correctness, middleware closure ergonomics, event-emitter
fire-and-forget semantics, and documentation cleanup.

#### Added

- **`ACL::try_new`** is now the only fallible constructor; **`ACL::new`**
  panics on invalid `default_effect`, matching the constructor-throws
  behaviour of apcore-python and apcore-typescript (sync finding A-D-302).
  YAML loading still surfaces validation failures as `Result` via
  `ACL::load`.
- **`middleware::adapters::BeforeAdapter` / `AfterAdapter`** — closure-based
  middleware adapters that implement the `Middleware` trait, allowing
  before-only and after-only async closures to be registered directly via
  `MiddlewareManager::add(Box::new(BeforeAdapter::new(...)))` without
  defining a struct (sync finding A-D-402).
- **`EventEmitter::emit_spawn`** — fire-and-forget event dispatch that
  spawns one `tokio::task` per matching subscriber and returns
  immediately. Use this for the canonical fire-and-forget path; the
  existing `emit` method remains for sequential / deterministic test
  ordering (sync finding A-D-501).
- **`EventEmitter::shutdown` / `is_shutdown`** — idempotent shutdown
  that flushes pending work and turns subsequent `emit` / `emit_spawn`
  calls into no-ops (sync finding A-D-502).
- **`TraceContext::inject_with_options`** — extended W3C inject API
  accepting an optional 16-hex `parent_id` override, an optional
  propagated `trace_flags` byte, and an optional `tracestate` slice.
  When a tracestate is supplied (and non-empty) it is emitted as the
  `tracestate` header alongside `traceparent`. Invalid `parent_id`
  values fall back to a freshly generated random span ID. The
  zero-argument `TraceContext::inject` retains its existing public
  signature and is now a thin shim over `inject_with_options`
  (issue #35).
- **`TraceContext::extract_context`** — returns a full `TraceContext`
  parsed from an inbound header map, populating `tracestate` from
  the `tracestate` header per W3C §3.3.1 (comma-separated, capped at
  32 entries, malformed entries silently dropped) (issue #35).

#### Fixed

- **`TraceContext::extract` case-insensitive header KEY lookup**
  (issue #35) — RFC 7230 §3.2 requires HTTP header field names to be
  treated case-insensitively. `extract` and `extract_context` now
  match `traceparent` / `tracestate` header keys regardless of map
  key casing (`traceparent`, `Traceparent`, `TRACEPARENT`, etc.) via
  a new internal `lookup_header_ci` helper. Previously the `extract`
  path required exact lowercase keys.

#### Changed

- **`APCore::disable` / `APCore::enable`** signatures are
  `async fn (&self, name: &str, reason: Option<&str>) -> Result<Value, ModuleError>`,
  routing through `system.control.toggle_feature` and returning a status
  payload. **This is a breaking change** relative to pre-0.20 releases
  that exposed `disable(&mut self, name, reason) -> Result<(), ModuleError>`;
  cross-language parity is now restored with apcore-python and
  apcore-typescript (sync finding A-003).
- **`ACL::async_check`** now snapshots `rules` and `default_effect` at
  entry, eliminating a TOCTOU race where a concurrent `add_rule` /
  `reload` could mutate the rule list mid-evaluation. Mirrors the sync
  `check()` snapshot and matches apcore-python's `_snapshot()` and
  apcore-typescript's `rules.slice()` (sync finding A-D-301).
- **`ACL::reload`** restructured so the `&mut self` borrow on
  `self.yaml_path` ends *before* the blocking file read in `load()`,
  closing a deadlock window when the ACL is held in an
  `Arc<RwLock<ACL>>` wrapper and a reader hits the lock concurrently
  (sync finding A-D-303).
- **`EventEmitter` internal storage** changed from
  `Vec<Box<dyn EventSubscriber>>` to `Vec<Arc<dyn EventSubscriber>>` so
  subscribers can be cloned into `tokio::spawn` tasks for `emit_spawn`.
  No public-API surface change — `subscribe(Box<dyn ...>)` continues to
  work via `Arc::from`.
- **`CancelToken::check`** return type changed from
  `Result<(), ModuleError>` to `Result<(), ExecutionCancelledError>`
  (sync finding CANCEL-001). The typed variant matches Python's
  `ExecutionCancelledError(ModuleError)` subclass and TypeScript's
  `ExecutionCancelledError extends ModuleError` hierarchy, letting
  callers `match` on cancellation specifically. A `From<ExecutionCancelledError>
  for ModuleError` impl plus an `ExecutionCancelledError::to_module_error()`
  helper are provided for ergonomic widening, so most callers (`?` against
  a `ModuleError` context) need no changes. A new
  `CancelToken::check_for(&str)` method preserves the caller-supplied
  module ID in the typed error.

#### Documentation

- **`MiddlewareManager::remove`** doc clarified: name-based, not
  identity-based. Two middlewares registered with the same name cannot
  be removed independently (sync finding A-D-401).
- **README install pin** bumped from `apcore = "0.19"` to
  `apcore = "0.20"` (sync finding B-001).

---

## [0.20.0] - 2026-04-30

### System Modules Hardening (Issue #45, system-modules.md §1.1–§1.5)

Implements the cross-language System Modules Hardening normative rules:
overrides persistence, contextual audit trail, Prometheus exporter for the
UsageCollector, glob-based bulk reload, and a strict registration entry
point. Aligns Rust with the `apcore-python` reference implementation.

#### Added

- **`sys_modules::audit`** — `AuditAction`, `AuditChange`, `AuditEntry`, the
  async `AuditStore` trait, and `InMemoryAuditStore`. Every state-changing
  control call (`update_config`, `reload_module`, `toggle_feature`) records
  an entry with `actor_id` / `actor_type` extracted from `context.identity`
  and the call's `trace_id`. When no store is configured, entries are logged
  at INFO level and discarded.
- **`sys_modules::overrides`** — `load_overrides` and `write_override` for
  YAML-backed runtime override persistence. Writes use a per-path lock and
  tempfile + rename to avoid partial-write corruption. Loaded after the
  base `Config`, so manual restores never erase runtime overrides.
- **`UpdateConfigModule::with_overrides_path` / `with_audit_store`,
  `ToggleFeatureModule::with_overrides_path` / `with_audit_store`,
  `ReloadModule::with_audit_store`** — opt-in builders for §1.1/§1.2.
- **Sensitive-key redaction in `UpdateConfigModule`** — `old_value` / `new_value`
  in the response payload, the `apcore.config.updated` event payload, and the
  `AuditChange` are replaced with `***REDACTED***` whenever the key matches a
  sensitive segment (`token`, `secret`, `key`, `password`, `auth`, `credential`).
  The in-memory `Config` still holds the real value — redaction is for egress
  only. Aligned with `apcore-python` (`utils/redaction.REDACTED_VALUE`).
- **Misconfiguration warning in `register_sys_modules_with_options`** — when
  `overrides_path` or `audit_store` is set but `sys_modules.events.enabled=false`,
  a `WARN`-level tracing event flags that control modules are not registered
  and the options have no effect.
- **`ReloadModule` `path_filter` input** — accepts a glob pattern and
  reloads every matching module in dependency-topological order (leaves
  first). `module_id` and `path_filter` are mutually exclusive — providing
  both raises `ErrorCode::ModuleReloadConflict`.
- **`UsageCollector::export_prometheus`** — emits
  `apcore_usage_calls_total{module_id,status}` (counter),
  `apcore_usage_error_rate{module_id}` (gauge), and
  `apcore_usage_p50/p95/p99_latency_ms{module_id}` (gauges).
  `PrometheusExporter::with_usage_collector` wires the new metrics into
  the existing `/metrics` endpoint.
- **`SysModulesOptions`** + **`register_sys_modules_with_options`** — passes
  `overrides_path`, `audit_store`, and `fail_on_error` into the registration
  flow without breaking the simpler 4-arg call site.
- **`SysModuleError`** — `RegistrationFailed { module_id, source }` returned
  from `register_sys_modules` when `fail_on_error` is `true`.
- **`ErrorCode::ModuleReloadConflict`** (`MODULE_RELOAD_CONFLICT`) and
  **`ErrorCode::SysModuleRegistrationFailed`** (`SYS_MODULE_REGISTRATION_FAILED`).
- **`tests/test_system_modules_hardening_conformance.rs`** — 10/10 cases from
  `apcore/conformance/fixtures/system_modules_hardening.json`.

#### Changed (BREAKING)

- **`register_sys_modules` signature** — now returns
  `Result<SysModulesContext, SysModuleError>` instead of
  `Option<SysModulesContext>`. When `sys_modules.enabled` is `false`, the
  function returns `Ok(SysModulesContext { … empty … })`. Callers that
  previously matched on `Option::Some` / `None` must switch to `Result`.
  `client::APCore::with_options` updated to log and continue on failure,
  preserving lenient default behavior.

---

### Middleware Architecture Hardening (Issue #42, middleware-system.md §1.x)

Implements the cross-language Middleware Architecture Hardening normative
rules: context-data namespace partitioning, the `CircuitBreakerMiddleware`
state machine, the OpenTelemetry-compatible `TracingMiddleware`, and the
YAML-driven middleware chain configuration.

#### Added

- **`middleware::context_namespace`** — `ContextWriter`, `validate_context_key`,
  and `enforce_context_key` helpers enforcing the `_apcore.*` (framework) /
  `ext.*` (user) prefix rules. Canonical key constants exposed via
  `middleware::namespace_keys` (`LOGGING_START_TIME`, `TRACING_SPAN_ID`,
  `CIRCUIT_STATE`).
- **`middleware::circuit_breaker::CircuitBreakerMiddleware`** — per-`(module_id,
  caller_id)` rolling-window breaker with `CLOSED → OPEN → HALF_OPEN → CLOSED`
  state machine. Emits `apcore.circuit.opened` and `apcore.circuit.closed`
  via an injected `Arc<EventEmitter>`. Writes `CLOSED` / `OPEN` / `HALF_OPEN`
  into `context.data["_apcore.mw.circuit.state"]` on every call.
- **`middleware::otel_tracing::TracingMiddleware`** (OTel-compatible) — opens
  a logical span on `before()` with attributes `apcore.trace_id`,
  `apcore.caller_id`, `apcore.module_id`, writes the span id under
  `_apcore.mw.tracing.span_id`, and records lifecycle status (`ok` / `error`)
  on `after()` / `on_error()`. Gated by the new compile-time `opentelemetry`
  feature with a runtime `enabled(bool)` builder override; silent no-op when
  disabled.
- **`ErrorCode::CircuitBreakerOpen`** — serialized as `CIRCUIT_OPEN`.
  Constructor: `ModuleError::circuit_breaker_open(module_id, caller_id)`.
- **`middleware::yaml_config`** — declarative middleware chain config:
  `MiddlewareConfig` enum (`Tracing`, `CircuitBreaker`, `Logging`, `Custom`),
  `MiddlewareChainConfig::from_yaml` / `from_json`, and `MiddlewareFactory`
  with custom-handler registration and optional event-emitter injection.
- **`tests/test_middleware_hardening_conformance.rs`** — 10/10 cases from
  `apcore/conformance/fixtures/middleware_hardening.json`.

#### Notes

- Async-handler detection is satisfied statically by Rust's type system; the
  conformance test asserts compile-time witness via the `Middleware` trait.
- The pre-existing `observability::tracing_middleware::TracingMiddleware`
  (span-exporter based) is unchanged. The new OTel-compatible middleware is
  re-exported at the crate root as `OtelTracingMiddleware` to avoid a name
  collision.

### Multi-Module Discovery (Issue #32, PROTOCOL_SPEC §2.1.1, multi-module-discovery.md)

Adds opt-in multi-class discovery: multiple `Module` implementations may
coexist in a single source file, each receiving an ID of the form
`base_id.snake_case(struct_name)`. Off by default — single-class files are
unaffected and produce identical IDs regardless of whether the feature is
enabled (single-class identity guarantee).

#### Added

- **`registry::multi_class` module** — new module hosting the cross-language
  ID-derivation primitives.
- **`DiscoveryConfig { multi_class: bool }`** — opt-in flag (default `false`).
- **`class_name_to_segment(&str) -> String`** — snake_case conversion
  algorithm aligned with `apcore-python.class_name_to_segment`. Handles
  `Addition` → `addition`, `MathOps` → `math_ops`, `HTTPSender` →
  `http_sender`, `MyModule_V2` → `my_module_v2`.
- **`compute_base_id(&Path, &str) -> String`** — Algorithm A01 base ID
  derivation from file path + extensions root.
- **`derive_module_ids(&Path, &str, &[DiscoveredClass], &DiscoveryConfig)`** —
  pure ID-derivation function returning the list of derived IDs (or
  `MODULE_ID_CONFLICT` / `INVALID_SEGMENT` / `ID_TOO_LONG` errors).
- **`DiscoveredClass`** struct (`name`, `implements_module`) for the
  conformance-fixture interface.
- **`MultiClassEntry`** struct + **`Registry::register_multi_class()`** — the
  user-facing registration helper. Atomic registration: if any per-module
  registration fails, already-registered modules from the batch are rolled
  back so the file is registered all-or-nothing.
- **`ErrorCode::ModuleIdConflict`** (`MODULE_ID_CONFLICT`) — two or more
  classes in the same file produce the same `class_segment` after snake_case
  conversion. Details carry `file_path`, `class_names`, and
  `conflicting_segment`.
- **`ErrorCode::InvalidSegment`** (`INVALID_SEGMENT`) — derived segment does
  not conform to the canonical ID grammar.
- **`ErrorCode::IdTooLong`** (`ID_TOO_LONG`) — full derived `module_id`
  exceeds `MAX_MODULE_ID_LEN` (192).
- **`ModuleError::module_id_conflict()`**, **`invalid_segment()`**,
  **`id_too_long()`** builders.
- **`MAX_MODULE_ID_LEN: usize = 192`** constant in the multi_class module
  (mirrors the existing `MAX_MODULE_ID_LENGTH` in the registry module for
  cross-SDK naming consistency).
- **Cross-language conformance tests** for all eight Issue #32 fixture cases
  (`single_class_id_unchanged`, `two_classes_distinct_ids`,
  `class_name_snake_case_addition`, `class_name_snake_case_math_ops`,
  `class_name_snake_case_https_sender`, `conflict_same_segment`,
  `full_id_grammar_valid`, `disabled_by_default`) in
  `tests/test_multi_module_discovery_conformance.rs`.

#### Notes

- **Rust integration model**: Rust has no runtime reflection, so
  multi-class discovery cannot enumerate `impl Module for X` at scan time
  the way Python `inspect.getmembers` does. Module authors register a list
  of `(class_name, instance)` pairs explicitly via
  `Registry::register_multi_class`. The pure ID-derivation logic is shared
  with the conformance fixture so all three SDKs validate against the same
  test cases.
- The single-class identity guarantee applies regardless of
  `multi_class` mode: a file with exactly one qualifying class always
  receives the bare `base_id` (no `.class_segment` suffix). This preserves
  all existing module IDs.
- Multi-class disabled with multiple classes in a file: the file is
  treated as single-class — only the first qualifying class is loaded
  under `base_id`. Mirrors the `disabled_by_default` fixture case and
  apcore-python policy.

### Pipeline Architecture Hardening (Issue #33, core-executor.md §Pipeline Hardening)

This release adds the cross-SDK pipeline hardening primitives required by
Issue #33. Public APIs (`Executor::call`, `Executor::validate`,
`Executor::stream`) preserve their existing typed errors — `PipelineEngine`
wraps step failures in `PipelineStepError` internally and the executor
unwraps before returning, mirroring the apcore-python reference.

#### Added

- **`ErrorCode::PipelineStepError`** (`PIPELINE_STEP_ERROR`) — fail-fast wrapper
  carrying the failing step's name and the original `ModuleError` cause.
- **`ErrorCode::PipelineStepNotFound`** (`PIPELINE_STEP_NOT_FOUND`) — surfaced by
  `ExecutionStrategy::configure_step` when the target step does not exist.
- **`ModuleError::pipeline_step_error(step_name, &cause)`** builder, plus the
  `is_pipeline_step_error()`, `step_name()`, and `unwrap_pipeline_step_error()`
  accessors for inspecting / unwrapping wrapped errors.
- **`ExecutionStrategy::configure_step(name, step)`** — replace-semantic that is
  idempotent and preserves the step's position in the execution order (§1.2).
- **`ExecutionStrategy::name_to_idx()`** — exposes the maintained
  `HashMap<String, usize>` so the O(1) lookup is observable per §1.5. The map
  is rebuilt after every mutation (`new`, `insert_after`, `insert_before`,
  `remove`, `replace`, `replace_with`, `configure_step`).
- **`PipelineState`** — snapshot type passed to `run_until` predicates, carrying
  `step_name`, `outputs`, and a borrowed reference to the live `PipelineContext`.
- **`RunUntilPredicate`** type alias and **`RunOptions`** struct for the new
  `PipelineEngine::run_with_options` entry point.
- **`PipelineEngine::run_until(strategy, ctx, predicate)`** — predicate-based
  termination per §1.4. Evaluated after each step's clean continue; returning
  `true` halts the pipeline and reports success.
- **Cross-language conformance tests** for the five Issue #33 fixtures
  (`fail_fast_on_step_error`, `continue_on_ignored_error`,
  `replace_semantic_no_duplicate`, `run_until_stops_early`,
  `step_lookup_is_not_linear`) in
  `tests/test_pipeline_hardening_conformance.rs`.

#### Changed

- **`PipelineEngine` step-error behavior** — when a step returns `Err` and its
  `ignore_errors` is `false`, the engine now wraps the error in a
  `PipelineStepError` (§1.1). `Executor::call`, `Executor::validate`, and
  `Executor::stream` unwrap before returning so user-visible error codes are
  unchanged. Callers that drive `PipelineEngine::run` directly will observe the
  wrapped code and should call `unwrap_pipeline_step_error()` for the cause.
- **`PipelineEngine::run_until` previously took `stop_before_step: &str`** for
  the streaming pre-execute phase. That method is renamed to
  **`run_until_step`**; the `run_until` name now hosts the spec-conformant
  predicate-based API. The two streaming callers in `executor.rs` and
  `pipeline.rs` were migrated.
- **`skip_to` lookup** in the engine now uses `ExecutionStrategy::name_to_idx`
  (O(1)) and explicitly rejects same-position / backward targets to prevent
  infinite loops.

#### Notes

- **§1.3 step-level middleware** is `SHOULD` in the spec, has no conformance
  fixture, and is not yet implemented in apcore-python. Per the
  reference-implementation alignment policy (apcore-python is canonical),
  this is deferred — to be revisited once Python lands the API.

### Schema System Hardening (Issue #44, PROTOCOL_SPEC §4.15)

This release replaces the hand-written schema validator with the `jsonschema`
crate (Draft 2020-12) and adds the cross-SDK hardening primitives required by
Issue #44. All previously passing schema tests continue to pass.

### Added

- **`jsonschema = "0.28"` Draft 2020-12 backend** — `SchemaValidator` now wraps
  `jsonschema::Validator` and gains complete support for `anyOf` / `oneOf` /
  `allOf` / `not`, recursive `$ref` (e.g. self-referencing TreeNode schemas via
  `"$ref": "#"`), and all numerical / string constraints (`minimum`, `maximum`,
  `exclusiveMinimum`, `minLength`, `maxLength`, `pattern`).
- **`SchemaValidator::validate_detailed`** — new structured-result variant that
  returns mapped error codes (`SchemaUnionNoMatch`, `SchemaUnionAmbiguous`,
  `SchemaValidationError`) and non-fatal format warnings.
- **`schema::content_hash(&Value) -> String`** — SHA-256 hex digest of the
  canonical (sorted-keys) JSON form of a schema. Two byte-equivalent schemas
  hash to the same digest, satisfying the cross-SDK deduplication invariant.
- **Content-addressable compile cache** on `SchemaValidator` — repeated
  validation against the same schema (or a key-reordered copy) compiles the
  schema exactly once. `cache_len()` and `clear_cache()` accessors included.
- **`schema::format_warnings(&Value, &Value)`** — opt-in semantic format check
  for `date-time`, `date`, `time`, `email`, `uri`, `uuid`, `ipv4`, `ipv6`.
  Format enforcement is SHOULD-level: invalid values produce a warning rather
  than failing validation, matching the Python and TypeScript SDKs.
- **New `ErrorCode` variants**: `SchemaUnionNoMatch` (`SCHEMA_UNION_NO_MATCH`),
  `SchemaUnionAmbiguous` (`SCHEMA_UNION_AMBIGUOUS`), `SchemaMaxDepthExceeded`
  (`SCHEMA_MAX_DEPTH_EXCEEDED`).
- **`sha2 = "0.10"`** dependency for the canonical-JSON content hash.
- **Cross-language conformance tests** for the five new fixtures
  (`schema_hardening_union`, `schema_hardening_recursive`,
  `schema_hardening_constraints`, `schema_hardening_formats`,
  `schema_hardening_cache`) in `tests/test_schema_hardening_conformance.rs`.

### Changed

- **`SchemaValidator` is no longer a unit struct.** It now owns an internal
  `Arc<Mutex<HashMap<String, Arc<jsonschema::Validator>>>>` compile cache.
  Existing constructors (`SchemaValidator::new()`,
  `SchemaValidator::default()`) and the `validate` / `validate_or_error`
  methods remain source-compatible. Code that relied on the unit-struct form
  (`SchemaValidator;`) must switch to `SchemaValidator::default()` or
  `SchemaValidator::new()`.

### AsyncTask Evolution (Issue #34, async-tasks.md §AsyncTaskManager Evolution)

Adds three capability extensions to `AsyncTaskManager`: a pluggable
`TaskStore` trait, configurable retry with exponential backoff, and an
opt-in TTL-based Reaper background task. The pre-existing 3-arg
`AsyncTaskManager::new(executor, max_concurrent, max_tasks)` constructor is
preserved; it now defaults to the new `InMemoryTaskStore` so existing
callers and tests are unaffected.

#### Added

- **`async_task::TaskStore`** trait (`#[async_trait]`) — pluggable storage
  backend with `save / get / list / delete / list_expired` and
  `store_type_name`. Decouples task state from in-process memory and enables
  distributed deployments / persistence across process restarts. Ships with
  `InMemoryTaskStore` (default, `dashmap`-backed) — third-party backends
  (`RedisTaskStore`, `SqlTaskStore`) live in downstream crates.
- **`AsyncTaskManager::with_store(executor, max_concurrent, max_tasks, store)`** —
  constructor accepting a caller-provided `Arc<dyn TaskStore>`.
- **`AsyncTaskManager::store_type_name()`** — exposes the active backend's
  identifier for tooling / introspection.
- **`AsyncTaskManager::store()`** — returns a clone of the underlying
  `Arc<dyn TaskStore>` for direct interaction.
- **`async_task::RetryConfig` { `max_retries`, `retry_delay_ms`,
  `backoff_multiplier`, `max_retry_delay_ms` }** — per-task retry policy.
  `delay_for_attempt(attempt)` computes
  `min(retry_delay_ms * (backoff_multiplier ^ attempt), max_retry_delay_ms)`.
  Not re-exported at the crate root to avoid colliding with
  `middleware::RetryConfig` — import via `apcore::async_task::RetryConfig`.
- **`AsyncTaskManager::submit_with_retry(module_id, inputs, ctx, retry)`** —
  submission variant that accepts an optional retry policy. On failure the
  task is rescheduled with `tokio::time::sleep` and `status` returns to
  `Pending` until `max_retries` is exhausted, after which it transitions to
  `Failed` with `error` populated.
- **`async_task::ReaperConfig` { `ttl_seconds`, `sweep_interval_ms`}** and
  **`async_task::ReaperHandle`** — opt-in background reaper.
  `AsyncTaskManager::start_reaper(ReaperConfig)` returns a handle; calling
  `handle.stop().await` signals graceful shutdown via a
  `tokio::sync::watch` channel and awaits the join. The sweep calls
  `store.list_expired(now - ttl)` which only returns terminal-state tasks,
  so pending and running tasks are never deleted by the reaper.
- **`async_task::AsyncTaskManager::get_status_async` / `get_result_async`** —
  async variants of the synchronous facade methods, intended for callers
  with network-backed `TaskStore` implementations.
- **`TaskInfo.retry_count` and `TaskInfo.max_retries`** fields (both
  `#[serde(default)]`, so wire-format compatibility is preserved).
- **`dashmap = "6"`** dependency (used internally by `InMemoryTaskStore` for
  lock-free concurrent access).
- **`tests/test_async_task_evolution_conformance.rs`** — 10/10 cases from
  `apcore/conformance/fixtures/async_task_evolution.json` plus a smoke test
  for the `ReaperHandle` lifecycle.

#### Notes

- The synchronous facade (`submit`, `cancel`, `cleanup`, `shutdown`,
  `list_tasks`, `get_status`, `get_result`, `task_count`) is preserved and
  internally drives the async `TaskStore` through a single-poll no-op-waker
  helper. Custom `TaskStore` implementations whose futures actually yield
  MUST use the `_async` variants — the synchronous facade panics if a future
  returns `Pending`.
- Concurrent `submit_with_retry` calls are serialised through an
  `admission_lock` so the `len() < max_tasks` check and the subsequent
  `save` are atomic — without this, two racing submits could both pass the
  cap check and exceed `max_tasks`.
- Cross-language alignment: `RETRYING` is not a separate `TaskStatus` in the
  Rust SDK (the cross-language spec lifecycle pins five states); a task
  awaiting its next retry attempt stays in `Pending`. `started_at` is set
  on the first execution and preserved across retries to match Python.

## [0.19.1] - 2026-04-27

### Added

- **`Registry::export_schema_strict(name, strict)`** — Adds a strict-mode variant of `Registry::export_schema` that returns the full schema envelope (`module_id`, `description`, `input_schema`, `output_schema`) with strict-mode transformation applied when `strict=true` (sets `additionalProperties:false` on objects, marks all properties required, rewrites optional fields as nullable). This aligns the Rust SDK with `apcore-python` and `apcore-typescript` `Registry` interfaces for MCP compatibility.

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
