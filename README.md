<div align="center">
  <img src="https://raw.githubusercontent.com/aipartnerup/apcore/main/apcore-logo.svg" alt="apcore logo" width="200"/>
</div>

# apcore

![Rust](https://img.shields.io/badge/rust-1.75+-orange.svg)
![License](https://img.shields.io/badge/license-Apache%202.0-green.svg)

> **Build once, invoke by Code or AI.**

A schema-enforced module standard for the AI-Perceivable era.

**apcore** is an AI-Perceivable module standard that makes every interface naturally perceivable and understandable by AI through enforced Schema definitions and behavioral annotations. It provides strict type safety, access control, middleware pipelines, and built-in observability — enabling you to define modules with structured input/output schemas that are easily consumed by both code and AI.

## Features

- **Schema-driven modules** — Define input/output contracts using `schemars`-derived types with automatic validation
- **Execution Pipeline** — Context creation, safety checks, ACL enforcement, approval gate, validation, middleware chains, and execution with timeout support
- **`Module` trait** — Implement the `Module` trait to create fully schema-aware modules
- **YAML bindings** — Register modules declaratively without modifying source code
- **Access control (ACL)** — Pattern-based, first-match-wins rules with wildcard support
- **Middleware system** — Composable before/after hooks with error recovery
- **Observability** — Tracing (spans), metrics collection, and structured context logging
- **Async support** — Built on `tokio` for seamless async module execution
- **Safety guards** — Call depth limits, circular call detection, frequency throttling
- **Approval system** — Pluggable approval gate with async handlers, Phase B resume, and audit events
- **Extension points** — Unified extension management for discoverers, middleware, ACL, approval handlers, span exporters, and module validators
- **Async task management** — Background module execution with status tracking, cancellation, and concurrency limiting
- **Behavioral annotations** — Declare module traits (readonly, destructive, idempotent, cacheable, paginated, streaming) for AI-aware orchestration
- **W3C Trace Context** — `traceparent` header injection/extraction for distributed tracing interop

## API Overview

**Core**

| Type | Description |
|------|-------------|
| `APCore` | High-level client — register modules, call, stream, validate |
| `Registry` | Module storage — discover, register, get, list, watch |
| `Executor` | Execution engine — call with middleware pipeline, ACL, approval |
| `Context` | Request context — trace ID, identity, call chain, cancel token |
| `Config` | Configuration — from_defaults with env overrides, load YAML/JSON, get/set dot-path, validate, reload |
| `Identity` | Caller identity — id, type, roles, attributes |
| `Module` | Core trait for implementing schema-aware modules |

**Access Control & Approval**

| Type | Description |
|------|-------------|
| `ACL` | Access control — rule-based caller/target authorization |
| `ApprovalHandler` | Pluggable approval gate trait |
| `AlwaysDenyHandler` / `AutoApproveHandler` | Built-in approval handlers |

**Middleware**

| Type | Description |
|------|-------------|
| `Middleware` | Pipeline hooks — before/after/on_error interception |
| `BeforeMiddleware` / `AfterMiddleware` | Single-phase middleware adapters |
| `ObsLoggingMiddleware` | Structured logging middleware |
| `RetryMiddleware` | Automatic retry with backoff |
| `ErrorHistoryMiddleware` | Records errors into `ErrorHistory` |
| `PlatformNotifyMiddleware` | Emits events on error rate/latency spikes |

**Schema**

| Type | Description |
|------|-------------|
| `SchemaLoader` | Load schemas from YAML or native types |
| `SchemaValidator` | Validate data against schemas |
| `SchemaExporter` | Export schemas for MCP, OpenAI, Anthropic, generic |
| `RefResolver` | Resolve `$ref` references in JSON Schema |

**Observability**

| Type | Description |
|------|-------------|
| `TracingMiddleware` | Distributed tracing with span export |
| `MetricsMiddleware` / `MetricsCollector` | Call count, latency, error rate metrics |
| `ContextLogger` | Context-aware structured logging |
| `ErrorHistory` | Ring buffer of recent errors with deduplication |
| `UsageCollector` | Per-module usage statistics and trends |

**Events & Extensions**

| Type | Description |
|------|-------------|
| `EventEmitter` | Event system — subscribe, unsubscribe, emit, emit_filtered, flush |
| `WebhookSubscriber` | Built-in event subscriber |
| `ExtensionManager` | Unified extension point management |
| `AsyncTaskManager` | Background module execution with status tracking |
| `CancelToken` | Cooperative cancellation token |
| `BindingLoader` | Load modules from YAML binding files |

## Documentation

For full documentation, including Quick Start guides for Python and Rust, visit:
**[https://aipartnerup.github.io/apcore/getting-started.html](https://aipartnerup.github.io/apcore/getting-started.html)**

## Requirements

- Rust >= 1.75
- Tokio async runtime

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
apcore = "0.13"
tokio = { version = "1", features = ["full"] }
serde_json = "1"
```

## Quick Start

### Simple client

```rust
use apcore::APCore;
use apcore::module::Module;
use apcore::context::Context;
use serde_json::{json, Value};

struct AddModule;

#[async_trait::async_trait]
impl Module for AddModule {
    fn description(&self) -> &str { "Add two integers" }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {"a": {"type": "integer"}, "b": {"type": "integer"}}, "required": ["a", "b"]})
    }
    fn output_schema(&self) -> Value {
        json!({"type": "object", "properties": {"result": {"type": "integer"}}})
    }

    async fn execute(
        &self,
        _ctx: &Context<Value>,
        input: Value,
    ) -> Result<Value, apcore::errors::ModuleError> {
        let a = input["a"].as_i64().unwrap_or(0);
        let b = input["b"].as_i64().unwrap_or(0);
        Ok(json!({ "result": a + b }))
    }
}

