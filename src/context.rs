// APCore Protocol — Identity, Context, and ContextFactory
// Spec reference: Execution context and identity model

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use parking_lot::RwLock;
use std::sync::Arc;

use crate::cancel::CancelToken;
use crate::observability::logging::ContextLogger;
use crate::trace_context::TraceParent;

const TRACE_ID_ZEROS: &str = "00000000000000000000000000000000";
const TRACE_ID_FFFF: &str = "ffffffffffffffffffffffffffffffff";

/// Generate a fresh 32-char lowercase hex trace_id aligned with W3C Trace Context.
#[must_use]
fn generate_trace_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Accept a trace_parent's trace_id if it is well-formed (32 lowercase hex,
/// not W3C-invalid); otherwise log WARN and return a fresh trace_id.
///
/// PROTOCOL_SPEC §10.5 `external_trace_parent_handling`: no dashed-UUID
/// stripping, no case folding — such normalization is the caller's
/// responsibility, not Context::create's.
fn accept_or_regenerate_trace_id(incoming: &str) -> String {
    let is_valid_hex = incoming.len() == 32
        && incoming
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let is_w3c_valid = incoming != TRACE_ID_ZEROS && incoming != TRACE_ID_FFFF;
    if is_valid_hex && is_w3c_valid {
        incoming.to_string()
    } else {
        tracing::warn!(
            "Invalid trace_id format in trace_parent: {:?}. Restarting trace.",
            incoming
        );
        generate_trace_id()
    }
}

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "IdentityRaw")]
#[allow(clippy::struct_field_names)] // `identity_type` intentionally prefixed for clarity with renamed JSON field
pub struct Identity {
    id: String,
    #[serde(rename = "type")]
    identity_type: String,
    roles: Vec<String>,
    attrs: HashMap<String, serde_json::Value>,
}

impl Identity {
    /// Create a new identity with the given fields.
    #[must_use]
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
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The type of the identity (e.g. "user", "service", "agent").
    #[must_use]
    pub fn identity_type(&self) -> &str {
        &self.identity_type
    }

    /// The roles assigned to this identity.
    #[must_use]
    pub fn roles(&self) -> &[String] {
        &self.roles
    }

    /// Additional attributes associated with this identity.
    #[must_use]
    pub fn attrs(&self) -> &HashMap<String, serde_json::Value> {
        &self.attrs
    }
}

// Manual Hash impl: serde_json::Value doesn't implement Hash, so we sort
// attrs by key and hash each value's canonical JSON string representation.
// This is consistent with the derived PartialEq/Eq (order-independent HashMap
// equality), because sorting by key always produces the same ordered sequence
// for any two maps that compare equal.
impl std::hash::Hash for Identity {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.identity_type.hash(state);
        self.roles.hash(state);
        let mut pairs: Vec<_> = self.attrs.iter().collect();
        pairs.sort_by_key(|(k, _)| k.as_str());
        for (k, v) in pairs {
            k.hash(state);
            v.to_string().hash(state);
        }
    }
}

/// Shared mutable data map that is readable/writable along the call chain.
///
/// Parent and child contexts share the same underlying `HashMap` through
/// an `Arc<parking_lot::RwLock<...>>`. Use `data.read()` / `data.write()`
/// to access entries — the guards are infallible (not `Result`), so no
/// `.unwrap()` is required.
///
/// IMPORTANT: `parking_lot::RwLock` guards are synchronous. Never hold a
/// guard across an `.await` point — copy or clone the data you need and
/// drop the guard first.
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redacted_output: Option<HashMap<String, serde_json::Value>>,
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
    let map = data.read();
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
    #[must_use]
    pub fn new(identity: Identity) -> Self {
        Self {
            trace_id: generate_trace_id(),
            identity: Some(identity),
            services: T::default(),
            caller_id: None,
            data: default_shared_data(),
            call_chain: vec![],
            redacted_inputs: None,
            redacted_output: None,
            cancel_token: None,
            global_deadline: None,
            executor: None,
        }
    }

    /// Create a context with no identity (anonymous/unauthenticated).
    #[must_use]
    pub fn anonymous() -> Self {
        Self {
            trace_id: generate_trace_id(),
            identity: None,
            services: T::default(),
            caller_id: None,
            data: default_shared_data(),
            call_chain: vec![],
            redacted_inputs: None,
            redacted_output: None,
            cancel_token: None,
            global_deadline: None,
            executor: None,
        }
    }

    /// Create a child context for nested calls.
    ///
    /// The child shares the same `data` map (via `Arc`) so writes in either
    /// parent or child are visible to both.
    #[must_use]
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
            redacted_output: None,
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
    /// Excludes: executor, services, `cancel_token`, `global_deadline`.
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
            result["redacted_inputs"] = serde_json::to_value(redacted).unwrap_or_default();
        }
        if let Some(ref redacted) = self.redacted_output {
            result["redacted_output"] = serde_json::to_value(redacted).unwrap_or_default();
        }

        // Filter _-prefixed keys from data
        let filtered: HashMap<String, serde_json::Value> = self
            .data
            .read()
            .iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        result["data"] = serde_json::to_value(filtered).unwrap_or_default();
        result
    }

    /// Deserialize a cross-language JSON representation into a Context.
    ///
    /// Non-serializable fields (executor, services, `cancel_token`,
    /// `global_deadline`) are set to `None`/default after deserialization.
    /// If `_context_version` is greater than 1, a warning is logged
    /// but deserialization proceeds (forward compatibility).
    #[allow(clippy::needless_pass_by_value)] // public API: callers pass owned Value; changing to &Value would be breaking
    pub fn deserialize(value: serde_json::Value) -> Result<Self, serde_json::Error>
    where
        T: Default,
    {
        let obj = value
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("expected JSON object"))?;

        let version = obj
            .get("_context_version")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(1);

        if version > 1 {
            tracing::warn!(
                version = version,
                "Unknown _context_version (expected 1). \
                 Proceeding with best-effort deserialization."
            );
        }

        let identity: Option<Identity> = obj.get("identity").and_then(|v| {
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

        let redacted_output: Option<HashMap<String, serde_json::Value>> = obj
            .get("redacted_output")
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
                .map(std::string::ToString::to_string),
            call_chain,
            identity,
            redacted_inputs,
            redacted_output,
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
            trace_id: generate_trace_id(),
            identity: Some(identity),
            services,
            caller_id,
            data: Arc::new(RwLock::new(data.unwrap_or_default())),
            call_chain: vec![],
            redacted_inputs: None,
            redacted_output: None,
            cancel_token: None,
            global_deadline: None,
            executor: None,
        }
    }

    /// Start building a context with optional W3C trace_parent inheritance.
    ///
    /// Use this when integrating with web frameworks that parse incoming
    /// `traceparent` headers — the builder accepts an `Option<TraceParent>`
    /// and inherits its trace_id when well-formed, or regenerates on invalid
    /// input. See PROTOCOL_SPEC §10.5 `external_trace_parent_handling`.
    #[must_use]
    pub fn builder() -> ContextBuilder<T> {
        ContextBuilder::new()
    }
}

