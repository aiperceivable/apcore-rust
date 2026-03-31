// APCore Protocol — Event subscribers
// Spec reference: Event subscription, webhook delivery, A2A delivery

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use async_trait::async_trait;
#[cfg(feature = "events")]
use serde_json::json;

use super::emitter::ApCoreEvent;
use crate::errors::{ErrorCode, ModuleError};

/// Trait for receiving events from the EventEmitter.
#[async_trait]
pub trait EventSubscriber: Send + Sync + std::fmt::Debug {
    /// Unique ID for this subscriber (used by unsubscribe).
    fn subscriber_id(&self) -> &str;

    /// The event type pattern this subscriber is interested in.
    fn event_pattern(&self) -> &str;

    /// Handle an incoming event.
    async fn on_event(&self, event: &ApCoreEvent) -> Result<(), ModuleError>;
}

// ---------------------------------------------------------------------------
// WebhookSubscriber
// ---------------------------------------------------------------------------

/// Delivers events via HTTP POST to a webhook URL.
///
/// Retry strategy: 2xx = success, 4xx = no retry, 5xx / connection error = retry.
/// Requires the `events` cargo feature for actual HTTP delivery.
#[derive(Debug, Clone)]
pub struct WebhookSubscriber {
    pub id: String,
    pub url: String,
    pub event_pattern: String,
    pub headers: HashMap<String, String>,
    pub retry_count: u32,
    pub timeout_ms: u64,
}

impl WebhookSubscriber {
    pub fn new(
        id: impl Into<String>,
        url: impl Into<String>,
        event_pattern: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            url: url.into(),
            event_pattern: event_pattern.into(),
            headers: HashMap::new(),
            retry_count: 3,
            timeout_ms: 5000,
        }
    }
}

#[async_trait]
impl EventSubscriber for WebhookSubscriber {
    fn subscriber_id(&self) -> &str {
        &self.id
    }

    fn event_pattern(&self) -> &str {
        &self.event_pattern
    }

    #[cfg(feature = "events")]
    async fn on_event(&self, event: &ApCoreEvent) -> Result<(), ModuleError> {
        let body = serde_json::to_value(event).unwrap_or_default();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(self.timeout_ms))
            .build()
            .map_err(|e| {
                ModuleError::new(
                    ErrorCode::GeneralInternalError,
                    format!("HTTP client error: {e}"),
                )
            })?;

