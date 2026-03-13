// APCore Protocol — Identity, Context, and ContextFactory
// Spec reference: Execution context and identity model

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use std::sync::Arc;

use crate::cancel::CancelToken;
use crate::errors::ModuleError;
use crate::observability::logging::ContextLogger;
use crate::trace_context::TraceContext;

/// Frozen/immutable identity representing the caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub id: String,
    #[serde(rename = "type")]
    pub identity_type: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub attrs: HashMap<String, serde_json::Value>,
}

/// Generic execution context carrying identity, services, and data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context<T> {
    pub trace_id: String,
    pub identity: Identity,
    pub services: T,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_id: Option<String>,
    #[serde(default)]
    pub data: HashMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_trace_id: Option<String>,
    #[serde(default)]
    pub call_depth: u32,
    #[serde(default)]
    pub call_chain: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redacted_inputs: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip)]
    pub cancel_token: Option<CancelToken>,
    #[serde(skip)]
    pub trace_context: Option<TraceContext>,
    /// Runtime reference to the executor for nested calls (not serialized).
    #[serde(skip)]
    pub executor: Option<Arc<dyn std::any::Any + Send + Sync>>,
}

impl<T: Default> Context<T> {
    pub fn new(identity: Identity) -> Self {
        Self {
            trace_id: uuid::Uuid::new_v4().to_string(),
            identity,
            services: T::default(),
            created_at: Utc::now(),
            caller_id: None,
            data: HashMap::new(),
            parent_trace_id: None,
            call_depth: 0,
            call_chain: vec![],
            redacted_inputs: None,
            cancel_token: None,
            trace_context: None,
            executor: None,
        }
    }

    /// Create a child context for nested calls.
    pub fn child(&self, target_module_id: &str) -> Context<T> where T: Clone {
        let caller_id = self.call_chain.last().cloned();
        let mut call_chain = self.call_chain.clone();
        call_chain.push(target_module_id.to_string());

        Context {
            trace_id: self.trace_id.clone(),
            identity: self.identity.clone(),
            services: self.services.clone(),
            created_at: Utc::now(),
            caller_id,
            data: self.data.clone(),
            parent_trace_id: self.parent_trace_id.clone(),
            call_depth: self.call_depth + 1,
            call_chain,
            redacted_inputs: None,
            cancel_token: self.cancel_token.clone(),
            trace_context: self.trace_context.clone(),
            executor: self.executor.clone(),
        }
    }

    /// Serialize context to JSON.
    pub fn to_json(&self) -> serde_json::Value {
        todo!("Context.to_json() — serialize context")
    }

    /// Deserialize context from JSON.
    pub fn from_json(data: serde_json::Value) -> Result<Context<serde_json::Value>, crate::errors::ModuleError> {
        todo!("Context.from_json() — deserialize context")
    }

    /// Get a context-scoped logger.
    pub fn logger(&self) -> ContextLogger {
        todo!("Context.logger() — context-scoped logger")
    }

    /// Create a context from explicit parameters.
    pub fn create(
        identity: Identity,
        services: T,
        caller_id: Option<String>,
        data: Option<HashMap<String, serde_json::Value>>,
    ) -> Self {
        Self {
            trace_id: uuid::Uuid::new_v4().to_string(),
            identity,
            services,
            created_at: Utc::now(),
            caller_id,
            data: data.unwrap_or_default(),
            parent_trace_id: None,
            call_depth: 0,
            call_chain: vec![],
            redacted_inputs: None,
            cancel_token: None,
            trace_context: None,
            executor: None,
        }
    }
}

/// Factory trait for creating execution contexts.
#[async_trait]
pub trait ContextFactory: Send + Sync {
    /// Create a new context for the given identity and services.
    async fn create(
        &self,
        identity: Identity,
        services: serde_json::Value,
    ) -> Result<Context<serde_json::Value>, crate::errors::ModuleError>;

    /// Create a child context from an existing parent context.
    async fn create_child(
        &self,
        parent: &Context<serde_json::Value>,
        module_name: &str,
    ) -> Result<Context<serde_json::Value>, crate::errors::ModuleError>;
}
