// APCore Protocol — Event subscribers
// Spec reference: Event subscription, webhook delivery, A2A delivery

use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use async_trait::async_trait;
#[cfg(feature = "events")]
use serde_json::json;
use tokio::io::AsyncWriteExt;

use super::emitter::ApCoreEvent;
use crate::errors::{ErrorCode, ModuleError};

/// Severity ranking used by `StdoutSubscriber` for `level_filter`.
/// Unknown levels fall back to `info` (rank 0).
fn severity_rank(level: &str) -> u8 {
    match level {
        "warn" => 1,
        "error" => 2,
        "fatal" => 3,
        _ => 0, // "info" and unknown levels share the lowest rank
    }
}

/// Render an event as a single text line: `[ts] [LEVEL] event_type module=... data=...`.
fn render_event_text(event: &ApCoreEvent) -> String {
    let module = match &event.module_id {
        Some(m) => m.as_str(),
        None => "None",
    };
    format!(
        "[{}] [{}] {} module={} data={}",
        event.timestamp,
        event.severity.to_uppercase(),
        event.event_type,
        module,
        event.data
    )
}

/// Render an event as a single JSON line.
fn render_event_json(event: &ApCoreEvent) -> Result<String, ModuleError> {
    serde_json::to_string(event).map_err(|e| {
        ModuleError::new(
            ErrorCode::GeneralInternalError,
            format!("failed to serialize event: {e}"),
        )
    })
}

/// Trait for receiving events from the `EventEmitter`.
#[async_trait]
pub trait EventSubscriber: Send + Sync + std::fmt::Debug {
    /// Unique ID for this subscriber (used by unsubscribe).
    ///
    /// Defaults to `"default"`. Override this when multiple subscribers must be
    /// distinguishable by `EventEmitter::unsubscribe_by_id`.
    // The default returns a literal, but implementors may return non-static strings
    // (e.g. `&self.id`), so the trait signature must remain `&str`.
    #[allow(clippy::unnecessary_literal_bound)]
    fn subscriber_id(&self) -> &str {
        "default"
    }

    /// The event type pattern this subscriber is interested in.
    ///
    /// Defaults to `"*"` (matches all events). Override to filter by prefix or
    /// exact event type (e.g. `"module.*"` or `"module.loaded"`).
    #[allow(clippy::unnecessary_literal_bound)]
    fn event_pattern(&self) -> &str {
        "*"
    }

    /// The bound event-type string for bulk unsubscription, if set.
    ///
    /// Returns `Some(event_type)` for subscribers wrapped by `APCore::on()`
    /// (via `EventTypeSubscriber`), `None` for all others. Used internally by
    /// `EventEmitter::unsubscribe_by_event_type()` to implement
    /// `APCore::off_by_type()` without downcasting.
    fn event_type_filter(&self) -> Option<&str> {
        None
    }

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
// FileSubscriber — built-in `file` type
// ---------------------------------------------------------------------------

/// Output format for `FileSubscriber` and `StdoutSubscriber`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Json,
    Text,
}

impl OutputFormat {
    /// Parse a YAML/JSON config string into an `OutputFormat`.
    #[must_use]
    pub fn from_config_str(s: Option<&str>) -> Self {
        match s {
            Some("json") => Self::Json,
            _ => Self::Text,
        }
    }
}

/// Writes events to a local file using append-mode IO with optional rotation.
///
/// When `rotate_bytes` is set and the existing file's size is at or above the
/// limit at write time, the current file is renamed to `<path>.1` before the
/// next write — matching the Python `FileSubscriber` behaviour.
#[derive(Debug, Clone)]
pub struct FileSubscriber {
    pub id: String,
    pub path: PathBuf,
    pub append: bool,
    pub format: OutputFormat,
    pub rotate_bytes: Option<u64>,
}

