# apcore-rust — Examples

Runnable demos for the Rust SDK. Examples follow Cargo's convention: every top-level `examples/*.rs` file is auto-registered and runnable via `cargo run --example <name>`.

## Quick start

```bash
# From the apcore-rust repo root
cargo run --example simple_client
```

## All examples

| File | What it demonstrates | Run |
|---|---|---|
| [`simple_client.rs`](simple_client.rs) | Implement the `Module` trait directly and call `module.execute()` with a `Context`. | `cargo run --example simple_client` |
| [`global_client.rs`](global_client.rs) | Use the global `APCore` registry — register and call without an explicit client variable. | `cargo run --example global_client` |
| [`decorated_add.rs`](decorated_add.rs) | The `FunctionModule` adapter — turn a plain async fn into a registered module. | `cargo run --example decorated_add` |
| [`greet.rs`](greet.rs) | Minimal module: input/output schemas + `execute`. | `cargo run --example greet` |
| [`get_user.rs`](get_user.rs) | Read-only module annotation. | `cargo run --example get_user` |
| [`send_email.rs`](send_email.rs) | Full-featured module: `ModuleAnnotations`, `ModuleExample`, sensitive-field redaction. | `cargo run --example send_email` |
| [`cancel_token.rs`](cancel_token.rs) | Cooperative cancellation: cancel a long-running module via `CancelToken`. | `cargo run --example cancel_token` |
| [`pipeline_demo.rs`](pipeline_demo.rs) | The 11-step `ExecutionStrategy` pipeline — introspection, step-middleware tracing, and orchestration via `insert_after` / `replace`. See note below. | `cargo run --example pipeline_demo` |

### Bindings

The [`bindings/`](bindings/) directory shows the YAML-binding pattern:

| File | Role |
|---|---|
| [`bindings/format_date.binding.yaml`](bindings/format_date.binding.yaml) | Canonical binding definition. |
| [`bindings/format_date.rs`](bindings/format_date.rs) | Target function loaded by the binding. |

Because this file lives in a sub-directory, Cargo does not auto-register it as an example. To run it, add `[[example]] name = "format_date" path = "examples/bindings/format_date.rs"` to `Cargo.toml`, or copy the loader pattern from the file into your own program.

## Pipeline demo — what to look for

`pipeline_demo.rs` is the deep-dive into the engine. One run prints three sections:

1. **Introspection** — the canonical 11 step names from `strategy.step_names()` / `strategy.info()`.
2. **Middleware tracing** — a `StepMiddleware` that narrates every step of one call:
   ```
   [ 1/11] context_creation    — create execution context, set global deadline
           ✓   0.16 ms · caller=anonymous trace_id=…
   ...
   [11/11] return_result       — finalize and return output
           ✓   0.00 ms · returning {…}
   ```
3. **Orchestration** — `strategy.insert_after("output_validation", Box::new(AuditLogStep))?` adds a 12th step (rendered as `[  +  ]` to mark it as user-inserted), then `strategy.replace("audit_log", Box::new(QuietAuditLogStep))?` swaps the implementation while keeping the position.

The `[N/11]` numbering stays pinned to the protocol's 11 standard steps; custom steps appear as `[  +  ]`. This makes the "11 standard + N custom" composition unmistakable in the trace output.
