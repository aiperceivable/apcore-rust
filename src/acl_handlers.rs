// Built-in ACL condition handlers and handler trait.
//
// Defines the ACLConditionHandler trait, three basic handlers
// (identity_types, roles, max_call_depth), and two compound operators ($or, $not).

use async_trait::async_trait;
use parking_lot::RwLock;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock};

use crate::context::Context;

/// The three outcomes of evaluating an ACL condition (PROTOCOL_SPEC §6.1.1,
/// spec v1.22.0, apcore#100).
///
/// A condition that **is false** and a condition that **cannot be evaluated**
/// are different outcomes, and the difference decides what a `deny` rule does.
/// Collapsing them into a `bool` is the defect §6.1.1 exists to prevent: it
/// made a `deny` rule carrying a misspelled condition key silently inert.
///
/// | Rule `effect` | `Unsatisfied` | `Unevaluable` |
/// |---|---|---|
/// | `allow` | rule does not match → continue | rule does not match → continue (MUST NOT grant) |
/// | `deny`  | rule does not match → continue | rule **takes effect** → the call is denied |
///
/// Exactly three situations produce [`ConditionOutcome::Unevaluable`]:
/// the condition key has no registered handler; the handler panicked; or the
/// handler was asynchronous and could not be resolved on the synchronous
/// [`ACL::check`](crate::ACL::check) path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConditionOutcome {
    /// A registered handler ran to completion and returned `true`.
    Satisfied,
    /// A registered handler ran to completion and returned `false`. An
    /// ordinary non-match: evaluation continues with the next rule.
    Unsatisfied,
    /// No answer was obtainable at all (§6.1.1).
    Unevaluable,
}

impl ConditionOutcome {
    /// Map a handler's plain boolean answer onto the three-valued outcome.
    #[must_use]
    pub fn from_bool(value: bool) -> Self {
        if value {
            Self::Satisfied
        } else {
            Self::Unsatisfied
        }
    }

    /// `true` only for [`ConditionOutcome::Satisfied`].
    #[must_use]
    pub fn is_satisfied(self) -> bool {
        self == Self::Satisfied
    }

    /// `true` only for [`ConditionOutcome::Unevaluable`].
    #[must_use]
    pub fn is_unevaluable(self) -> bool {
        self == Self::Unevaluable
    }

    /// Three-valued (Kleene) AND, per §6.1.1's composition table:
    /// an outright `Unsatisfied` wins even against an unevaluable sibling;
    /// otherwise any `Unevaluable` child makes the conjunction unevaluable.
    #[must_use]
    pub fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unsatisfied, _) | (_, Self::Unsatisfied) => Self::Unsatisfied,
            (Self::Unevaluable, _) | (_, Self::Unevaluable) => Self::Unevaluable,
            (Self::Satisfied, Self::Satisfied) => Self::Satisfied,
        }
    }

    /// Three-valued (Kleene) OR, per §6.1.1's composition table:
    /// an outright `Satisfied` wins even against an unevaluable sibling;
    /// otherwise any `Unevaluable` child makes the disjunction unevaluable.
    #[must_use]
    pub fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::Satisfied, _) | (_, Self::Satisfied) => Self::Satisfied,
            (Self::Unevaluable, _) | (_, Self::Unevaluable) => Self::Unevaluable,
            (Self::Unsatisfied, Self::Unsatisfied) => Self::Unsatisfied,
        }
    }

    /// Three-valued (Kleene) NOT. `$not` of an unevaluable condition is
    /// **unevaluable**, never satisfied (§6.1.1) — negating "no answer" into
    /// "yes" would let a misspelled key inside a `$not` satisfy the very rule
    /// it was meant to gate.
    #[must_use]
    pub fn negate(self) -> Self {
        match self {
            Self::Satisfied => Self::Unsatisfied,
            Self::Unsatisfied => Self::Satisfied,
            Self::Unevaluable => Self::Unevaluable,
        }
    }
}

/// Per-call accumulation of unevaluable-condition diagnostics, keyed by the
/// offending condition key.
///
/// A `BTreeMap` rather than a `HashMap` on purpose: §6.1.1 rule 2 requires
/// `handler_error` to list every unevaluable condition **ordered
/// lexicographically by condition key**, not in evaluation order, because
/// evaluation order is not portable across SDKs. `BTreeMap` iterates in
/// ascending key order, so the required ordering is a property of the
/// container we chose and not an accident of whatever map `serde_json`
/// happens to use. See [`join_handler_errors`].
type HandlerErrors = BTreeMap<String, String>;

/// Join accumulated diagnostics into the single `AuditEntry.handler_error`
/// string, ordered lexicographically by condition key and separated by `"; "`
/// (PROTOCOL_SPEC §6.1.1 rule 2). Returns `None` when nothing was reported.
fn join_handler_errors(errors: &HandlerErrors) -> Option<String> {
    if errors.is_empty() {
        return None;
    }
    // Explicit: collect the values in ascending-key order, then join. The sort
    // is `BTreeMap`'s ordering invariant, restated here so the rule is visible
    // at the point it is applied.
    let mut ordered: Vec<(&String, &String)> = errors.iter().collect();
    ordered.sort_by(|(a, _), (b, _)| a.cmp(b));
    Some(
        ordered
            .into_iter()
            .map(|(_, message)| message.as_str())
            .collect::<Vec<_>>()
            .join("; "),
    )
}

