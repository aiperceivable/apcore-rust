// APCore Protocol — Event subscribers
// Spec reference: Event subscription, webhook delivery, A2A delivery

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::OnceLock;

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
    async fn on_event(&self, _event: &ApCoreEvent) -> Result<(), ModuleError> {
        Err(ModuleError::new(
            ErrorCode::GeneralInternalError,
            "WebhookSubscriber requires the 'events' feature",
        ))
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
    async fn on_event(&self, _event: &ApCoreEvent) -> Result<(), ModuleError> {
        Err(ModuleError::new(
            ErrorCode::GeneralInternalError,
            "A2ASubscriber requires the 'events' feature",
        ))
    }
}

// ---------------------------------------------------------------------------
// Subscriber factory — config-driven instantiation
// ---------------------------------------------------------------------------

type SubscriberFactory =
    Box<dyn Fn(&serde_json::Value) -> Result<Box<dyn EventSubscriber>, ModuleError> + Send + Sync>;

/// Register the webhook + a2a factories with the full config-honoring logic.
fn register_builtin_factories(map: &mut HashMap<String, SubscriberFactory>) {
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
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(3)
                .try_into()
                .unwrap_or(3u32);
            let timeout_ms = config
                .get("timeout_ms")
                .and_then(serde_json::Value::as_u64)
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
                    ModuleError::new(ErrorCode::ConfigInvalid, "a2a: 'platform_url' is required")
                })?
                .to_string();
            let timeout_ms = config
                .get("timeout_ms")
                .and_then(serde_json::Value::as_u64)
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
}

fn global_subscriber_factories() -> &'static RwLock<HashMap<String, SubscriberFactory>> {
    static FACTORIES: OnceLock<RwLock<HashMap<String, SubscriberFactory>>> = OnceLock::new();
    FACTORIES.get_or_init(|| {
        let mut map: HashMap<String, SubscriberFactory> = HashMap::new();
        register_builtin_factories(&mut map);
        RwLock::new(map)
    })
}

/// Register a custom subscriber type factory.
pub fn register_subscriber_type(type_name: &str, factory: SubscriberFactory) {
    let mut map = global_subscriber_factories().write();
    map.insert(type_name.to_string(), factory);
}

/// Unregister a subscriber type.
pub fn unregister_subscriber_type(type_name: &str) -> Result<(), ModuleError> {
    let mut map = global_subscriber_factories().write();
    if map.remove(type_name).is_none() {
        return Err(ModuleError::new(
            ErrorCode::GeneralInternalError,
            format!("Subscriber type '{type_name}' is not registered"),
        ));
    }
    Ok(())
}

