// APCore Protocol — Identity, Context, and ContextFactory
// Spec reference: Execution context and identity model

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use std::sync::{Arc, RwLock};
use tokio::time::Instant;

use crate::cancel::CancelToken;
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

/// Shared mutable data map that is readable/writable along the call chain.
///
/// Parent and child contexts share the same underlying `HashMap` through
/// an `Arc<RwLock<...>>`. Use `data.read().unwrap_or_else(|e| e.into_inner())`
/// / `data.write().unwrap_or_else(|e| e.into_inner())` to access entries.
///
/// Note: deserialization creates a new `Arc` (not shared with the original).
pub type SharedData = Arc<RwLock<HashMap<String, serde_json::Value>>>;

/// Generic execution context carrying identity, services, and data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Context<T> {
    pub trace_id: String,
    /// The caller identity. `None` represents an anonymous/unauthenticated caller.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    pub services: T,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller_id: Option<String>,
    /// Shared data map — parent and child contexts share the same instance.
    /// Serializes as a plain `HashMap`; deserializes into a new `Arc<RwLock<...>>`.
    #[serde(
        serialize_with = "serialize_shared_data",
        deserialize_with = "deserialize_shared_data",
        default = "default_shared_data"
    )]
    pub data: SharedData,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_trace_id: Option<String>,
    #[serde(default)]
    pub call_chain: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redacted_inputs: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip)]
    pub cancel_token: Option<CancelToken>,
    #[serde(skip)]
    pub trace_context: Option<TraceContext>,
    /// Global deadline for dual-timeout enforcement (not serialized).
    /// Set on root call; propagated to child contexts.
    #[serde(skip)]
    pub global_deadline: Option<Instant>,
    /// Runtime reference to the executor for nested calls (not serialized).
    #[serde(skip)]
    pub executor: Option<Arc<dyn std::any::Any + Send + Sync>>,
}

fn default_shared_data() -> SharedData {
    Arc::new(RwLock::new(HashMap::new()))
}

fn serialize_shared_data<S>(data: &SharedData, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let map = data.read().map_err(serde::ser::Error::custom)?;
    map.serialize(serializer)
}

fn deserialize_shared_data<'de, D>(deserializer: D) -> Result<SharedData, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let map = HashMap::<String, serde_json::Value>::deserialize(deserializer)?;
    Ok(Arc::new(RwLock::new(map)))
}

impl<T: Default> Context<T> {
    pub fn new(identity: Identity) -> Self {
        Self {
            trace_id: uuid::Uuid::new_v4().to_string(),
            identity: Some(identity),
            services: T::default(),
            created_at: Utc::now(),
            caller_id: None,
            data: default_shared_data(),
            parent_trace_id: None,
            call_chain: vec![],
            redacted_inputs: None,
            cancel_token: None,
            trace_context: None,
            global_deadline: None,
            executor: None,
        }
    }

    /// Create a context with no identity (anonymous/unauthenticated).
    pub fn anonymous() -> Self {
        Self {
            trace_id: uuid::Uuid::new_v4().to_string(),
            identity: None,
            services: T::default(),
            created_at: Utc::now(),
            caller_id: None,
            data: default_shared_data(),
            parent_trace_id: None,
            call_chain: vec![],
            redacted_inputs: None,
            cancel_token: None,
            trace_context: None,
            global_deadline: None,
            executor: None,
        }
    }

    /// Create a child context for nested calls.
    ///
    /// The child shares the same `data` map (via `Arc`) so writes in either
    /// parent or child are visible to both.
    pub fn child(&self, target_module_id: &str) -> Context<T>
    where
        T: Clone,
    {
        let caller_id = self.call_chain.last().cloned();
        let mut call_chain = self.call_chain.clone();
        call_chain.push(target_module_id.to_string());

        Context {
            trace_id: self.trace_id.clone(),
            identity: self.identity.clone(),
            services: self.services.clone(),
            created_at: Utc::now(),
            caller_id,
            // Share the same underlying data map (Arc clone, not HashMap clone).
            data: Arc::clone(&self.data),
            parent_trace_id: self.parent_trace_id.clone(),
            call_chain,
            redacted_inputs: None,
            cancel_token: self.cancel_token.clone(),
            trace_context: self.trace_context.clone(),
            global_deadline: self.global_deadline,
            executor: self.executor.clone(),
        }
    }

    /// Serialize context to JSON.
    pub fn to_json(&self) -> serde_json::Value
    where
        T: Serialize,
    {
        let mut value = serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({}));
        // Filter out internal keys (keys starting with "_") from data
        if let Some(obj) = value.as_object_mut() {
            if let Some(data_val) = obj.get_mut("data") {
                if let Some(data_obj) = data_val.as_object_mut() {
                    let internal_keys: Vec<String> = data_obj
                        .keys()
                        .filter(|k| k.starts_with('_'))
                        .cloned()
                        .collect();
                    for key in internal_keys {
                        data_obj.remove(&key);
                    }
                }
            }
        }
        value
    }

    /// Deserialize context from JSON.
    pub fn from_json(
        data: serde_json::Value,
    ) -> Result<Context<serde_json::Value>, crate::errors::ModuleError> {
        let ctx: Context<serde_json::Value> = serde_json::from_value(data)?;
        Ok(ctx)
    }

    /// Get a context-scoped logger.
    pub fn logger(&self) -> ContextLogger {
        let module_id = self.call_chain.last().cloned();
        let caller_id = self.caller_id.clone();
        ContextLogger {
            name: "apcore".to_string(),
            level: "info".to_string(),
            format: crate::observability::logging::LogFormat::Json,
            trace_id: Some(self.trace_id.clone()),
            module_id,
            caller_id,
        }
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
            identity: Some(identity),
            services,
            created_at: Utc::now(),
            caller_id,
            data: Arc::new(RwLock::new(data.unwrap_or_default())),
            parent_trace_id: None,
            call_chain: vec![],
            redacted_inputs: None,
            cancel_token: None,
            trace_context: None,
            global_deadline: None,
            executor: None,
        }
    }
}

/// Factory trait for creating execution contexts.
#[async_trait]
pub trait ContextFactory: Send + Sync {
    /// Create a new context for the given identity and services.
    /// Pass `None` for anonymous/unauthenticated contexts.
    async fn create(
        &self,
        identity: Option<Identity>,
        services: serde_json::Value,
    ) -> Result<Context<serde_json::Value>, crate::errors::ModuleError>;

    /// Create a child context from an existing parent context.
    async fn create_child(
        &self,
        parent: &Context<serde_json::Value>,
        module_name: &str,
    ) -> Result<Context<serde_json::Value>, crate::errors::ModuleError>;
}
