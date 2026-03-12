//! Simple client example — implement the Module trait and execute it directly.

use apcore::context::{Context, Identity};
use apcore::errors::ModuleError;
use apcore::module::Module;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Module definition
// ---------------------------------------------------------------------------

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
        json!({
            "type": "object",
            "properties": {
                "result": { "type": "integer" }
            }
        })
    }

    fn description(&self) -> &str {
        "Add two integers"
    }

    async fn execute(
        &self,
        _ctx: &Context<Value>,
        input: Value,
    ) -> Result<Value, ModuleError> {
        let a = input["a"].as_i64().unwrap_or(0);
        let b = input["b"].as_i64().unwrap_or(0);
        Ok(json!({ "result": a + b }))
    }
}

// ---------------------------------------------------------------------------
// Another module: greet
// ---------------------------------------------------------------------------

struct GreetModule;

#[async_trait]
impl Module for GreetModule {
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name":     { "type": "string" },
                "greeting": { "type": "string" }
            },
            "required": ["name"]
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "message": { "type": "string" }
            }
        })
    }

    fn description(&self) -> &str {
        "Greet a user"
    }

    async fn execute(
        &self,
        _ctx: &Context<Value>,
        input: Value,
    ) -> Result<Value, ModuleError> {
        let name = input["name"].as_str().unwrap_or("World");
        let greeting = input["greeting"].as_str().unwrap_or("Hello");
        Ok(json!({ "message": format!("{}, {}!", greeting, name) }))
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    // Build a caller identity
    let identity = Identity {
        id: "user-1".to_string(),
        name: "Alice".to_string(),
        roles: vec!["user".to_string()],
        attributes: HashMap::new(),
    };

    // Create an execution context
    let ctx: Context<Value> = Context::new(identity);

    // Instantiate modules
    let add_module = AddModule;
    let greet_module = GreetModule;

    // Execute math.add
    let result = add_module
        .execute(&ctx, json!({"a": 10, "b": 5}))
        .await
        .unwrap();
    println!("math.add result:  {result}"); // {"result":15}

    // Execute greet
    let result = greet_module
        .execute(&ctx, json!({"name": "Alice", "greeting": "Hi"}))
        .await
        .unwrap();
    println!("greet result:     {result}"); // {"message":"Hi, Alice!"}

    // Default greeting
    let result = greet_module
        .execute(&ctx, json!({"name": "Bob"}))
        .await
        .unwrap();
    println!("greet (default):  {result}"); // {"message":"Hello, Bob!"}
}