// Per-call slot for the latest handler error message. Mirrors Python's
// `_handler_error_var` ContextVar and TypeScript's `_lastHandlerError`
// — a handler that detects an internal failure can record it here, and
// `ACL::build_audit_entry` reads it back when emitting the audit record.
//
// Python uses a single `contextvars.ContextVar` that works on both its sync
// and async `check()` paths. Rust has no single primitive with that property,
// so we keep TWO scopes that the read/write helpers consult transparently:
//
//   * `HANDLER_ERROR` — a tokio task-local, used by the async `async_check()`
//     path. Concurrent ACL evaluations on different tokio tasks cannot see
//     each other's errors.
//   * `HANDLER_ERROR_SYNC` — a thread-local, used by the synchronous `check()`
//     path (A-D-002). Tokio task-locals are unavailable on a purely
//     synchronous call, so the async-only scope left `handler_error` always
//     null on the sync path even when a condition handler reported an error.
//
// `report_handler_error` writes to whichever scope is active; `take_*` /
// `current_*` reads mirror that. A sync scope is only "active" inside
// `with_handler_error_capture_sync`, so a stray report outside any check is a
// no-op (matching the async task-local's behavior).
tokio::task_local! {
    pub(crate) static HANDLER_ERROR: RefCell<HandlerErrors>;
}

thread_local! {
    // `(active_depth, slot)`. The depth guards against a stray
    // `report_handler_error` leaking into an unrelated later read: writes and
    // reads only apply while a `with_handler_error_capture_sync` scope is open.
    static HANDLER_ERROR_SYNC: RefCell<(u32, HandlerErrors)> =
        const { RefCell::new((0, BTreeMap::new())) };
}

/// Record that condition `key` was **unevaluable** (PROTOCOL_SPEC §6.1.1).
///
/// Unlike [`report_handler_error`], the condition key is supplied separately so
/// the §6.1.1 rule 2 ordering ("lexicographically by condition key") is applied
/// to the key itself rather than to a formatted message. The stored message is
/// `"{key}: {reason}"`, matching the format all three SDKs emit.
pub(crate) fn report_condition_unevaluable(key: &str, reason: impl std::fmt::Display) {
    record_handler_error(key.to_string(), format!("{key}: {reason}"));
}

/// Insert one `(sort key, message)` pair into whichever capture scope is
/// active. Shared by [`report_handler_error`] and
/// [`report_condition_unevaluable`].
fn record_handler_error(key: String, message: String) {
    // Prefer the async task-local scope when present.
    let key_for_sync = key.clone();
    let message_for_sync = message.clone();
    if HANDLER_ERROR
        .try_with(move |cell| {
            cell.borrow_mut().insert(key, message);
        })
        .is_ok()
    {
        return;
    }
    // Fall back to the synchronous thread-local scope, if one is active.
    HANDLER_ERROR_SYNC.with(|cell| {
        let mut state = cell.borrow_mut();
        if state.0 > 0 {
            state.1.insert(key_for_sync, message_for_sync);
        }
    });
}

/// The condition keys reported as unevaluable in the active capture scope,
/// in ascending key order. Empty outside any scope.
///
/// Used by the ACL rule loop to name the offending keys in the §6.1.1 rule 3
/// warning, which must also carry the rule index and the rule's `effect` —
/// neither of which is known this far down the call stack.
pub(crate) fn reported_condition_keys() -> Vec<String> {
    if let Ok(keys) =
        HANDLER_ERROR.try_with(|cell| cell.borrow().keys().cloned().collect::<Vec<_>>())
    {
        if !keys.is_empty() {
            return keys;
        }
    }
    HANDLER_ERROR_SYNC.with(|cell| cell.borrow().1.keys().cloned().collect())
}

/// Record a handler-evaluation error for the current ACL check.
///
/// Cross-language parity with apcore-python `_handler_error_var.set(...)` and
/// apcore-typescript `_lastHandlerError = ...`. Writes to the active async
/// task-local scope if one exists, otherwise to the active synchronous
/// thread-local scope. If called outside any active capture scope (i.e.
/// outside an ACL check entirely), the call is a no-op so handlers never panic
/// on a missing scope.
/// Messages are accumulated, not overwritten: §6.1.1 rule 2 requires
/// `handler_error` to report **every** unevaluable condition in one `check()`,
/// ordered lexicographically by condition key. This free-form entry point has
/// no separate key argument, so the sort key is taken from the conventional
/// `"{key}: {reason}"` message prefix (everything before the first `':'`),
/// falling back to the whole message when it carries no colon. SDK-internal
/// call sites use [`report_condition_unevaluable`], which passes the key
/// explicitly.
pub fn report_handler_error(message: impl Into<String>) {
    let msg = message.into();
    let key = msg
        .split_once(':')
        .map_or_else(|| msg.clone(), |(prefix, _)| prefix.trim().to_string());
    record_handler_error(key, msg);
}

/// Read the handler error recorded for the current ACL check, if any.
///
/// Checks the async task-local scope first, then the synchronous thread-local
/// scope. Returns `None` outside any active capture scope. Used by
/// `ACL::build_audit_entry` so a handler error populates the audit entry on
/// both the sync and async paths (A-D-002).
pub(crate) fn current_handler_error() -> Option<String> {
    if let Ok(v) = HANDLER_ERROR.try_with(|cell| join_handler_errors(&cell.borrow())) {
        if v.is_some() {
            return v;
        }
    }
    HANDLER_ERROR_SYNC.with(|cell| join_handler_errors(&cell.borrow().1))
}

