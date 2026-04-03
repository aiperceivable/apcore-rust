// APCore Protocol — Identity, Context, and ContextFactory
// Spec reference: Execution context and identity model

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use std::sync::{Arc, RwLock};

use crate::cancel::CancelToken;
use crate::observability::logging::ContextLogger;

/// Raw intermediate struct for deserializing Identity with private fields.
#[derive(Deserialize)]
struct IdentityRaw {
    id: String,
    #[serde(rename = "type")]
    identity_type: String,
    #[serde(default)]
    roles: Vec<String>,
    #[serde(default)]
    attrs: HashMap<String, serde_json::Value>,
}

impl From<IdentityRaw> for Identity {
    fn from(raw: IdentityRaw) -> Self {
        Identity {
            id: raw.id,
            identity_type: raw.identity_type,
            roles: raw.roles,
            attrs: raw.attrs,
        }
    }
}

/// Frozen/immutable identity representing the caller.
/// Fields are private to enforce immutability after construction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "IdentityRaw")]
pub struct Identity {
    id: String,
    #[serde(rename = "type")]
    identity_type: String,
    roles: Vec<String>,
    attrs: HashMap<String, serde_json::Value>,
}

impl Identity {
    /// Create a new identity with the given fields.
    pub fn new(
        id: String,
        identity_type: String,
        roles: Vec<String>,
        attrs: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            id,
            identity_type,
            roles,
            attrs,
        }
    }

    /// The unique identifier of the caller.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The type of the identity (e.g. "user", "service", "agent").
    pub fn identity_type(&self) -> &str {
        &self.identity_type
    }

    /// The roles assigned to this identity.
    pub fn roles(&self) -> &[String] {
        &self.roles
    }

    /// Additional attributes associated with this identity.
    pub fn attrs(&self) -> &HashMap<String, serde_json::Value> {
        &self.attrs
    }
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
    #[serde(default)]
    pub call_chain: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redacted_inputs: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip)]
    pub cancel_token: Option<CancelToken>,
    /// Global deadline for dual-timeout enforcement (not serialized).
    /// Stored as absolute epoch seconds (f64) for cross-language alignment.
    #[serde(skip)]
    pub global_deadline: Option<f64>,
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
            caller_id: None,
            data: default_shared_data(),
            call_chain: vec![],
            redacted_inputs: None,
            cancel_token: None,
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
            caller_id: None,
            data: default_shared_data(),
            call_chain: vec![],
            redacted_inputs: None,
            cancel_token: None,
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
            caller_id,
            // Share the same underlying data map (Arc clone, not HashMap clone).
            data: Arc::clone(&self.data),
            call_chain,
            redacted_inputs: None,
            cancel_token: self.cancel_token.clone(),
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

    /// Serialize context to a cross-language JSON representation.
    ///
    /// Includes `_context_version: 1` at top level.
    /// Excludes: executor, services, cancel_token, global_deadline.
    /// Filters `_`-prefixed keys from data.
    pub fn serialize(&self) -> serde_json::Value {
        let mut result = serde_json::json!({
            "_context_version": 1,
            "trace_id": self.trace_id,
            "caller_id": self.caller_id,
            "call_chain": self.call_chain,
        });

        if let Some(ref identity) = self.identity {
            result["identity"] = serde_json::json!({
                "id": identity.id(),
                "type": identity.identity_type(),
                "roles": identity.roles(),
                "attrs": identity.attrs(),
            });
        } else {
            result["identity"] = serde_json::Value::Null;
        }

        if let Some(ref redacted) = self.redacted_inputs {
            result["redacted_inputs"] =
                serde_json::to_value(redacted).unwrap_or_default();
        }

        // Filter _-prefixed keys from data
        let filtered: HashMap<String, serde_json::Value> = self
            .data
            .read()
            .map(|map| {
                map.iter()
                    .filter(|(k, _)| !k.starts_with('_'))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();

        result["data"] = serde_json::to_value(filtered).unwrap_or_default();
        result
    }

    /// Deserialize a cross-language JSON representation into a Context.
    ///
    /// Non-serializable fields (executor, services, cancel_token,
    /// global_deadline) are set to `None`/default after deserialization.
    /// If `_context_version` is greater than 1, a warning is logged
    /// but deserialization proceeds (forward compatibility).
    pub fn deserialize(
        value: serde_json::Value,
    ) -> Result<Self, serde_json::Error>
    where
        T: Default,
    {
        let obj = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("expected JSON object"))?;

        let version = obj
            .get("_context_version")
            .and_then(|v| v.as_i64())
            .unwrap_or(1);

        if version > 1 {
            tracing::warn!(
                version = version,
                "Unknown _context_version (expected 1). \
                 Proceeding with best-effort deserialization."
            );
        }

        let identity: Option<Identity> = obj
            .get("identity")
            .and_then(|v| {
                if v.is_null() {
                    None
                } else {
                    serde_json::from_value(v.clone()).ok()
                }
            });

        let data_map: HashMap<String, serde_json::Value> = obj
            .get("data")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let call_chain: Vec<String> = obj
            .get("call_chain")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let redacted_inputs: Option<HashMap<String, serde_json::Value>> = obj
            .get("redacted_inputs")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        Ok(Context {
            trace_id: obj
                .get("trace_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            caller_id: obj
                .get("caller_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            call_chain,
            identity,
            redacted_inputs,
            data: Arc::new(RwLock::new(data_map)),
            services: T::default(),
            cancel_token: None,
            global_deadline: None,
            executor: None,
        })
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
            caller_id,
            data: Arc::new(RwLock::new(data.unwrap_or_default())),
            call_chain: vec![],
            redacted_inputs: None,
            cancel_token: None,
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
