// APCore Protocol — Identity, Context, and ContextFactory
// Spec reference: Execution context and identity model

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::cancel::CancelToken;
use crate::trace_context::TraceContext;

/// Frozen/immutable identity representing the caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub attributes: HashMap<String, serde_json::Value>,
}

/// Generic execution context carrying identity, config, and metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context<T> {
    pub execution_id: Uuid,
    pub identity: Identity,
    pub config: T,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_execution_id: Option<Uuid>,
    #[serde(default)]
    pub call_depth: u32,
    #[serde(default)]
    pub call_chain: Vec<String>,
    #[serde(skip)]
    pub cancel_token: Option<CancelToken>,
    #[serde(skip)]
    pub trace_context: Option<TraceContext>,
}

impl<T: Default> Context<T> {
    pub fn new(identity: Identity) -> Self {
        Self {
            execution_id: Uuid::new_v4(),
            identity,
            config: T::default(),
            created_at: Utc::now(),
            metadata: HashMap::new(),
            parent_execution_id: None,
            call_depth: 0,
            call_chain: vec![],
            cancel_token: None,
            trace_context: None,
        }
    }
}

/// Factory trait for creating execution contexts.
#[async_trait]
pub trait ContextFactory: Send + Sync {
    /// Create a new context for the given identity and config.
    async fn create(
        &self,
        identity: Identity,
        config: serde_json::Value,
    ) -> Result<Context<serde_json::Value>, crate::errors::ModuleError>;

    /// Create a child context from an existing parent context.
    async fn create_child(
        &self,
        parent: &Context<serde_json::Value>,
        module_name: &str,
    ) -> Result<Context<serde_json::Value>, crate::errors::ModuleError>;
}
