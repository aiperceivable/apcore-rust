// APCore Protocol — Per-subscriber circuit breaker (Issue #36)
// Spec reference: docs/features/event-system.md (Event Management Hardening)
//
// Wraps any `EventSubscriber` with an independent circuit breaker that
// tolerates a degraded downstream by tripping into OPEN after
// `open_threshold` consecutive failures and probing once via HALF_OPEN after
// `recovery_window_ms`. Mirrors the Python `CircuitBreakerWrapper`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde_json::json;

use super::emitter::ApCoreEvent;
use super::subscribers::EventSubscriber;
use crate::errors::ModuleError;

/// Circuit-breaker lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

impl CircuitState {
    /// Spec-canonical uppercase representation (`"CLOSED" | "OPEN" | "HALF_OPEN"`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "CLOSED",
            Self::Open => "OPEN",
            Self::HalfOpen => "HALF_OPEN",
        }
    }
}

/// Sink for the circuit-breaker's own lifecycle events
/// (`apcore.subscriber.circuit_opened` / `circuit_closed`).
///
/// In production this is typically a wrapper around the surrounding
/// `EventEmitter`; in tests it is a recording sink that captures the events
/// for assertion. Decoupling the breaker from a concrete `EventEmitter`
/// avoids an Arc cycle (the emitter holds the wrapper, the wrapper would
/// otherwise need to hold the emitter).
pub trait CircuitEventSink: Send + Sync + std::fmt::Debug {
    /// Receive an `apcore.subscriber.*` lifecycle event.
    fn emit_circuit_event(&self, event: ApCoreEvent);
}

/// Default tunables, matching the spec.
pub const DEFAULT_TIMEOUT_MS: u64 = 5000;
pub const DEFAULT_OPEN_THRESHOLD: u32 = 5;
pub const DEFAULT_RECOVERY_WINDOW_MS: u64 = 60_000;

/// Circuit-breaker wrapper that enforces timeouts, counts failures, and
/// transitions through `CLOSED → OPEN → HALF_OPEN → CLOSED`.
pub struct CircuitBreakerWrapper {
    subscriber: Box<dyn EventSubscriber>,
    sink: Option<Arc<dyn CircuitEventSink>>,
    timeout_ms: u64,
    open_threshold: u32,
    recovery_window_ms: u64,
    state: Arc<Mutex<CircuitState>>,
    consecutive_failures: AtomicU32,
    last_failure_at: Arc<Mutex<Option<DateTime<Utc>>>>,
    /// Pluggable clock for tests; defaults to wall clock.
    clock: Box<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    subscriber_type_name: String,
}

impl std::fmt::Debug for CircuitBreakerWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CircuitBreakerWrapper")
            .field("state", &*self.state.lock())
            .field(
                "consecutive_failures",
                &self.consecutive_failures.load(Ordering::SeqCst),
            )
            .field("timeout_ms", &self.timeout_ms)
            .field("open_threshold", &self.open_threshold)
            .field("recovery_window_ms", &self.recovery_window_ms)
            .field("subscriber_type_name", &self.subscriber_type_name)
            .finish_non_exhaustive()
    }
}

impl CircuitBreakerWrapper {
    /// Wrap `subscriber` with circuit-breaker behaviour.
    ///
    /// `sink` receives `apcore.subscriber.circuit_opened` and
    /// `apcore.subscriber.circuit_closed` events on transitions; if `None`,
    /// transitions still happen but no events are emitted.
    #[must_use]
    pub fn new(
        subscriber: Box<dyn EventSubscriber>,
        sink: Option<Arc<dyn CircuitEventSink>>,
    ) -> Self {
        let subscriber_type_name = guess_subscriber_type_name(subscriber.as_ref());
        Self {
            subscriber,
            sink,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            open_threshold: DEFAULT_OPEN_THRESHOLD,
            recovery_window_ms: DEFAULT_RECOVERY_WINDOW_MS,
            state: Arc::new(Mutex::new(CircuitState::Closed)),
            consecutive_failures: AtomicU32::new(0),
            last_failure_at: Arc::new(Mutex::new(None)),
            clock: Box::new(Utc::now),
            subscriber_type_name,
        }
    }

    #[must_use]
    pub fn with_timeout_ms(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }

    #[must_use]
    pub fn with_open_threshold(mut self, n: u32) -> Self {
        self.open_threshold = n;
        self
    }

    #[must_use]
    pub fn with_recovery_window_ms(mut self, ms: u64) -> Self {
        self.recovery_window_ms = ms;
        self
    }

    /// Override the clock used to compute recovery-window elapse.
    /// Intended for tests that exercise time-based transitions.
    #[must_use]
    pub fn with_clock<F>(mut self, clock: F) -> Self
    where
        F: Fn() -> DateTime<Utc> + Send + Sync + 'static,
    {
        self.clock = Box::new(clock);
        self
    }

    /// Override the human-readable subscriber type name embedded in
    /// `apcore.subscriber.*` event payloads.
    #[must_use]
    pub fn with_subscriber_type_name(mut self, name: impl Into<String>) -> Self {
        self.subscriber_type_name = name.into();
        self
    }

