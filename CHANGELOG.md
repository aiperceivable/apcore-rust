# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **Note on versioning:** This crate starts at `0.13.0` rather than `0.1.0` to stay in sync
> with the [apcore-python](https://github.com/aiperceivable/apcore-python) and
> [apcore-typescript](https://github.com/aiperceivable/apcore-typescript) packages.
> All three SDKs implement the same protocol specification and share a unified version line.

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