/// Builder for [`Context`] that supports W3C Trace Context propagation.
///
/// Created via [`Context::builder`]. Accepts an optional `TraceParent` whose
/// trace_id is inherited when it matches `^[0-9a-f]{32}$` and is not the
/// W3C-reserved all-zero or all-f value. Otherwise the builder generates a
/// fresh trace_id and emits a `tracing::warn!` log.
pub struct ContextBuilder<T> {
    trace_parent: Option<TraceParent>,
    identity: Option<Identity>,
    services: Option<T>,
    caller_id: Option<String>,
    data: Option<HashMap<String, serde_json::Value>>,
}

impl<T> Default for ContextBuilder<T> {
    fn default() -> Self {
        Self {
            trace_parent: None,
            identity: None,
            services: None,
            caller_id: None,
            data: None,
        }
    }
}

impl<T> ContextBuilder<T> {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the W3C trace_parent to inherit from (when well-formed).
    #[must_use]
    pub fn trace_parent(mut self, trace_parent: Option<TraceParent>) -> Self {
        self.trace_parent = trace_parent;
        self
    }

    /// Set the caller identity. `None` is the anonymous/unauthenticated case.
    #[must_use]
    pub fn identity(mut self, identity: Option<Identity>) -> Self {
        self.identity = identity;
        self
    }

    /// Set the services container for dependency injection.
    #[must_use]
    pub fn services(mut self, services: T) -> Self {
        self.services = Some(services);
        self
    }

    /// Set the caller_id. Top-level calls leave this as `None`.
    #[must_use]
    pub fn caller_id(mut self, caller_id: Option<String>) -> Self {
        self.caller_id = caller_id;
        self
    }

    /// Seed the shared data map with initial entries.
    #[must_use]
    pub fn data(mut self, data: HashMap<String, serde_json::Value>) -> Self {
        self.data = Some(data);
        self
    }
}

impl<T: Default> ContextBuilder<T> {
    /// Finalize the builder into a [`Context`].
    ///
    /// If a `trace_parent` was set, its trace_id is validated per
    /// PROTOCOL_SPEC §10.5 and either accepted verbatim or replaced with a
    /// freshly generated 32-char hex trace_id (with a WARN log on regen).
    #[must_use]
    pub fn build(self) -> Context<T> {
        let trace_id = match self.trace_parent.as_ref() {
            Some(tp) => accept_or_regenerate_trace_id(&tp.trace_id),
            None => generate_trace_id(),
        };
        Context {
            trace_id,
            identity: self.identity,
            services: self.services.unwrap_or_default(),
            caller_id: self.caller_id,
            data: Arc::new(RwLock::new(self.data.unwrap_or_default())),
            call_chain: vec![],
            redacted_inputs: None,
            redacted_output: None,
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

    /// Spec-compliant alias for [`create`].
    ///
    /// `create_context` is the canonical method name defined in the apcore protocol
    /// spec (`ContextFactory.create_context(request)`). This default implementation
    /// delegates to [`create`] so existing implementations remain unbroken.
    async fn create_context(
        &self,
        identity: Option<Identity>,
        services: serde_json::Value,
    ) -> Result<Context<serde_json::Value>, crate::errors::ModuleError> {
        self.create(identity, services).await
    }

    /// Create a child context from an existing parent context.
    async fn create_child(
        &self,
        parent: &Context<serde_json::Value>,
        module_name: &str,
    ) -> Result<Context<serde_json::Value>, crate::errors::ModuleError>;
}