    /// Snapshot the current circuit state.
    #[must_use]
    pub fn state(&self) -> CircuitState {
        *self.state.lock()
    }

    /// Snapshot the current consecutive-failure count.
    #[must_use]
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures.load(Ordering::SeqCst)
    }

    /// Last recorded failure timestamp, or `None` if there has been no failure
    /// yet (or the breaker has fully recovered to CLOSED).
    #[must_use]
    pub fn last_failure_at(&self) -> Option<DateTime<Utc>> {
        *self.last_failure_at.lock()
    }

    /// Force the circuit state. Public for tests + callers that need to
    /// hydrate state from external storage; no public guarantees about safety
    /// during concurrent delivery.
    pub fn force_state(&self, state: CircuitState) {
        *self.state.lock() = state;
    }

    /// Force the recorded `last_failure_at`. Public for tests.
    pub fn force_last_failure_at(&self, t: Option<DateTime<Utc>>) {
        *self.last_failure_at.lock() = t;
    }

    /// Force the consecutive-failure counter. Public for tests.
    pub fn force_consecutive_failures(&self, n: u32) {
        self.consecutive_failures.store(n, Ordering::SeqCst);
    }

    /// Transition `OPEN → HALF_OPEN` if `recovery_window_ms` has elapsed since
    /// `last_failure_at`. Always called at the start of `on_event`; exposed
    /// publicly so tests can probe the transition without invoking delivery.
    pub fn check_recovery(&self) {
        let mut state_guard = self.state.lock();
        if *state_guard != CircuitState::Open {
            return;
        }
        let Some(last) = *self.last_failure_at.lock() else {
            return;
        };
        let now = (self.clock)();
        let elapsed_ms_signed = now.signed_duration_since(last).num_milliseconds().max(0);
        // safe cast: max(0) ensures the value is non-negative
        let elapsed_ms = u64::try_from(elapsed_ms_signed).unwrap_or(u64::MAX);
        if elapsed_ms >= self.recovery_window_ms {
            *state_guard = CircuitState::HalfOpen;
        }
    }

    /// Record a successful delivery, returning the lifecycle event (if any) to emit.
    fn on_success(&self) -> Option<ApCoreEvent> {
        let mut state_guard = self.state.lock();
        let was_half_open = *state_guard == CircuitState::HalfOpen;
        *state_guard = CircuitState::Closed;
        self.consecutive_failures.store(0, Ordering::SeqCst);
        if was_half_open {
            Some(self.make_event(
                "apcore.subscriber.circuit_closed",
                "info",
                json!({
                    "subscriber_type": self.subscriber_type_name,
                    "recovery_attempt": true,
                }),
            ))
        } else {
            None
        }
    }

    /// Record a failed delivery, returning the lifecycle event (if any) to emit.
    fn on_failure(&self, error_msg: &str) -> Option<ApCoreEvent> {
        let now = (self.clock)();
        let new_count = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
        *self.last_failure_at.lock() = Some(now);

        let mut state_guard = self.state.lock();
        let opens = match *state_guard {
            CircuitState::HalfOpen => true,
            CircuitState::Closed => new_count >= self.open_threshold,
            CircuitState::Open => false,
        };
        if opens {
            *state_guard = CircuitState::Open;
            tracing::warn!(
                subscriber_type = %self.subscriber_type_name,
                consecutive_failures = new_count,
                error = %error_msg,
                "circuit opened for subscriber"
            );
            Some(self.make_event(
                "apcore.subscriber.circuit_opened",
                "warn",
                json!({
                    "subscriber_type": self.subscriber_type_name,
                    "consecutive_failures": new_count,
                }),
            ))
        } else {
            None
        }
    }

    fn make_event(&self, event_type: &str, severity: &str, data: serde_json::Value) -> ApCoreEvent {
        ApCoreEvent {
            event_type: event_type.to_string(),
            timestamp: (self.clock)().to_rfc3339(),
            data,
            module_id: None,
            severity: severity.to_string(),
        }
    }

    fn dispatch_circuit_event(&self, event: ApCoreEvent) {
        if let Some(sink) = &self.sink {
            sink.emit_circuit_event(event);
        }
    }
}

#[async_trait]
impl EventSubscriber for CircuitBreakerWrapper {
    fn subscriber_id(&self) -> &str {
        self.subscriber.subscriber_id()
    }

    fn event_pattern(&self) -> &str {
        self.subscriber.event_pattern()
    }

    fn event_type_filter(&self) -> Option<&str> {
        self.subscriber.event_type_filter()
    }

    async fn on_event(&self, event: &ApCoreEvent) -> Result<(), ModuleError> {
        self.check_recovery();
        if self.state() == CircuitState::Open {
            // Spec: deliver() is NOT called in OPEN state; events are silently discarded.
            return Ok(());
        }

        let timeout = Duration::from_millis(self.timeout_ms);
        let outcome = tokio::time::timeout(timeout, self.subscriber.on_event(event)).await;

        let circuit_event = match outcome {
            Ok(Ok(())) => self.on_success(),
            Ok(Err(e)) => self.on_failure(&e.to_string()),
            Err(_) => self.on_failure("delivery timeout"),
        };

        if let Some(ev) = circuit_event {
            self.dispatch_circuit_event(ev);
        }
        Ok(())
    }
}