/// Reset the subscriber factory registry to built-in types only.
pub fn reset_subscriber_registry() {
    let mut map = global_subscriber_factories().write();
    map.clear();
    register_builtin_factories(&mut map);
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

    let map = global_subscriber_factories().read();
    let factory = map.get(type_name).ok_or_else(|| {
        ModuleError::new(
            ErrorCode::ConfigInvalid,
            format!("Unknown subscriber type '{type_name}'"),
        )
    })?;
    factory(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    /// Tests that call `reset_subscriber_registry` or mutate the global
    /// subscriber registry must hold this lock to avoid cross-test races.
    static REGISTRY_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_webhook_subscriber_new() {
        let sub = WebhookSubscriber::new("wh1", "https://example.com/hook", "test.*");
        assert_eq!(sub.id, "wh1");
        assert_eq!(sub.url, "https://example.com/hook");
        assert_eq!(sub.event_pattern, "test.*");
        assert_eq!(sub.retry_count, 3);
        assert_eq!(sub.timeout_ms, 5000);
        assert!(sub.headers.is_empty());
    }

    #[test]
    fn test_webhook_subscriber_trait_methods() {
        let sub = WebhookSubscriber::new("wh1", "https://example.com", "*");
        assert_eq!(sub.subscriber_id(), "wh1");
        assert_eq!(sub.event_pattern(), "*");
    }

    #[test]
    fn test_a2a_subscriber_new() {
        let sub = A2ASubscriber::new("a2a1", "https://platform.example.com", "module.*");
        assert_eq!(sub.id, "a2a1");
        assert_eq!(sub.platform_url, "https://platform.example.com");
        assert_eq!(sub.event_pattern, "module.*");
        assert!(sub.auth.is_none());
        assert_eq!(sub.timeout_ms, 5000);
    }

    #[test]
    fn test_a2a_subscriber_trait_methods() {
        let sub = A2ASubscriber::new("a2a1", "https://p.com", "ev.*");
        assert_eq!(sub.subscriber_id(), "a2a1");
        assert_eq!(sub.event_pattern(), "ev.*");
    }

    #[test]
    fn test_a2a_auth_bearer() {
        let auth = A2AAuth::Bearer("my_token".to_string());
        match auth {
            A2AAuth::Bearer(t) => assert_eq!(t, "my_token"),
            A2AAuth::Headers(_) => panic!("expected Bearer"),
        }
    }

    #[test]
    fn test_a2a_auth_headers() {
        let mut headers = HashMap::new();
        headers.insert("X-Custom".to_string(), "value".to_string());
        let auth = A2AAuth::Headers(headers);
        match auth {
            A2AAuth::Headers(h) => assert_eq!(h.get("X-Custom").unwrap(), "value"),
            A2AAuth::Bearer(_) => panic!("expected Headers"),
        }
    }

    #[cfg(not(feature = "events"))]
    #[tokio::test]
    async fn test_webhook_on_event_requires_events_feature() {
        use super::super::emitter::ApCoreEvent;
        let sub = WebhookSubscriber::new("wh1", "https://example.com", "*");
        let event = ApCoreEvent::new("test", json!({}));
        let result = sub.on_event(&event).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::GeneralInternalError);
    }

    #[cfg(not(feature = "events"))]
    #[tokio::test]
    async fn test_a2a_on_event_requires_events_feature() {
        use super::super::emitter::ApCoreEvent;
        let sub = A2ASubscriber::new("a1", "https://p.com", "*");
        let event = ApCoreEvent::new("test", json!({}));
        let result = sub.on_event(&event).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::GeneralInternalError);
    }

    #[test]
    fn test_create_subscriber_missing_type_field() {
        let config = json!({"url": "https://example.com"});
        let result = create_subscriber(&config);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::ConfigInvalid);
        assert!(err.message.contains("'type'"));
    }

    #[test]
    fn test_create_subscriber_unknown_type() {
        let _lock = REGISTRY_LOCK.lock().unwrap();
        reset_subscriber_registry();
        let config = json!({"type": "nonexistent"});
        let result = create_subscriber(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("nonexistent"));
    }

    #[test]
    fn test_create_webhook_subscriber_via_factory() {
        let _lock = REGISTRY_LOCK.lock().unwrap();
        reset_subscriber_registry();
        let config = json!({
            "type": "webhook",
            "url": "https://example.com/hook",
            "retry_count": 5,
            "timeout_ms": 3000,
            "headers": {"Authorization": "Bearer tok"}
        });
        let sub = create_subscriber(&config).unwrap();
        assert!(sub.subscriber_id().starts_with("webhook-"));
        assert_eq!(sub.event_pattern(), "*");
    }

    #[test]
    fn test_create_webhook_subscriber_missing_url() {
        let _lock = REGISTRY_LOCK.lock().unwrap();
        reset_subscriber_registry();
        let config = json!({"type": "webhook"});
        let result = create_subscriber(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("url"));
    }

    #[test]
    fn test_create_a2a_subscriber_via_factory() {
        let _lock = REGISTRY_LOCK.lock().unwrap();
        reset_subscriber_registry();
        let config = json!({
            "type": "a2a",
            "platform_url": "https://platform.example.com",
            "timeout_ms": 2000,
            "auth": "my_bearer_token"
        });
        let sub = create_subscriber(&config).unwrap();
        assert!(sub.subscriber_id().starts_with("a2a-"));
        assert_eq!(sub.event_pattern(), "*");
    }

    #[test]
    fn test_create_a2a_subscriber_missing_platform_url() {
        let _lock = REGISTRY_LOCK.lock().unwrap();
        reset_subscriber_registry();
        let config = json!({"type": "a2a"});
        let result = create_subscriber(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("platform_url"));
    }

    #[test]
    fn test_register_and_unregister_custom_subscriber_type() {
        #[derive(Debug)]
        struct CustomSub;

        #[async_trait]
        impl EventSubscriber for CustomSub {
            fn subscriber_id(&self) -> &'static str {
                "custom-1"
            }
            fn event_pattern(&self) -> &'static str {
                "*"
            }
            async fn on_event(
                &self,
                _event: &super::super::emitter::ApCoreEvent,
            ) -> Result<(), ModuleError> {
                Ok(())
            }
        }

        let _lock = REGISTRY_LOCK.lock().unwrap();
        let type_name = "custom_unique_test_type";

        register_subscriber_type(
            type_name,
            Box::new(|_config| Ok(Box::new(CustomSub) as Box<dyn EventSubscriber>)),
        );

        let config = json!({"type": type_name});
        let sub = create_subscriber(&config).unwrap();
        assert_eq!(sub.subscriber_id(), "custom-1");

        unregister_subscriber_type(type_name).unwrap();
        let result = create_subscriber(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_unregister_nonexistent_type_returns_error() {
        let _lock = REGISTRY_LOCK.lock().unwrap();
        reset_subscriber_registry();
        let result = unregister_subscriber_type("does_not_exist");
        assert!(result.is_err());
    }

    #[test]
    fn test_reset_subscriber_registry_restores_builtins() {
        let _lock = REGISTRY_LOCK.lock().unwrap();
        reset_subscriber_registry();
        let wh_config = json!({"type": "webhook", "url": "https://example.com"});
        assert!(create_subscriber(&wh_config).is_ok());

        let a2a_config = json!({"type": "a2a", "platform_url": "https://p.com"});
        assert!(create_subscriber(&a2a_config).is_ok());
    }
}