/// Run an async evaluation under a fresh handler-error capture scope.
///
/// The scope's final `Option<String>` is returned alongside the evaluation
/// result so callers can attach it to an `AuditEntry`. Mirrors the Python
/// `with _handler_error_var.set(None)` context-manager pattern.
pub async fn with_handler_error_capture<F, T>(fut: F) -> (T, Option<String>)
where
    F: std::future::Future<Output = T>,
{
    let cell = RefCell::new(HandlerErrors::new());
    let result = HANDLER_ERROR.scope(cell, async move {
        let value = fut.await;
        let captured = HANDLER_ERROR.with(|c| join_handler_errors(&c.borrow()));
        (value, captured)
    });
    result.await
}

/// Run a synchronous evaluation under a fresh handler-error capture scope.
///
/// Synchronous analogue of [`with_handler_error_capture`] for the sync
/// `ACL::check()` path (A-D-002). Opens a thread-local scope for the duration
/// of `f`, restoring the previous slot on exit so nested checks are isolated.
/// Mirrors Python's `_handler_error_var.set(None)` / `.reset(token)` pairing.
pub fn with_handler_error_capture_sync<F, T>(f: F) -> (T, Option<String>)
where
    F: FnOnce() -> T,
{
    // Open a fresh slot, saving the previous one so a nested check restores it.
    let previous = HANDLER_ERROR_SYNC.with(|cell| {
        let mut state = cell.borrow_mut();
        state.0 += 1;
        std::mem::take(&mut state.1)
    });

    let value = f();

    let captured = HANDLER_ERROR_SYNC.with(|cell| {
        let mut state = cell.borrow_mut();
        let captured = join_handler_errors(&state.1);
        state.0 -= 1;
        // Restore the enclosing scope's slot (empty at the outermost level).
        state.1 = previous;
        captured
    });

    (value, captured)
}

/// Extract a human-readable message from a panic payload.
///
/// Shared by the sync and async ACL condition-evaluation paths (A-D-011) so a
/// panicking custom handler produces a consistent `handler_error` string.
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// Trait for evaluating a single ACL condition.
#[async_trait]
pub trait ACLConditionHandler: Send + Sync {
    /// Answer the condition. `true` = satisfied, `false` = **unsatisfied**.
    ///
    /// A handler cannot report "unevaluable" through this method, and does not
    /// need to: PROTOCOL_SPEC §6.1.1's three unevaluable situations are all
    /// detected by the evaluator around the handler (no registration, a panic,
    /// or a future that is not ready on the synchronous path). Returning
    /// `false` therefore always means an ordinary non-match.
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool;

    /// Three-valued evaluation (PROTOCOL_SPEC §6.1.1).
    ///
    /// The default implementation maps [`Self::evaluate`] onto
    /// `Satisfied` / `Unsatisfied`, which is correct for every leaf handler.
    /// Only the compound operators `$or` and `$not` override it, because they
    /// have to *propagate* an unevaluable sub-condition rather than collapse
    /// it — `$not` of `Unevaluable` is `Unevaluable`, never `Satisfied`.
    ///
    /// Overriding is optional and additive: a handler written before spec
    /// v1.22.0 keeps working unchanged.
    async fn evaluate_outcome(&self, value: &Value, ctx: &Context<Value>) -> ConditionOutcome {
        ConditionOutcome::from_bool(self.evaluate(value, ctx).await)
    }
}

/// Global registry of condition handlers (sync entry point — consulted by
/// `ACL::check` and as the fallback for `async_check`).
pub static CONDITION_HANDLERS: LazyLock<RwLock<HashMap<String, Arc<dyn ACLConditionHandler>>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Separate registry for handlers explicitly registered as async-only via
/// `ACL::register_async_condition`. `async_check` consults this map first
/// and falls back to [`CONDITION_HANDLERS`] when no async-specific handler
/// is registered for a key. Mirrors apcore-python `_async_condition_handlers`
/// and apcore-typescript `_asyncConditionHandlers` (closes A-D-ACL-002).
pub static ASYNC_CONDITION_HANDLERS: LazyLock<
    RwLock<HashMap<String, Arc<dyn ACLConditionHandler>>>,
> = LazyLock::new(|| RwLock::new(HashMap::new()));

/// Register a condition handler globally. Replaces any existing handler for the same key.
///
/// An explicit registration always wins over the built-in for that key, in
/// either order: built-ins are seeded only where the key is unclaimed (see
/// `register_builtin_handlers`).
///
/// See also [`ACL::register_condition`](crate::ACL::register_condition) for the convenience
/// static method on the ACL type, which delegates here.
pub fn register_condition(key: impl Into<String>, handler: Arc<dyn ACLConditionHandler>) {
    let mut map = CONDITION_HANDLERS.write();
    map.insert(key.into(), handler);
}

/// Register an async-only condition handler.
///
/// Handlers registered here are consulted by [`evaluate_conditions_async`]
/// *before* the sync registry, allowing async-only logic to override a sync
/// handler with the same key without affecting the sync `ACL::check` path.
/// Cross-language parity with apcore-python `register_async_condition` and
/// apcore-typescript `registerAsyncCondition` (closes A-D-ACL-002).
pub fn register_async_condition(key: impl Into<String>, handler: Arc<dyn ACLConditionHandler>) {
    let mut map = ASYNC_CONDITION_HANDLERS.write();
    map.insert(key.into(), handler);
}