#[tokio::main]
async fn main() {
    let mut client = APCore::new();
    client.register(Box::new(AddModule)).unwrap();

    let result = client
        .call("math.add", json!({"a": 10, "b": 5}), Default::default())
        .await
        .unwrap();
    println!("{}", result); // {"result": 15}
}
```

### With configuration

```rust
use apcore::{APCore, Config};
use std::path::Path;

#[tokio::main]
async fn main() {
    let config = Config::from_yaml_file(Path::new("apcore.yaml")).unwrap();
    let client = APCore::with_config(config);
}
```

### Module with typed schemas

```rust
use apcore::module::{Module, ModuleAnnotations};
use apcore::context::Context;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Serialize, Deserialize)]
struct GetUserInput {
    user_id: String,
}

#[derive(Serialize, Deserialize)]
struct GetUserOutput {
    id: String,
    name: String,
    email: String,
}

struct GetUserModule;

#[async_trait::async_trait]
impl Module for GetUserModule {
    fn description(&self) -> &str { "Get user details by ID" }

    // Annotations (readonly, idempotent, etc.) are set on
    // ModuleDescriptor when registering the module with the registry.

    async fn execute(
        &self,
        _ctx: &Context<Value>,
        input: Value,
    ) -> Result<Value, apcore::errors::ModuleError> {
        let req: GetUserInput = serde_json::from_value(input)?;
        let user = match req.user_id.as_str() {
            "user-1" => GetUserOutput { id: "user-1".into(), name: "Alice".into(), email: "alice@example.com".into() },
            "user-2" => GetUserOutput { id: "user-2".into(), name: "Bob".into(),   email: "bob@example.com".into() },
            id       => GetUserOutput { id: id.into(),       name: "Unknown".into(), email: "unknown@example.com".into() },
        };
        Ok(serde_json::to_value(user)?)
    }
}
```

### Add middleware

```rust
use apcore::observability::{ObsLoggingMiddleware, TracingMiddleware};

client.use_middleware(Box::new(ObsLoggingMiddleware::new()));
client.use_middleware(Box::new(TracingMiddleware::new()));
```

### Access control

```rust
use apcore::acl::{ACL, ACLRule};

let acl = ACL::new(vec![
    ACLRule::new(vec!["admin.*"], vec!["*"],       "allow", "Admins can call anything"),
    ACLRule::new(vec!["*"],       vec!["admin.*"], "deny",  "Others cannot call admin modules"),
]);
```

### YAML bindings

Register modules without touching Rust source — define a `binding.yaml`:

```yaml
bindings:
  - module_id: "utils.format_date"
    target: "format_date::format_date_string"
    description: "Format a date string into a specified format"
    tags: ["utility", "date"]
    version: "1.0.0"
    input_schema:
      type: object
      properties:
        date_string:   { type: string }
        output_format: { type: string }
      required: [date_string, output_format]
    output_schema:
      type: object
      properties:
        formatted: { type: string }
      required: [formatted]
```

Load it at runtime:

```rust
use apcore::bindings::BindingLoader;