/// Best-effort subscriber-type label for `apcore.subscriber.*` payloads.
///
/// We don't have access to the concrete generic type name at runtime, but the
/// `subscriber_id` carries a deterministic prefix for built-in types
/// (`webhook-…`, `a2a-…`, `file-…`, `stdout-…`, `filter-…`). Use that prefix
/// when present; otherwise fall back to the literal id.
fn guess_subscriber_type_name(subscriber: &dyn EventSubscriber) -> String {
    let id = subscriber.subscriber_id();
    if let Some(idx) = id.find('-') {
        id[..idx].to_string()
    } else {
        id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::ErrorCode;
    use serde_json::json;
    use std::sync::Mutex as StdMutex;

    #[derive(Debug, Default)]
    struct RecordingSink {
        events: StdMutex<Vec<ApCoreEvent>>,
    }

    impl RecordingSink {
        fn captured(&self) -> Vec<ApCoreEvent> {
            self.events.lock().unwrap().clone()
        }
    }

    impl CircuitEventSink for RecordingSink {
        fn emit_circuit_event(&self, event: ApCoreEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[derive(Debug)]
    struct AlwaysFail {
        id: String,
    }

    #[async_trait]
    impl EventSubscriber for AlwaysFail {
        fn subscriber_id(&self) -> &str {
            &self.id
        }
        #[allow(clippy::unnecessary_literal_bound)]
        fn event_pattern(&self) -> &str {
            "*"
        }
        async fn on_event(&self, _event: &ApCoreEvent) -> Result<(), ModuleError> {
            Err(ModuleError::new(ErrorCode::GeneralInternalError, "boom"))
        }
    }

    #[derive(Debug)]
    struct AlwaysOk {
        id: String,
    }

    #[async_trait]
    impl EventSubscriber for AlwaysOk {
        fn subscriber_id(&self) -> &str {
            &self.id
        }
        #[allow(clippy::unnecessary_literal_bound)]
        fn event_pattern(&self) -> &str {
            "*"
        }
        async fn on_event(&self, _event: &ApCoreEvent) -> Result<(), ModuleError> {
            Ok(())
        }
    }

    fn make_event() -> ApCoreEvent {
        ApCoreEvent::new("test.event", json!({}))
    }

    #[tokio::test]
    async fn opens_after_threshold() {
        let sink = Arc::new(RecordingSink::default());
        let wrapper = CircuitBreakerWrapper::new(
            Box::new(AlwaysFail {
                id: "webhook-x".into(),
            }),
            Some(sink.clone()),
        )
        .with_open_threshold(3)
        .with_subscriber_type_name("webhook");

        let event = make_event();
        for _ in 0..3 {
            wrapper.on_event(&event).await.unwrap();
        }

        assert_eq!(wrapper.state(), CircuitState::Open);
        assert_eq!(wrapper.consecutive_failures(), 3);
        let captured = sink.captured();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].event_type, "apcore.subscriber.circuit_opened");
    }

    #[tokio::test]
    async fn open_state_discards_without_delivery() {
        let sink = Arc::new(RecordingSink::default());
        let wrapper = CircuitBreakerWrapper::new(
            Box::new(AlwaysFail {
                id: "webhook-x".into(),
            }),
            Some(sink.clone()),
        );
        wrapper.force_state(CircuitState::Open);
        wrapper.force_last_failure_at(Some(Utc::now()));

        let event = make_event();
        wrapper.on_event(&event).await.unwrap();
        assert_eq!(wrapper.state(), CircuitState::Open);
        // No delivery attempted ⇒ no failure recorded.
        assert_eq!(wrapper.consecutive_failures(), 0);
    }

    #[tokio::test]
    async fn half_open_on_success_closes_and_emits() {
        let sink = Arc::new(RecordingSink::default());
        let wrapper = CircuitBreakerWrapper::new(
            Box::new(AlwaysOk {
                id: "webhook-x".into(),
            }),
            Some(sink.clone()),
        );
        wrapper.force_state(CircuitState::HalfOpen);

        wrapper.on_event(&make_event()).await.unwrap();
        assert_eq!(wrapper.state(), CircuitState::Closed);
        assert_eq!(wrapper.consecutive_failures(), 0);
        let captured = sink.captured();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].event_type, "apcore.subscriber.circuit_closed");
    }

    #[tokio::test]
    async fn check_recovery_transitions_open_to_half_open() {
        let wrapper = CircuitBreakerWrapper::new(
            Box::new(AlwaysOk {
                id: "webhook-x".into(),
            }),
            None,
        )
        .with_recovery_window_ms(30_000);
        wrapper.force_state(CircuitState::Open);
        let last = Utc::now() - chrono::Duration::seconds(31);
        wrapper.force_last_failure_at(Some(last));
        wrapper.check_recovery();
        assert_eq!(wrapper.state(), CircuitState::HalfOpen);
    }
}