/// Whether `key` resolves on the **synchronous** [`ACL::check`](crate::ACL::check)
/// path — i.e. whether it is present in [`CONDITION_HANDLERS`].
///
/// PROTOCOL_SPEC §6.1.3 keeps the two registries deliberately separate: a key
/// registered only through [`register_async_condition`] is a working condition
/// under `async_check()` and an **unevaluable** one under `check()`.
#[must_use]
pub fn is_sync_registered(key: &str) -> bool {
    CONDITION_HANDLERS.read().contains_key(key)
}

/// Whether `key` resolves on the **asynchronous**
/// [`ACL::async_check`](crate::ACL::async_check) path.
///
/// `async_check()` consults [`ASYNC_CONDITION_HANDLERS`] first and falls back
/// to [`CONDITION_HANDLERS`], so a sync-only registration resolves here too
/// (PROTOCOL_SPEC §6.1.3).
#[must_use]
pub fn is_async_registered(key: &str) -> bool {
    ASYNC_CONDITION_HANDLERS.read().contains_key(key) || is_sync_registered(key)
}

// ---------------------------------------------------------------------------
// Free async evaluator (used by compound operators and re-exported to ACL)
// ---------------------------------------------------------------------------

/// One condition key paired with the handler it resolved to (if any) and its
/// configured value. `None` in the middle slot is PROTOCOL_SPEC §6.1.1
/// situation 1 — no registered handler, i.e. an unevaluable condition.
type ResolvedCondition = (String, Option<Arc<dyn ACLConditionHandler>>, Value);

/// Evaluate all conditions with AND logic using the handler registry.
///
/// Boolean façade over [`evaluate_conditions_async_outcome`], kept for callers
/// that only care whether the conditions were satisfied. An
/// [`ConditionOutcome::Unevaluable`] result maps to `false` here, which is
/// exactly the collapse PROTOCOL_SPEC §6.1.1 forbids the **rule loop** from
/// making — use the `_outcome` form anywhere the rule's `effect` matters.
pub async fn evaluate_conditions_async<S: ::std::hash::BuildHasher>(
    conditions: &HashMap<String, Value, S>,
    ctx: &Context<Value>,
) -> bool {
    evaluate_conditions_async_outcome(conditions, ctx)
        .await
        .is_satisfied()
}

/// Evaluate all conditions with three-valued AND logic (PROTOCOL_SPEC §6.1.1).
///
/// Resolves each condition key by consulting [`ASYNC_CONDITION_HANDLERS`]
/// first (async-only overrides), then falling back to [`CONDITION_HANDLERS`]
/// (the sync registry). A key with no handler in either registry is
/// **unevaluable**, not unsatisfied; so is a handler that panics. Handlers are
/// cloned out of the registries before any `.await` so no `parking_lot` read
/// guard is held across await points.
///
/// **Every** child is evaluated — no short-circuit. §6.1.1 permits
/// short-circuiting AND on the first `Unsatisfied`, but explicitly allows an
/// implementation to evaluate every child instead "for deterministic
/// diagnostics", and that is what we do: `handler_error` must list *all*
/// unevaluable conditions (rule 2), and a key skipped by a short-circuit would
/// make the audit entry depend on map iteration order. The decision is
/// identical either way — an outright `Unsatisfied` still wins the conjunction.
pub async fn evaluate_conditions_async_outcome<S: ::std::hash::BuildHasher>(
    conditions: &HashMap<String, Value, S>,
    ctx: &Context<Value>,
) -> ConditionOutcome {
    use futures_util::FutureExt;

    // Resolve handlers first; an unresolved key is a leaf outcome of its own.
    // `None` in the middle slot is PROTOCOL_SPEC §6.1.1 situation 1.
    let mut to_evaluate: Vec<ResolvedCondition> = Vec::with_capacity(conditions.len());
    {
        let async_handlers = ASYNC_CONDITION_HANDLERS.read();
        let sync_handlers = CONDITION_HANDLERS.read();
        for (key, value) in conditions {
            let handler = async_handlers
                .get(key.as_str())
                .or_else(|| sync_handlers.get(key.as_str()))
                .cloned();
            to_evaluate.push((key.clone(), handler, value.clone()));
        }
    }

    // AND over three-valued children, starting from the vacuous truth of an
    // empty `conditions` object.
    let mut outcome = ConditionOutcome::Satisfied;
    for (key, handler, value) in &to_evaluate {
        let Some(handler) = handler else {
            // §6.1.1 situation 1: no registered handler.
            tracing::warn!(
                condition = %key,
                "Unknown ACL condition — unevaluable (PROTOCOL_SPEC §6.1.1)"
            );
            report_condition_unevaluable(key, "unknown ACL condition");
            outcome = outcome.and(ConditionOutcome::Unevaluable);
            continue;
        };
        // §6.1.1 situation 2 (SECURITY, A-D-011): a panicking custom handler
        // must NOT unwind out of the ACL gate. Catch the panic, record it, and
        // report the condition unevaluable. Mirrors Python `try/except` and
        // TypeScript `try/catch` around handler.evaluate.
        let fut = std::panic::AssertUnwindSafe(handler.evaluate_outcome(value, ctx)).catch_unwind();
        let child = match fut.await {
            Ok(child) => child,
            Err(payload) => {
                let msg = panic_message(payload.as_ref());
                tracing::error!(
                    condition = %key,
                    panic = %msg,
                    "ACL condition handler panicked — unevaluable (PROTOCOL_SPEC §6.1.1)"
                );
                report_condition_unevaluable(key, format!("handler panicked: {msg}"));
                ConditionOutcome::Unevaluable
            }
        };
        outcome = outcome.and(child);
    }
    outcome
}