let loader = BindingLoader::new();
loader.load_file("binding.yaml", &mut client).unwrap();
```

## Examples

The `examples/` directory contains runnable demos. Run any example with:

```bash
cargo run --example simple_client
cargo run --example greet
cargo run --example get_user
cargo run --example send_email
cargo run --example cancel_token
```

---

### `simple_client` — Implement `Module` and execute directly

Defines two modules (`AddModule`, `GreetModule`), builds an `Identity` + `Context`, and calls them directly without a registry.

```rust
use apcore::context::{Context, Identity};
use apcore::errors::ModuleError;
use apcore::module::Module;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;

struct AddModule;

#[async_trait]
impl Module for AddModule {
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "a": { "type": "integer" },
                "b": { "type": "integer" }
            },
            "required": ["a", "b"]
        })
    }
    fn output_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "result": { "type": "integer" } } })
    }
    fn description(&self) -> &str { "Add two integers" }

    async fn execute(&self, _ctx: &Context<Value>, input: Value) -> Result<Value, ModuleError> {
        let a = input["a"].as_i64().unwrap_or(0);
        let b = input["b"].as_i64().unwrap_or(0);
        Ok(json!({ "result": a + b }))
    }
}

#[tokio::main]
async fn main() {
    let identity = Identity {
        id: "user-1".to_string(),
        identity_type: "user".to_string(),
        roles: vec!["user".to_string()],
        attrs: HashMap::new(),
    };
    let ctx: Context<Value> = Context::new(identity);
    let module = AddModule;

    let result = module.execute(&ctx, json!({"a": 10, "b": 5})).await.unwrap();
    println!("{result}"); // {"result":15}
}
```

---

### `greet` — Typed input/output with `serde` and default field values

Uses `#[serde(default)]` for optional fields and shows schema introspection and validation error handling.

```rust
use apcore::context::{Context, Identity};
use apcore::errors::ModuleError;
use apcore::module::Module;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize)]
struct GreetInput {
    name: String,
    #[serde(default = "default_greeting")]
    greeting: String,
}
fn default_greeting() -> String { "Hello".to_string() }

#[derive(Debug, Serialize, Deserialize)]
struct GreetOutput { message: String }

struct GreetModule;

#[async_trait]
impl Module for GreetModule {
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name":     { "type": "string", "description": "Name of the person to greet" },
                "greeting": { "type": "string", "description": "Custom greeting prefix", "default": "Hello" }
            },
            "required": ["name"]
        })
    }
    fn output_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "message": { "type": "string" } }, "required": ["message"] })
    }
    fn description(&self) -> &str { "Greet a user by name" }

    async fn execute(&self, _ctx: &Context<Value>, input: Value) -> Result<Value, ModuleError> {
        let req: GreetInput = serde_json::from_value(input)
            .map_err(|e| ModuleError::new(apcore::errors::ErrorCode::GeneralInvalidInput, e.to_string()))?;
        Ok(serde_json::to_value(GreetOutput { message: format!("{}, {}!", req.greeting, req.name) }).unwrap())
    }
}

#[tokio::main]
async fn main() {
    let identity = Identity { id: "agent-1".to_string(), identity_type: "agent".to_string(), roles: vec![], attrs: HashMap::new() };
    let ctx: Context<Value> = Context::new(identity);
    let module = GreetModule;

    let out = module.execute(&ctx, json!({"name": "Alice", "greeting": "Good morning"})).await.unwrap();
    println!("{out}"); // {"message":"Good morning, Alice!"}

    let out = module.execute(&ctx, json!({"name": "Bob"})).await.unwrap();
    println!("{out}"); // {"message":"Hello, Bob!"}  ← default greeting applied

    // Schema introspection
    println!("{}", serde_json::to_string_pretty(&module.input_schema()).unwrap());

    // Missing required field → validation error
    let err = module.execute(&ctx, json!({"greeting": "Hi"})).await.unwrap_err();
    println!("Error: {err}");
}
```

---

### `get_user` — Readonly module with `ModuleAnnotations` and `ModuleExample`

Demonstrates behavioral annotations (`readonly`, `idempotent`), `ModuleExample` for AI-perceivable documentation, and looking up records by ID.

```rust
use apcore::module::{Module, ModuleAnnotations, ModuleExample};
// ...

fn get_user_annotations() -> ModuleAnnotations {
    ModuleAnnotations {
        readonly: true,
        idempotent: true,
        ..Default::default()
    }
}

fn get_user_examples() -> Vec<ModuleExample> {
    vec![ModuleExample {
        title: "Look up Alice".to_string(),
        description: Some("Returns Alice's profile".to_string()),
        inputs: json!({"user_id": "user-1"}),
        output: json!({"id": "user-1", "name": "Alice", "email": "alice@example.com"}),
    }]
}
```

