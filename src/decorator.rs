// APCore Protocol — Decorator / FunctionModule
// Spec reference: Function-based module creation via attribute macros
//
// In Rust, the #[module] attribute macro concept would be implemented
// as a proc-macro in a separate crate. This file provides the runtime
// support types for function-based modules.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::context::Context;
use crate::errors::ModuleError;
use crate::module::{Module, ModuleAnnotations};

/// Boxed async handler type for FunctionModule.
type HandlerFn = Box<
    dyn for<'a> Fn(
            &'a Context<serde_json::Value>,
            serde_json::Value,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<serde_json::Value, ModuleError>> + Send + 'a>,
        > + Send
        + Sync,
>;

/// A module implemented as a wrapped async function.
pub struct FunctionModule {
    pub annotations: ModuleAnnotations,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    handler: HandlerFn,
}

impl std::fmt::Debug for FunctionModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FunctionModule")
            .field("annotations", &self.annotations)
            .finish()
    }
}

impl FunctionModule {
    /// Create a new FunctionModule wrapping an async handler.
    pub fn new<F, Fut>(
        annotations: ModuleAnnotations,
        input_schema: serde_json::Value,
        output_schema: serde_json::Value,
        handler: F,
    ) -> Self
    where
        F: for<'a> Fn(
                &'a Context<serde_json::Value>,
                serde_json::Value,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<serde_json::Value, ModuleError>> + Send + 'a>,
            > + Send
            + Sync
            + 'static,
    {
        Self {
            annotations,
            input_schema,
            output_schema,
            handler: Box::new(handler),
        }
    }
}

#[async_trait]
impl Module for FunctionModule {
    fn input_schema(&self) -> serde_json::Value {
        self.input_schema.clone()
    }

    fn output_schema(&self) -> serde_json::Value {
        self.output_schema.clone()
    }

    fn description(&self) -> &str {
        self.annotations.description.as_deref().unwrap_or("")
    }

    async fn execute(
        &self,
        ctx: &Context<serde_json::Value>,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, ModuleError> {
        (self.handler)(ctx, input).await
    }
}