        let mut last_error = None;
        for attempt in 0..=self.retry_count {
            if attempt > 0 {
                tracing::debug!(attempt, url = %self.url, "WebhookSubscriber retrying");
            }
            let mut req = client.post(&self.url).json(&body);
            req = req.header("Content-Type", "application/json");
            for (k, v) in &self.headers {
                req = req.header(k.as_str(), v.as_str());
            }
            match req.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    if (200..300).contains(&status) {
                        return Ok(());
                    }
                    if status < 500 {
                        // 4xx: no retry.
                        tracing::warn!(status, url = %self.url, "WebhookSubscriber: non-retryable error");
                        return Ok(());
                    }
                    last_error = Some(format!("HTTP {status}"));
                }
                Err(e) => {
                    last_error = Some(e.to_string());
                }
            }
        }
        tracing::warn!(
            url = %self.url,
            error = ?last_error,
            "WebhookSubscriber: delivery failed after retries"
        );
        Ok(())
    }

    #[cfg(not(feature = "events"))]
    async fn on_event(&self, event: &ApCoreEvent) -> Result<(), ModuleError> {
        tracing::info!(
            subscriber_id = %self.id,
            url = %self.url,
            event_type = %event.event_type,
            "WebhookSubscriber: HTTP delivery requires 'events' feature"
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// A2ASubscriber
// ---------------------------------------------------------------------------

/// Authentication for A2A subscriber.
#[derive(Debug, Clone)]
pub enum A2AAuth {
    /// Bearer token → `Authorization: Bearer <token>`.
    Bearer(String),
    /// Dict of headers to merge into the request.
    Headers(HashMap<String, String>),
}

/// Delivers events via the A2A protocol to the platform.
///
/// Payload: `{ "skillId": "apevo.event_receiver", "event": <serialized> }`.
/// Single attempt (no retries). Errors logged, not raised.
#[derive(Debug, Clone)]
pub struct A2ASubscriber {
    pub id: String,
    pub platform_url: String,
    pub auth: Option<A2AAuth>,
    pub event_pattern: String,
    pub timeout_ms: u64,
}

impl A2ASubscriber {
    pub fn new(
        id: impl Into<String>,
        platform_url: impl Into<String>,
        event_pattern: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            platform_url: platform_url.into(),
            auth: None,
            event_pattern: event_pattern.into(),
            timeout_ms: 5000,
        }
    }
}

#[async_trait]
impl EventSubscriber for A2ASubscriber {
    fn subscriber_id(&self) -> &str {
        &self.id
    }

    fn event_pattern(&self) -> &str {
        &self.event_pattern
    }

    #[cfg(feature = "events")]
    async fn on_event(&self, event: &ApCoreEvent) -> Result<(), ModuleError> {
        let payload = json!({
            "skillId": "apevo.event_receiver",
            "event": {
                "event_type": event.event_type,
                "module_id": event.module_id,
                "timestamp": event.timestamp,
                "severity": event.severity,
                "data": event.data,
            }
        });

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(self.timeout_ms))
            .build()
            .map_err(|e| {
                ModuleError::new(
                    ErrorCode::GeneralInternalError,
                    format!("HTTP client error: {e}"),
                )
            })?;

        let mut req = client.post(&self.platform_url).json(&payload);
        req = req.header("Content-Type", "application/json");
        match &self.auth {
            Some(A2AAuth::Bearer(token)) => {
                req = req.header("Authorization", format!("Bearer {token}"));
            }
            Some(A2AAuth::Headers(headers)) => {
                for (k, v) in headers {
                    req = req.header(k.as_str(), v.as_str());
                }
            }
            None => {}
        }

        if let Err(e) = req.send().await {
            tracing::warn!(
                url = %self.platform_url,
                error = %e,
                "A2ASubscriber: delivery failed"
            );
        }
        Ok(())
    }

    #[cfg(not(feature = "events"))]
    async fn on_event(&self, event: &ApCoreEvent) -> Result<(), ModuleError> {
        tracing::info!(
            subscriber_id = %self.id,
            url = %self.platform_url,
            event_type = %event.event_type,
            "A2ASubscriber: HTTP delivery requires 'events' feature"
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Subscriber factory — config-driven instantiation
// ---------------------------------------------------------------------------

type SubscriberFactory =
    Box<dyn Fn(&serde_json::Value) -> Result<Box<dyn EventSubscriber>, ModuleError> + Send + Sync>;

fn global_subscriber_factories() -> &'static RwLock<HashMap<String, SubscriberFactory>> {
    static FACTORIES: OnceLock<RwLock<HashMap<String, SubscriberFactory>>> = OnceLock::new();
    FACTORIES.get_or_init(|| {
        let mut map: HashMap<String, SubscriberFactory> = HashMap::new();

        // Built-in: webhook
        map.insert(
            "webhook".to_string(),
            Box::new(|config| {
                let url = config
                    .get("url")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ModuleError::new(ErrorCode::ConfigInvalid, "webhook: 'url' is required")
                    })?
                    .to_string();
                let retry_count = config
                    .get("retry_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(3) as u32;
                let timeout_ms = config
                    .get("timeout_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(5000);
                let mut headers = HashMap::new();
                if let Some(h) = config.get("headers").and_then(|v| v.as_object()) {
                    for (k, v) in h {
                        if let Some(vs) = v.as_str() {
                            headers.insert(k.clone(), vs.to_string());
                        }
                    }
                }
                let id = format!("webhook-{}", uuid::Uuid::new_v4());
                let mut sub = WebhookSubscriber::new(id, url, "*");
                sub.headers = headers;
                sub.retry_count = retry_count;
                sub.timeout_ms = timeout_ms;
                Ok(Box::new(sub) as Box<dyn EventSubscriber>)
            }),
        );

        // Built-in: a2a
        map.insert(
            "a2a".to_string(),
            Box::new(|config| {
                let platform_url = config
                    .get("platform_url")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        ModuleError::new(
                            ErrorCode::ConfigInvalid,
                            "a2a: 'platform_url' is required",
                        )
                    })?
                    .to_string();
                let timeout_ms = config
                    .get("timeout_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(5000);
                let auth = match config.get("auth") {
                    Some(serde_json::Value::String(s)) => Some(A2AAuth::Bearer(s.clone())),
                    Some(serde_json::Value::Object(m)) => {
                        let mut headers = HashMap::new();
                        for (k, v) in m {
                            if let Some(vs) = v.as_str() {
                                headers.insert(k.clone(), vs.to_string());
                            }
                        }
                        Some(A2AAuth::Headers(headers))
                    }
                    _ => None,
                };
                let id = format!("a2a-{}", uuid::Uuid::new_v4());
                let mut sub = A2ASubscriber::new(id, platform_url, "*");
                sub.auth = auth;
                sub.timeout_ms = timeout_ms;
                Ok(Box::new(sub) as Box<dyn EventSubscriber>)
            }),
        );

        RwLock::new(map)
    })
}

/// Register a custom subscriber type factory.
pub fn register_subscriber_type(type_name: &str, factory: SubscriberFactory) {
    let mut map = global_subscriber_factories()
        .write()
        .unwrap_or_else(|e| e.into_inner());
    map.insert(type_name.to_string(), factory);
}

/// Unregister a subscriber type.
pub fn unregister_subscriber_type(type_name: &str) -> Result<(), ModuleError> {
    let mut map = global_subscriber_factories()
        .write()
        .unwrap_or_else(|e| e.into_inner());
    if map.remove(type_name).is_none() {
        return Err(ModuleError::new(
            ErrorCode::GeneralInternalError,
            format!("Subscriber type '{}' is not registered", type_name),
        ));
    }
    Ok(())
}

/// Reset the subscriber factory registry to built-in types only.
pub fn reset_subscriber_registry() {
    let mut map = global_subscriber_factories()
        .write()
        .unwrap_or_else(|e| e.into_inner());
    map.clear();
    drop(map);
    // Re-trigger OnceLock initialization by reading — but OnceLock is already init.
    // Instead, re-insert built-ins manually.
    let mut map = global_subscriber_factories()
        .write()
        .unwrap_or_else(|e| e.into_inner());
    // Re-register webhook and a2a via register_subscriber_type after clearing.
    // The OnceLock was already initialized, so we just re-populate.
    map.insert(
        "webhook".to_string(),
        Box::new(|config| {
            let url = config
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ModuleError::new(ErrorCode::ConfigInvalid, "webhook: 'url' is required")
                })?
                .to_string();
            let id = format!("webhook-{}", uuid::Uuid::new_v4());
            Ok(Box::new(WebhookSubscriber::new(id, url, "*")) as Box<dyn EventSubscriber>)
        }),
    );
    map.insert(
        "a2a".to_string(),
        Box::new(|config| {
            let platform_url = config
                .get("platform_url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ModuleError::new(ErrorCode::ConfigInvalid, "a2a: 'platform_url' is required")
                })?
                .to_string();
            let id = format!("a2a-{}", uuid::Uuid::new_v4());
            Ok(Box::new(A2ASubscriber::new(id, platform_url, "*")) as Box<dyn EventSubscriber>)
        }),
    );
}

/// Create a subscriber from config using the factory registry.
pub fn create_subscriber(
    config: &serde_json::Value,
) -> Result<Box<dyn EventSubscriber>, ModuleError> {
    let type_name = config.get("type").and_then(|v| v.as_str()).ok_or_else(|| {
        ModuleError::new(
            ErrorCode::ConfigInvalid,
            "subscriber config missing 'type' field",
        )
    })?;

    let map = global_subscriber_factories()
        .read()
        .unwrap_or_else(|e| e.into_inner());
    let factory = map.get(type_name).ok_or_else(|| {
        ModuleError::new(
            ErrorCode::ConfigInvalid,
            format!("Unknown subscriber type '{}'", type_name),
        )
    })?;
    factory(config)
}