```
user-1: {"email":"alice@example.com","id":"user-1","name":"Alice"}
user-2: {"email":"bob@example.com","id":"user-2","name":"Bob"}
user-999: {"email":"unknown@example.com","id":"user-999","name":"Unknown"}
```

---

### `send_email` — Destructive module with sensitive fields

Shows `x-sensitive: true` on schema fields (for log redaction), `ModuleAnnotations` with metadata, and behavioral annotation for destructive operations.

```rust
fn input_schema(&self) -> Value {
    json!({
        "type": "object",
        "properties": {
            "to":      { "type": "string" },
            "subject": { "type": "string" },
            "body":    { "type": "string" },
            "api_key": { "type": "string", "x-sensitive": true }  // redacted in logs
        },
        "required": ["to", "subject", "body", "api_key"]
    })
}
```

```rust
fn send_email_annotations() -> ModuleAnnotations {
    ModuleAnnotations {
        destructive: true,
        requires_approval: true,
        ..Default::default()
    }
}

fn send_email_examples() -> Vec<ModuleExample> {
    vec![ModuleExample {
        title: "Send a welcome email".to_string(),
        inputs: json!({ "to": "user@example.com", "subject": "Welcome!", "body": "...", "api_key": "sk-xxx" }),
        output: json!({ "status": "sent", "message_id": "msg-12345" }),
        ..Default::default()
    }]
}
```

---

### `cancel_token` — Cooperative cancellation during long-running execution

`CancelToken` is a cloneable, shared cancellation signal. Modules poll `token.is_cancelled()` between steps to stop early.

```rust
use apcore::cancel::CancelToken;

// Attach a token to the context
let mut ctx: Context<Value> = Context::new(identity);
let token = CancelToken::new();
ctx.cancel_token = Some(token.clone());

// Cancel from another task after 80 ms
tokio::spawn(async move {
    tokio::time::sleep(Duration::from_millis(80)).await;
    token.cancel();
});

// Module checks the token between steps
async fn execute(&self, ctx: &Context<Value>, input: Value) -> Result<Value, ModuleError> {
    for i in 0..steps {
        if let Some(t) = &ctx.cancel_token {
            if t.is_cancelled() {
                return Err(ModuleError::new(ErrorCode::ExecutionCancelled, format!("cancelled at step {i}")));
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(json!({ "completed_steps": steps }))
}
```

```
=== Run 1: normal execution ===
  [SlowModule] Executing step 0...
  [SlowModule] Executing step 1...
  [SlowModule] Executing step 2...
Result: {"completed_steps":3}

=== Run 2: cancelled mid-flight ===
  [SlowModule] Executing step 0...
  [SlowModule] Executing step 1...
  [main] Sending cancel signal…
  [SlowModule] Cancelled at step 2
Error (expected): Execution cancelled after 2 steps
```

## Tests

Run all tests:

```bash
cargo test
```

Run a specific test file:

```bash
cargo test --test test_cancel
cargo test --test test_errors
```

Run a specific test by name:

```bash
cargo test test_cancel_token
```

Run with output visible:

```bash
cargo test -- --nocapture
```

## Development

### Prerequisites

Install Rust via [rustup](https://rustup.rs):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Clone and build

```bash
git clone https://github.com/aipartnerup/apcore-rust.git
cd apcore-rust
cargo build
```

### Run tests

```bash
cargo test
```

### Run tests with output

```bash
cargo test -- --nocapture
```

### Run a specific test

```bash
cargo test test_cancel_token
```

### Lint and format

```bash
cargo fmt           # auto-format code
cargo clippy        # lint
```

### Build documentation

```bash
cargo doc --open
```

### Check without building

```bash
cargo check
```

## License

Apache-2.0

## Links

- **Documentation**: [https://aipartnerup.github.io/apcore/](https://aipartnerup.github.io/apcore/)
- **Website**: [aipartnerup.com](https://aipartnerup.com)
- **GitHub**: [aipartnerup/apcore-rust](https://github.com/aipartnerup/apcore-rust)
- **crates.io**: [apcore](https://crates.io/crates/apcore)
- **Issues**: [GitHub Issues](https://github.com/aipartnerup/apcore-rust/issues)
- **Discussions**: [GitHub Discussions](https://github.com/aipartnerup/apcore-rust/discussions)