impl FileSubscriber {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            id: format!("file-{}", uuid::Uuid::new_v4()),
            path: path.into(),
            append: true,
            format: OutputFormat::Json,
            rotate_bytes: None,
        }
    }

    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    #[must_use]
    pub fn with_format(mut self, format: OutputFormat) -> Self {
        self.format = format;
        self
    }

    #[must_use]
    pub fn with_append(mut self, append: bool) -> Self {
        self.append = append;
        self
    }

    #[must_use]
    pub fn with_rotate_bytes(mut self, rotate_bytes: Option<u64>) -> Self {
        self.rotate_bytes = rotate_bytes;
        self
    }

    async fn rotate_if_needed(&self) -> Result<(), std::io::Error> {
        let Some(limit) = self.rotate_bytes else {
            return Ok(());
        };
        let metadata = match tokio::fs::metadata(&self.path).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        if metadata.len() >= limit {
            let mut rotated = self.path.clone();
            let Some(name) = rotated.file_name() else {
                return Ok(());
            };
            let new_name = format!("{}.1", name.to_string_lossy());
            rotated.set_file_name(new_name);
            tokio::fs::rename(&self.path, &rotated).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl EventSubscriber for FileSubscriber {
    fn subscriber_id(&self) -> &str {
        &self.id
    }

    // Built-in subscribers always match every event; the `&str` signature is
    // dictated by the trait, so the literal lifetime cannot be widened.
    #[allow(clippy::unnecessary_literal_bound)]
    fn event_pattern(&self) -> &str {
        "*"
    }

    async fn on_event(&self, event: &ApCoreEvent) -> Result<(), ModuleError> {
        if let Err(e) = self.rotate_if_needed().await {
            tracing::warn!(path = %self.path.display(), error = %e, "FileSubscriber: rotate failed");
        }

        let line = match self.format {
            OutputFormat::Json => {
                let mut s = render_event_json(event)?;
                s.push('\n');
                s
            }
            OutputFormat::Text => {
                let mut s = render_event_text(event);
                s.push('\n');
                s
            }
        };

        let mut opts = tokio::fs::OpenOptions::new();
        opts.create(true);
        if self.append {
            opts.append(true);
        } else {
            opts.write(true).truncate(true);
        }

        let mut file = opts.open(&self.path).await.map_err(|e| {
            ModuleError::new(
                ErrorCode::GeneralInternalError,
                format!("FileSubscriber: open '{}' failed: {e}", self.path.display()),
            )
        })?;

        file.write_all(line.as_bytes()).await.map_err(|e| {
            ModuleError::new(
                ErrorCode::GeneralInternalError,
                format!(
                    "FileSubscriber: write to '{}' failed: {e}",
                    self.path.display()
                ),
            )
        })?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// StdoutSubscriber — built-in `stdout` type
// ---------------------------------------------------------------------------

/// Writes events to stdout, optionally filtered by minimum severity.
#[derive(Debug, Clone)]
pub struct StdoutSubscriber {
    pub id: String,
    pub format: OutputFormat,
    pub level_filter: Option<String>,
}

impl Default for StdoutSubscriber {
    fn default() -> Self {
        Self::new()
    }
}

impl StdoutSubscriber {
    #[must_use]
    pub fn new() -> Self {
        Self {
            id: format!("stdout-{}", uuid::Uuid::new_v4()),
            format: OutputFormat::Text,
            level_filter: None,
        }
    }

    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    #[must_use]
    pub fn with_format(mut self, format: OutputFormat) -> Self {
        self.format = format;
        self
    }

    #[must_use]
    pub fn with_level_filter(mut self, level: Option<String>) -> Self {
        self.level_filter = level;
        self
    }

    fn allow(&self, event: &ApCoreEvent) -> bool {
        match &self.level_filter {
            Some(min) => severity_rank(&event.severity) >= severity_rank(min),
            None => true,
        }
    }
}

#[async_trait]
impl EventSubscriber for StdoutSubscriber {
    fn subscriber_id(&self) -> &str {
        &self.id
    }

    #[allow(clippy::unnecessary_literal_bound)] // see FileSubscriber comment
    fn event_pattern(&self) -> &str {
        "*"
    }

    async fn on_event(&self, event: &ApCoreEvent) -> Result<(), ModuleError> {
        if !self.allow(event) {
            return Ok(());
        }
        let line = match self.format {
            OutputFormat::Json => render_event_json(event)?,
            OutputFormat::Text => render_event_text(event),
        };
        // Explicit user-facing stdout subscriber — println! is intentional here.
        // It mirrors the Python `print(line, file=sys.stdout)` semantics and is the
        // documented behaviour of the `stdout` built-in subscriber type.
        #[allow(clippy::print_stdout)]
        {
            println!("{line}");
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// FilterSubscriber — built-in `filter` type
// ---------------------------------------------------------------------------

/// Wraps a delegate subscriber and forwards only events whose `event_type`
/// matches an allow list (or does NOT match a deny list).
///
/// Matching rules (mirror the spec normative text):
/// 1. If `include_events` is set, forward events that match any pattern in it.
/// 2. Else if `exclude_events` is set, drop events that match any pattern; forward the rest.
/// 3. Otherwise forward everything.
///
/// Patterns use simple `*` glob matching (compatible with Python's `fnmatch`
/// against typical event-type strings such as `apcore.error.*`).
pub struct FilterSubscriber {
    pub id: String,
    pub delegate: Box<dyn EventSubscriber>,
    pub include_events: Option<Vec<String>>,
    pub exclude_events: Option<Vec<String>>,
}

impl std::fmt::Debug for FilterSubscriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilterSubscriber")
            .field("id", &self.id)
            .field("include_events", &self.include_events)
            .field("exclude_events", &self.exclude_events)
            .finish_non_exhaustive()
    }
}

impl FilterSubscriber {
    #[must_use]
    pub fn new(delegate: Box<dyn EventSubscriber>) -> Self {
        Self {
            id: format!("filter-{}", uuid::Uuid::new_v4()),
            delegate,
            include_events: None,
            exclude_events: None,
        }
    }

    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    #[must_use]
    pub fn with_include(mut self, patterns: Vec<String>) -> Self {
        self.include_events = Some(patterns);
        self
    }

    #[must_use]
    pub fn with_exclude(mut self, patterns: Vec<String>) -> Self {
        self.exclude_events = Some(patterns);
        self
    }

    /// Decide whether the given event type should be forwarded to the delegate.
    #[must_use]
    pub fn matches(&self, event_type: &str) -> bool {
        if let Some(includes) = &self.include_events {
            return includes
                .iter()
                .any(|pat| match_glob_pattern(pat, event_type));
        }
        if let Some(excludes) = &self.exclude_events {
            return !excludes
                .iter()
                .any(|pat| match_glob_pattern(pat, event_type));
        }
        true
    }
}

#[async_trait]
impl EventSubscriber for FilterSubscriber {
    fn subscriber_id(&self) -> &str {
        &self.id
    }

    fn event_pattern(&self) -> &str {
        self.delegate.event_pattern()
    }

    async fn on_event(&self, event: &ApCoreEvent) -> Result<(), ModuleError> {
        if self.matches(&event.event_type) {
            self.delegate.on_event(event).await
        } else {
            Ok(())
        }
    }
}

/// Simple `*`-only glob matcher used by `FilterSubscriber`.
///
/// Supports any number of `*` wildcards in the pattern; each matches zero or
/// more characters. This is the subset of `fnmatch` behaviour the spec
/// fixtures and YAML examples actually exercise.
fn match_glob_pattern(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut remaining = value;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if let Some(rest) = remaining.strip_prefix(part) {
                remaining = rest;
            } else {
                return false;
            }
        } else if let Some(pos) = remaining.find(part) {
            remaining = &remaining[pos + part.len()..];
        } else {
            return false;
        }
    }
    if !pattern.ends_with('*') && !remaining.is_empty() {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// Subscriber factory — config-driven instantiation
// ---------------------------------------------------------------------------

type SubscriberFactory =
    Box<dyn Fn(&serde_json::Value) -> Result<Box<dyn EventSubscriber>, ModuleError> + Send + Sync>;

// ModuleError is a protocol-level domain type whose rich field set is spec-required;
// boxing individual fields would break ergonomics across the entire codebase.
#[allow(clippy::result_large_err)]
fn build_webhook_subscriber(
    config: &serde_json::Value,
) -> Result<Box<dyn EventSubscriber>, ModuleError> {
    let url = config
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ModuleError::new(ErrorCode::ConfigInvalid, "webhook: 'url' is required"))?
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
}

#[allow(clippy::result_large_err)]
fn build_a2a_subscriber(
    config: &serde_json::Value,
) -> Result<Box<dyn EventSubscriber>, ModuleError> {
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
}

#[allow(clippy::result_large_err)]
fn build_file_subscriber(
    config: &serde_json::Value,
) -> Result<Box<dyn EventSubscriber>, ModuleError> {
    let path = config
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ModuleError::new(ErrorCode::ConfigInvalid, "file: 'path' is required"))?
        .to_string();
    let format =
        OutputFormat::from_config_str(config.get("format").and_then(serde_json::Value::as_str));
    let append = config
        .get("append")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let rotate_bytes = config
        .get("rotate_bytes")
        .and_then(serde_json::Value::as_u64);
    let sub = FileSubscriber::new(path)
        .with_format(format)
        .with_append(append)
        .with_rotate_bytes(rotate_bytes);
    Ok(Box::new(sub) as Box<dyn EventSubscriber>)
}

// `Result` return matches the SubscriberFactory contract; the other built-in
// factories can fail and the registry uses a single uniform signature.
#[allow(clippy::result_large_err, clippy::unnecessary_wraps)]
fn build_stdout_subscriber(
    config: &serde_json::Value,
) -> Result<Box<dyn EventSubscriber>, ModuleError> {
    let format =
        OutputFormat::from_config_str(config.get("format").and_then(serde_json::Value::as_str));
    let level_filter = config
        .get("level_filter")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let sub = StdoutSubscriber::new()
        .with_format(format)
        .with_level_filter(level_filter);
    Ok(Box::new(sub) as Box<dyn EventSubscriber>)
}

#[allow(clippy::result_large_err)]
fn build_filter_subscriber(
    config: &serde_json::Value,
) -> Result<Box<dyn EventSubscriber>, ModuleError> {
    let delegate_type = config
        .get("delegate_type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            ModuleError::new(
                ErrorCode::ConfigInvalid,
                "filter: 'delegate_type' is required",
            )
        })?
        .to_string();
    let mut delegate_config = config
        .get("delegate_config")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    if let serde_json::Value::Object(map) = &mut delegate_config {
        map.insert(
            "type".to_string(),
            serde_json::Value::String(delegate_type.clone()),
        );
    } else {
        return Err(ModuleError::new(
            ErrorCode::ConfigInvalid,
            "filter: 'delegate_config' must be an object",
        ));
    }
    let delegate = create_subscriber(&delegate_config)?;

    let include = config
        .get("include_events")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        });
    let exclude = config
        .get("exclude_events")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        });

    let mut sub = FilterSubscriber::new(delegate);
    if let Some(p) = include {
        sub = sub.with_include(p);
    }
    if let Some(p) = exclude {
        sub = sub.with_exclude(p);
    }
    Ok(Box::new(sub) as Box<dyn EventSubscriber>)
}

/// Register the built-in subscriber type factories
/// (`webhook`, `a2a`, `file`, `stdout`, `filter`).
fn register_builtin_factories(map: &mut HashMap<String, SubscriberFactory>) {
    map.insert("webhook".to_string(), Box::new(build_webhook_subscriber));
    map.insert("a2a".to_string(), Box::new(build_a2a_subscriber));
    map.insert("file".to_string(), Box::new(build_file_subscriber));
    map.insert("stdout".to_string(), Box::new(build_stdout_subscriber));
    map.insert("filter".to_string(), Box::new(build_filter_subscriber));
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