// ---------------------------------------------------------------------------
// Basic handlers
// ---------------------------------------------------------------------------

/// Check context.identity.type is in the allowed list.
pub struct IdentityTypesHandler;

#[async_trait]
impl ACLConditionHandler for IdentityTypesHandler {
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool {
        let Some(arr) = value.as_array() else {
            return false;
        };
        let Some(identity) = &ctx.identity else {
            return false;
        };
        arr.iter()
            .any(|v| v.as_str().is_some_and(|s| s == identity.identity_type()))
    }
}

/// Check at least one role overlaps between identity and required roles.
pub struct RolesHandler;

#[async_trait]
impl ACLConditionHandler for RolesHandler {
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool {
        let Some(arr) = value.as_array() else {
            return false;
        };
        let Some(identity) = &ctx.identity else {
            return false;
        };
        arr.iter().any(|v| {
            v.as_str()
                .is_some_and(|s| identity.roles().contains(&s.to_string()))
        })
    }
}

/// Extract a non-negative integer threshold from a JSON number, accepting
/// integral floats (`5.0` → `5`) but rejecting non-integral floats (`5.5`).
///
/// A-D-005: YAML/JSON configs frequently parse a bare `5` as a float (`5.0`).
/// `as_u64()` returns `None` for any float, which previously caused the handler
/// to fail-closed and reject a legitimate integral threshold. We now accept any
/// number whose value is a non-negative integer, matching apcore-typescript.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn integral_threshold(value: &Value) -> Option<u64> {
    let n = value.as_number()?;
    if let Some(u) = n.as_u64() {
        return Some(u);
    }
    let f = n.as_f64()?;
    // The `fract`/sign/range guards make the cast exact for accepted values;
    // precision loss only affects the `u64::MAX` bound comparison (harmless —
    // out-of-range values are rejected).
    if f.is_finite() && f.fract() == 0.0 && f >= 0.0 && f <= u64::MAX as f64 {
        return Some(f as u64);
    }
    None
}

/// Check call chain length does not exceed threshold.
///
/// Accepts both the bare-integer form `max_call_depth: 5` and the dict form
/// `max_call_depth: { lte: 5 }`, mirroring apcore-python and apcore-typescript
/// (sync finding A-D-024). Integral float thresholds (`5.0`) are accepted as the
/// equivalent integer (A-D-005). Non-integral floats and non-numeric forms are
/// rejected (fail-closed) per spec.
pub struct MaxCallDepthHandler;

