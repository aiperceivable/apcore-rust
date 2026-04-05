# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **Note on versioning:** This crate starts at `0.13.0` rather than `0.1.0` to stay in sync
> with the [apcore-python](https://github.com/aiperceivable/apcore-python) and
> [apcore-typescript](https://github.com/aiperceivable/apcore-typescript) packages.
> All three SDKs implement the same protocol specification and share a unified version line.

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
