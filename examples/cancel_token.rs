//! CancelToken example — cooperative cancellation during long-running execution.

use apcore::cancel::CancelToken;
use apcore::context::{Context, Identity};
use apcore::errors::ModuleError;
use apcore::module::Module;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// A module that respects cancellation
// ---------------------------------------------------------------------------

struct SlowModule;

#[async_trait]
impl Module for SlowModule {
    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "steps": { "type": "integer" } } })
    }
    fn output_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "completed_steps": { "type": "integer" } } })
    }
    fn description(&self) -> &str {
        "A slow module that checks for cancellation between steps"
    }

    async fn execute(&self, input: Value, ctx: &Context<Value>) -> Result<Value, ModuleError> {
        let steps = input["steps"].as_i64().unwrap_or(5) as usize;

        for i in 0..steps {
            // Check cancellation before each step
            if let Some(token) = &ctx.cancel_token {
                if token.is_cancelled() {
                    println!("  [SlowModule] Cancelled at step {i}");
                    return Err(ModuleError::new(
                        apcore::errors::ErrorCode::ExecutionCancelled,
                        format!("Execution cancelled after {i} steps"),
                    ));
                }
            }

            println!("  [SlowModule] Executing step {i}...");
            // Simulate work (non-blocking)
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }

        Ok(json!({ "completed_steps": steps }))
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let identity = Identity::new(
        "user-1".to_string(),
        "Alice".to_string(),
        vec![],
        HashMap::new(),
    );

    // --- Run 1: complete all steps (no cancellation) ---
    println!("=== Run 1: normal execution ===");
    let mut ctx: Context<Value> = Context::new(identity.clone());
    let token = CancelToken::new();
    ctx.cancel_token = Some(token);

    let module = SlowModule;
    let result = module.execute(json!({"steps": 3}), &ctx).await.unwrap();
    println!("Result: {result}\n");

    // --- Run 2: cancel mid-flight ---
    println!("=== Run 2: cancelled mid-flight ===");
    let mut ctx2: Context<Value> = Context::new(identity.clone());
    let token2 = CancelToken::new();
    ctx2.cancel_token = Some(token2.clone());

    // Cancel after 80 ms (step 1 runs at ~0ms, step 2 at ~50ms, cancel fires at ~80ms)
    let token2_clone = token2.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;
        println!("  [main] Sending cancel signal…");
        token2_clone.cancel();
    });

    match module.execute(json!({"steps": 10}), &ctx2).await {
        Ok(r) => println!("Result: {r}"),
        Err(e) => println!("Error (expected): {e}"),
    }

    // --- CancelToken basics ---
    println!("\n=== CancelToken state demo ===");
    let t = CancelToken::new();
    println!("Before cancel: is_cancelled = {}", t.is_cancelled()); // false
    t.cancel();
    println!("After cancel:  is_cancelled = {}", t.is_cancelled()); // true

    // Clone shares state
    let t2 = t.clone();
    println!("Cloned token:  is_cancelled = {}", t2.is_cancelled()); // true
}