#[async_trait]
impl ACLConditionHandler for MaxCallDepthHandler {
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool {
        let threshold = match value {
            Value::Number(_) => integral_threshold(value),
            Value::Object(map) => map.get("lte").and_then(integral_threshold),
            _ => None,
        };
        match threshold {
            Some(max) => (ctx.call_chain.len() as u64) <= max,
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Compound handlers
// ---------------------------------------------------------------------------

/// Which registry a compound operator resolves its sub-conditions from.
///
/// PROTOCOL_SPEC §6.1: sub-conditions MUST be evaluated in the same mode as the
/// enclosing call. A single instance cannot know how it was invoked (the trait
/// has one `async fn evaluate`), so two instances are registered per operator:
/// the `Sync` one into [`CONDITION_HANDLERS`] (reached by `ACL::check`) and the
/// `Async` one into [`ASYNC_CONDITION_HANDLERS`] (reached by
/// `ACL::async_check` / [`evaluate_conditions_async`], which consults the async
/// registry first).
///
/// Before this split, both operators were registered only in the sync registry
/// yet delegated to [`evaluate_conditions_async`], so a *sync* `ACL::check` on
/// `{$or: [{k: v}]}` resolved `k` from the ASYNC registry — where apcore-python
/// and apcore-typescript resolve it from the sync registry only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompoundMode {
    Sync,
    Async,
}

/// Evaluate a sub-condition set in the mode of the enclosing call, preserving
/// the three-valued outcome so `$or` / `$not` can propagate it (§6.1.1).
async fn evaluate_sub_conditions(
    mode: CompoundMode,
    map: &HashMap<String, Value>,
    ctx: &Context<Value>,
) -> ConditionOutcome {
    match mode {
        CompoundMode::Sync => crate::acl::ACL::evaluate_conditions(map, ctx),
        CompoundMode::Async => evaluate_conditions_async_outcome(map, ctx).await,
    }
}

fn sub_condition_map(obj: &serde_json::Map<String, Value>) -> HashMap<String, Value> {
    obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// $or: list of condition dicts. Returns true if ANY sub-set passes.
pub(crate) struct OrHandler {
    mode: CompoundMode,
}

impl OrHandler {
    pub(crate) fn new(mode: CompoundMode) -> Self {
        Self { mode }
    }
}

#[async_trait]
impl ACLConditionHandler for OrHandler {
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool {
        self.evaluate_outcome(value, ctx).await.is_satisfied()
    }

    /// §6.1.1: an outright `Satisfied` child wins even against an unevaluable
    /// sibling; otherwise any unevaluable child makes the whole `$or`
    /// unevaluable. Every child is evaluated (no short-circuit on the first
    /// `Satisfied`) so that `handler_error` reports every unevaluable sibling
    /// deterministically — the decision is unchanged either way.
    async fn evaluate_outcome(&self, value: &Value, ctx: &Context<Value>) -> ConditionOutcome {
        let Some(arr) = value.as_array() else {
            return ConditionOutcome::Unsatisfied;
        };
        // OR over three-valued children, starting from the identity of an
        // empty `$or: []` (which stays a non-match, as before).
        let mut outcome = ConditionOutcome::Unsatisfied;
        for sub in arr {
            let child = match sub.as_object() {
                Some(obj) => evaluate_sub_conditions(self.mode, &sub_condition_map(obj), ctx).await,
                None => ConditionOutcome::Unsatisfied,
            };
            outcome = outcome.or(child);
        }
        outcome
    }
}

/// $not: single condition dict. Returns true if the sub-set FAILS.
pub(crate) struct NotHandler {
    mode: CompoundMode,
}

impl NotHandler {
    pub(crate) fn new(mode: CompoundMode) -> Self {
        Self { mode }
    }
}

#[async_trait]
impl ACLConditionHandler for NotHandler {
    async fn evaluate(&self, value: &Value, ctx: &Context<Value>) -> bool {
        self.evaluate_outcome(value, ctx).await.is_satisfied()
    }

    /// §6.1.1: `$not` of an unevaluable sub-condition is **unevaluable**, never
    /// satisfied. Negating "no answer" into "yes" would let a misspelled key
    /// inside a `$not` satisfy the very rule it was meant to gate — the bypass
    /// this section exists to close, reintroduced one nesting level down.
    async fn evaluate_outcome(&self, value: &Value, ctx: &Context<Value>) -> ConditionOutcome {
        match value.as_object() {
            Some(obj) => evaluate_sub_conditions(self.mode, &sub_condition_map(obj), ctx)
                .await
                .negate(),
            None => ConditionOutcome::Unsatisfied,
        }
    }
}

/// Register all built-in handlers. Called once during initialization.
pub fn register_builtin_handlers() {
    seed_condition("identity_types", || Arc::new(IdentityTypesHandler));
    seed_condition("roles", || Arc::new(RolesHandler));
    seed_condition("max_call_depth", || Arc::new(MaxCallDepthHandler));
    // Mode-matched pairs — see `CompoundMode`.
    seed_condition("$or", || Arc::new(OrHandler::new(CompoundMode::Sync)));
    seed_condition("$not", || Arc::new(NotHandler::new(CompoundMode::Sync)));
    seed_async_condition("$or", || Arc::new(OrHandler::new(CompoundMode::Async)));
    seed_async_condition("$not", || Arc::new(NotHandler::new(CompoundMode::Async)));
}

/// Install a built-in handler ONLY if `key` is unclaimed.
///
/// Built-ins are a floor, not an override. apcore-python and apcore-typescript
/// seed theirs at module load, so a deployment's `register_condition` call
/// always lands afterwards and wins. Rust seeds lazily, from the first
/// `ACL::new` — with an unconditional insert, a stricter handler registered at
/// startup was silently replaced by the permissive built-in the moment the
/// first ACL was constructed, and the ACL then evaluated `roles` against the
/// built-in rule the deployment had deliberately replaced. Same call after the
/// first ACL survived, so the failure was ordering-dependent and invisible
/// (sync finding A-D-010).
fn seed_condition(key: &str, build: impl FnOnce() -> Arc<dyn ACLConditionHandler>) {
    let mut map = CONDITION_HANDLERS.write();
    map.entry(key.to_string()).or_insert_with(build);
}

/// [`seed_condition`] for the async-only registry.
fn seed_async_condition(key: &str, build: impl FnOnce() -> Arc<dyn ACLConditionHandler>) {
    let mut map = ASYNC_CONDITION_HANDLERS.write();
    map.entry(key.to_string()).or_insert_with(build);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Context, Identity};

    fn make_ctx(identity_type: &str, roles: Vec<&str>, call_depth: usize) -> Context<Value> {
        let identity = Identity::new(
            "test-id".to_string(),
            identity_type.to_string(),
            roles.into_iter().map(String::from).collect(),
            HashMap::new(),
        );
        let mut ctx = Context::new(identity);
        for i in 0..call_depth {
            ctx.call_chain.push(format!("module.{i}"));
        }
        ctx
    }

    fn anon_ctx() -> Context<Value> {
        Context::<Value>::anonymous()
    }

    // -------------------------------------------------------------------------
    // IdentityTypesHandler
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn identity_types_matches_correct_type() {
        let handler = IdentityTypesHandler;
        let ctx = make_ctx("user", vec![], 0);
        let value = serde_json::json!(["user", "service"]);
        assert!(handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn identity_types_rejects_wrong_type() {
        let handler = IdentityTypesHandler;
        let ctx = make_ctx("agent", vec![], 0);
        let value = serde_json::json!(["user", "service"]);
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn identity_types_rejects_non_array_value() {
        let handler = IdentityTypesHandler;
        let ctx = make_ctx("user", vec![], 0);
        let value = serde_json::json!("user"); // not an array
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn identity_types_rejects_no_identity() {
        let handler = IdentityTypesHandler;
        let ctx = anon_ctx();
        let value = serde_json::json!(["user"]);
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    // -------------------------------------------------------------------------
    // RolesHandler
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn roles_matches_overlapping_role() {
        let handler = RolesHandler;
        let ctx = make_ctx("user", vec!["admin", "viewer"], 0);
        let value = serde_json::json!(["admin"]);
        assert!(handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn roles_rejects_no_overlap() {
        let handler = RolesHandler;
        let ctx = make_ctx("user", vec!["viewer"], 0);
        let value = serde_json::json!(["admin"]);
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn roles_rejects_no_identity() {
        let handler = RolesHandler;
        let ctx = anon_ctx();
        let value = serde_json::json!(["admin"]);
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    // -------------------------------------------------------------------------
    // MaxCallDepthHandler
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn max_call_depth_allows_under_limit() {
        let handler = MaxCallDepthHandler;
        let ctx = make_ctx("user", vec![], 3);
        let value = serde_json::json!(5u64);
        assert!(handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn max_call_depth_allows_at_limit() {
        let handler = MaxCallDepthHandler;
        let ctx = make_ctx("user", vec![], 5);
        let value = serde_json::json!(5u64);
        assert!(handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn max_call_depth_rejects_over_limit() {
        let handler = MaxCallDepthHandler;
        let ctx = make_ctx("user", vec![], 6);
        let value = serde_json::json!(5u64);
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn max_call_depth_rejects_non_numeric_value() {
        let handler = MaxCallDepthHandler;
        let ctx = make_ctx("user", vec![], 0);
        let value = serde_json::json!("five"); // not a number
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn max_call_depth_accepts_integral_float_threshold() {
        // A-D-005: `max_call_depth: 5.0` is an integral float; it must be
        // treated as depth 5 (matches TS). Caller at depth 5 → ALLOW.
        let handler = MaxCallDepthHandler;
        let ctx = make_ctx("user", vec![], 5);
        let value = serde_json::json!(5.0);
        assert!(handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn max_call_depth_integral_float_rejects_over_limit() {
        let handler = MaxCallDepthHandler;
        let ctx = make_ctx("user", vec![], 6);
        let value = serde_json::json!(5.0);
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn max_call_depth_rejects_non_integral_float() {
        // 5.5 is not an integer threshold — fail-closed.
        let handler = MaxCallDepthHandler;
        let ctx = make_ctx("user", vec![], 5);
        let value = serde_json::json!(5.5);
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn max_call_depth_accepts_integral_float_in_lte_form() {
        let handler = MaxCallDepthHandler;
        let ctx = make_ctx("user", vec![], 5);
        let value = serde_json::json!({ "lte": 5.0 });
        assert!(handler.evaluate(&value, &ctx).await);
    }

    // -------------------------------------------------------------------------
    // OrHandler
    // -------------------------------------------------------------------------

    /// Simple async handler for compound-operator tests: checks `{"pass": true}`.
    struct PassHandler;

    #[async_trait]
    impl ACLConditionHandler for PassHandler {
        async fn evaluate(&self, value: &Value, _ctx: &Context<Value>) -> bool {
            value.as_bool().unwrap_or(false)
        }
    }

    /// A built-in must not replace a handler the deployment already claimed.
    ///
    /// Tested on `seed_condition` directly, with a key no other test uses: the
    /// registries are process-global, so driving this through `ACL::new` and a
    /// real built-in key would leave a window in which any concurrently
    /// running ACL test reads the wrong handler.
    #[test]
    fn seed_condition_does_not_replace_a_claimed_key() {
        const KEY: &str = "_test_seed_if_absent_rs";
        register_condition(KEY, Arc::new(PassHandler));

        let claimed = Arc::as_ptr(
            CONDITION_HANDLERS
                .read()
                .get(KEY)
                .expect("the explicit registration landed"),
        );

        seed_condition(KEY, || Arc::new(IdentityTypesHandler));

        let after = Arc::as_ptr(
            CONDITION_HANDLERS
                .read()
                .get(KEY)
                .expect("the key is still registered"),
        );
        assert!(
            std::ptr::addr_eq(claimed, after),
            "seeding replaced a handler that was already registered — a \
             deployment's stricter handler would be silently swapped for the \
             permissive built-in (A-D-010)"
        );
    }

    /// The same call installs the handler when the key is unclaimed.
    #[test]
    fn seed_condition_installs_when_the_key_is_free() {
        const KEY: &str = "_test_seed_when_free_rs";
        assert!(!CONDITION_HANDLERS.read().contains_key(KEY));
        seed_condition(KEY, || Arc::new(IdentityTypesHandler));
        assert!(CONDITION_HANDLERS.read().contains_key(KEY));
    }

    /// Guard the fix at its source: every built-in must go through the
    /// if-absent seed, never through the overwriting `register_condition`.
    ///
    /// The unit tests above pin `seed_condition`'s semantics; this pins that
    /// `register_builtin_handlers` still uses it. Rewriting one line back to
    /// `register_condition("roles", …)` would otherwise reintroduce A-D-010
    /// with both unit tests still green.
    #[test]
    fn built_ins_are_seeded_never_overwritten() {
        let src = include_str!("acl_handlers.rs");
        let start = src
            .find("pub fn register_builtin_handlers() {")
            .expect("register_builtin_handlers still exists");
        let body_end = src[start..]
            .find("\n}")
            .expect("its body is brace-terminated")
            + start;
        let body = &src[start..body_end];

        assert!(
            !body.contains("register_condition(") && !body.contains("register_async_condition("),
            "register_builtin_handlers overwrites instead of seeding — a \
             handler registered before the first ACL::new would be replaced \
             by the built-in (A-D-010):\n{body}"
        );
        assert_eq!(
            body.matches("seed_condition(").count() + body.matches("seed_async_condition(").count(),
            7,
            "expected all seven built-in registrations to be seeded — three \
             sync-only handlers plus the two mode-matched compound pairs:\n{body}"
        );
    }

    /// Register "pass" handler and ensure built-ins are present before compound tests.
    fn setup_compound_test_handlers() {
        register_condition("pass", Arc::new(PassHandler));
        // Ensure the OrHandler / NotHandler themselves are registered so nested
        // compound operators work in the integration tests below.
        register_builtin_handlers();
    }

    #[tokio::test]
    async fn or_handler_true_if_any_sub_passes() {
        setup_compound_test_handlers();
        let handler = OrHandler::new(CompoundMode::Async);
        let ctx = anon_ctx();
        let value = serde_json::json!([
            {"pass": false},
            {"pass": true},
        ]);
        assert!(handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn or_handler_false_if_none_pass() {
        setup_compound_test_handlers();
        let handler = OrHandler::new(CompoundMode::Async);
        let ctx = anon_ctx();
        let value = serde_json::json!([
            {"pass": false},
            {"pass": false},
        ]);
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn or_handler_rejects_non_array_value() {
        let handler = OrHandler::new(CompoundMode::Async);
        let ctx = anon_ctx();
        let value = serde_json::json!({"pass": true}); // not an array
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    // -------------------------------------------------------------------------
    // NotHandler
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn not_handler_inverts_passing_condition() {
        setup_compound_test_handlers();
        let handler = NotHandler::new(CompoundMode::Async);
        let ctx = anon_ctx();
        let value = serde_json::json!({"pass": true});
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn not_handler_inverts_failing_condition() {
        setup_compound_test_handlers();
        let handler = NotHandler::new(CompoundMode::Async);
        let ctx = anon_ctx();
        let value = serde_json::json!({"pass": false});
        assert!(handler.evaluate(&value, &ctx).await);
    }

    #[tokio::test]
    async fn not_handler_rejects_non_object_value() {
        let handler = NotHandler::new(CompoundMode::Async);
        let ctx = anon_ctx();
        let value = serde_json::json!([{"pass": true}]); // not an object
        assert!(!handler.evaluate(&value, &ctx).await);
    }

    // -------------------------------------------------------------------------
    // register_condition
    // -------------------------------------------------------------------------

    #[test]
    fn register_condition_stores_and_overwrites() {
        register_condition("_test_handler", Arc::new(MaxCallDepthHandler));
        // Overwrite — should not panic
        register_condition("_test_handler", Arc::new(MaxCallDepthHandler));
        let map = CONDITION_HANDLERS.read();
        assert!(map.contains_key("_test_handler"));
    }

    // -------------------------------------------------------------------------
    // register_async_condition — separate registry from sync (A-D-ACL-002)
    // -------------------------------------------------------------------------

    /// Async-only handler that always returns true.
    struct AsyncOnlyTrue;

    #[async_trait]
    impl ACLConditionHandler for AsyncOnlyTrue {
        async fn evaluate(&self, _value: &Value, _ctx: &Context<Value>) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn register_async_condition_uses_separate_registry() {
        // Register the same key in both registries with opposite outcomes —
        // the async path MUST consult the async registry first.
        struct SyncDeny;
        #[async_trait]
        impl ACLConditionHandler for SyncDeny {
            async fn evaluate(&self, _value: &Value, _ctx: &Context<Value>) -> bool {
                false
            }
        }

        let key = "_test_async_vs_sync";
        register_condition(key, Arc::new(SyncDeny));
        register_async_condition(key, Arc::new(AsyncOnlyTrue));

        // Async path resolves the async-only handler → true.
        let mut conditions: HashMap<String, Value> = HashMap::new();
        conditions.insert(key.to_string(), Value::Null);
        let ctx = anon_ctx();
        assert!(evaluate_conditions_async(&conditions, &ctx).await);

        // Sync registry still contains the deny handler.
        let sync_map = CONDITION_HANDLERS.read();
        assert!(sync_map.contains_key(key));
        let async_map = ASYNC_CONDITION_HANDLERS.read();
        assert!(async_map.contains_key(key));
    }

    #[tokio::test]
    async fn async_check_falls_back_to_sync_registry_when_no_async_handler() {
        // Only register on the sync side — async evaluation MUST still find it.
        let key = "_test_async_fallback";
        register_condition(key, Arc::new(AsyncOnlyTrue));

        let mut conditions: HashMap<String, Value> = HashMap::new();
        conditions.insert(key.to_string(), Value::Null);
        let ctx = anon_ctx();
        assert!(evaluate_conditions_async(&conditions, &ctx).await);
    }

    // -------------------------------------------------------------------------
    // handler_error capture (A-D-ACL-001)
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn handler_error_capture_returns_reported_message() {
        let (decision, captured) = with_handler_error_capture(async {
            report_handler_error("simulated handler failure");
            false
        })
        .await;
        assert!(!decision);
        assert_eq!(captured.as_deref(), Some("simulated handler failure"));
    }

    #[tokio::test]
    async fn handler_error_capture_isolates_per_scope() {
        // Two independent scopes must not see each other's errors.
        let ((), first) = with_handler_error_capture(async {
            report_handler_error("first call");
        })
        .await;
        let ((), second) = with_handler_error_capture(async {
            // No report inside this scope.
        })
        .await;
        assert_eq!(first.as_deref(), Some("first call"));
        assert!(second.is_none());
    }

    #[test]
    fn report_handler_error_outside_scope_is_noop() {
        // Calling outside an active capture scope must not panic — it falls
        // through silently because the task-local has no slot.
        report_handler_error("dropped on the floor");
    }
}
