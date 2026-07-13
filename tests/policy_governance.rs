//! Integration tests for the execution-time governance policy (apcore#76 RFC
//! pilot) and its governance events (apcore#77 RFC pilot).
//!
//! Ported from apcore-python `tests/test_policy.py`. Covers the approval-gate
//! integration (policy forces/exempts approval, gate_destructive, strict
//! fail-closed, no-handler fail-loud, ApprovalRequest effective-annotations
//! contract), the three governance events, and the `validate()` preflight.
//! Pure `ExecutionPolicy`/`PolicyRule`/`PolicyDecision` unit tests live in
//! `src/policy.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{json, Value};

use apcore::acl::ACL;
use apcore::approval::{
    AlwaysDenyHandler, ApprovalHandler, ApprovalRequest, ApprovalResult, AutoApproveHandler,
};
use apcore::context::Context;
use apcore::errors::{ErrorCode, ModuleError};
use apcore::events::emitter::{ApCoreEvent, EventEmitter};
use apcore::events::subscribers::EventSubscriber;
use apcore::executor::Executor;
use apcore::module::{Module, ModuleAnnotations};
use apcore::registry::registry::{ModuleDescriptor, Registry, DEFAULT_MODULE_VERSION};
use apcore::{ExecutionPolicy, PolicyRule};

// ---------------------------------------------------------------------------
// Test module + harness
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct EchoModule;

#[async_trait]
impl Module for EchoModule {
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn description(&self) -> &'static str {
        "echo module"
    }
    async fn execute(&self, _inputs: Value, _ctx: &Context<Value>) -> Result<Value, ModuleError> {
        Ok(json!({"status": "executed"}))
    }
}

fn descriptor(module_id: &str, requires_approval: bool, destructive: bool) -> ModuleDescriptor {
    ModuleDescriptor {
        module_id: module_id.to_string(),
        name: None,
        description: "echo module".to_string(),
        documentation: None,
        input_schema: json!({"type": "object"}),
        output_schema: json!({"type": "object"}),
        version: DEFAULT_MODULE_VERSION.to_string(),
        tags: vec![],
        annotations: Some(ModuleAnnotations {
            requires_approval,
            destructive,
            ..ModuleAnnotations::default()
        }),
        examples: vec![],
        metadata: HashMap::new(),
        display: None,
        sunset_date: None,
        dependencies: vec![],
        enabled: true,
    }
}

/// Registry mirroring the Python fixture:
/// - `orders.list_orders` — plain module (no governance annotations)
/// - `orders.delete_order` — destructive=true, requires_approval=false (#76 footgun)
/// - `admin.reset` — requires_approval=true
fn make_registry() -> Arc<Registry> {
    let reg = Arc::new(Registry::new());
    reg.register(
        "orders.list_orders",
        Box::new(EchoModule),
        descriptor("orders.list_orders", false, false),
    )
    .unwrap();
    reg.register(
        "orders.delete_order",
        Box::new(EchoModule),
        descriptor("orders.delete_order", false, true),
    )
    .unwrap();
    reg.register(
        "admin.reset",
        Box::new(EchoModule),
        descriptor("admin.reset", true, false),
    )
    .unwrap();
    reg
}

fn executor(registry: Arc<Registry>) -> Executor {
    Executor::new(registry, apcore::config::Config::default())
}

/// Approval handler that records requests and returns a fixed status.
#[derive(Debug)]
struct RecordingHandler {
    requests: Arc<Mutex<Vec<ApprovalRequest>>>,
    status: String,
}

impl RecordingHandler {
    fn new(status: &str) -> (Self, Arc<Mutex<Vec<ApprovalRequest>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                requests: Arc::clone(&requests),
                status: status.to_string(),
            },
            requests,
        )
    }
}

#[async_trait]
impl ApprovalHandler for RecordingHandler {
    async fn request_approval(
        &self,
        request: &ApprovalRequest,
    ) -> Result<ApprovalResult, ModuleError> {
        self.requests.lock().push(request.clone());
        let mut result = ApprovalResult::default();
        result.status.clone_from(&self.status);
        Ok(result)
    }
    async fn check_approval(&self, _id: &str) -> Result<ApprovalResult, ModuleError> {
        let mut result = ApprovalResult::default();
        result.status = "rejected".to_string();
        result.reason = Some("not supported".to_string());
        Ok(result)
    }
}

/// Collects every emitted event (pattern `*`).
#[derive(Debug)]
struct CaptureSubscriber {
    events: Arc<Mutex<Vec<ApCoreEvent>>>,
}

#[async_trait]
impl EventSubscriber for CaptureSubscriber {
    fn subscriber_id(&self) -> &'static str {
        "policy-governance-capture"
    }
    fn event_pattern(&self) -> &'static str {
        "*"
    }
    async fn on_event(&self, event: &ApCoreEvent) -> Result<(), ModuleError> {
        self.events.lock().push(event.clone());
        Ok(())
    }
}

/// Build an executor with a capturing event emitter attached. Returns the
/// executor, a handle to the captured-events vec, and the emitter (for flush).
fn executor_with_emitter(
    registry: Arc<Registry>,
) -> (Executor, Arc<Mutex<Vec<ApCoreEvent>>>, Arc<EventEmitter>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let emitter = Arc::new(EventEmitter::new());
    emitter.subscribe(Box::new(CaptureSubscriber {
        events: Arc::clone(&events),
    }));
    let mut exec = executor(registry);
    exec.set_event_emitter(Some(Arc::clone(&emitter)));
    (exec, events, emitter)
}

fn events_of_type(events: &Arc<Mutex<Vec<ApCoreEvent>>>, event_type: &str) -> Vec<ApCoreEvent> {
    events
        .lock()
        .iter()
        .filter(|e| e.event_type == event_type)
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Approval gate integration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn policy_forces_approval_handler_consulted() {
    let (handler, requests) = RecordingHandler::new("approved");
    let mut exec = executor(make_registry());
    exec.set_approval_handler(Box::new(handler));
    exec.set_policy(Some(ExecutionPolicy::new(vec![PolicyRule::new(
        "orders.delete_*",
    )
    .unwrap()
    .with_requires_approval(true)
    .with_reason("sign-off")])));

    let out = exec
        .call("orders.delete_order", json!({}), None, None)
        .await
        .unwrap();
    assert_eq!(out["status"], "executed");
    assert_eq!(requests.lock().len(), 1);
    assert_eq!(requests.lock()[0].module_id, "orders.delete_order");
}

#[tokio::test]
async fn policy_forces_approval_deny_blocks() {
    let mut exec = executor(make_registry());
    exec.set_approval_handler(Box::new(AlwaysDenyHandler));
    exec.set_policy(Some(ExecutionPolicy::new(vec![PolicyRule::new(
        "orders.delete_*",
    )
    .unwrap()
    .with_requires_approval(true)])));

    let err = exec
        .call("orders.delete_order", json!({}), None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ApprovalDenied);
}

#[tokio::test]
async fn policy_exempts_module_handler_not_consulted() {
    let (handler, requests) = RecordingHandler::new("rejected");
    let mut exec = executor(make_registry());
    exec.set_approval_handler(Box::new(handler));
    exec.set_policy(Some(ExecutionPolicy::new(vec![PolicyRule::new(
        "admin.reset",
    )
    .unwrap()
    .with_requires_approval(false)])));

    let out = exec
        .call("admin.reset", json!({}), None, None)
        .await
        .unwrap();
    assert_eq!(out["status"], "executed");
    assert!(requests.lock().is_empty());
}

#[tokio::test]
async fn strict_policy_fails_closed_without_handler() {
    let mut exec = executor(make_registry());
    exec.set_policy(Some(ExecutionPolicy::new(vec![]).with_strict(true)));

    let err = exec
        .call("admin.reset", json!({}), None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ApprovalDenied);
    assert!(err.message.contains("fails closed"));
}

#[tokio::test]
async fn strict_policy_without_gated_modules_executes() {
    let mut exec = executor(make_registry());
    exec.set_policy(Some(ExecutionPolicy::new(vec![]).with_strict(true)));

    let out = exec
        .call("orders.list_orders", json!({}), None, None)
        .await
        .unwrap();
    assert_eq!(out["status"], "executed");
}

#[tokio::test]
async fn no_handler_skips_but_still_executes() {
    // Fail-loud default: requires_approval without a handler skips per spec §7.4
    // (warns once, not asserted here — Rust has no caplog) but still executes.
    let exec = executor(make_registry());
    for _ in 0..2 {
        let out = exec
            .call("admin.reset", json!({}), None, None)
            .await
            .unwrap();
        assert_eq!(out["status"], "executed");
    }
}

#[tokio::test]
async fn destructive_ungated_still_executes() {
    let exec = executor(make_registry());
    let out = exec
        .call("orders.delete_order", json!({}), None, None)
        .await
        .unwrap();
    assert_eq!(out["status"], "executed");
}

#[tokio::test]
async fn gate_destructive_covers_destructive_module() {
    let (handler, requests) = RecordingHandler::new("approved");
    let mut exec = executor(make_registry());
    exec.set_approval_handler(Box::new(handler));
    exec.set_policy(Some(
        ExecutionPolicy::new(vec![]).with_gate_destructive(true),
    ));

    let out = exec
        .call("orders.delete_order", json!({}), None, None)
        .await
        .unwrap();
    assert_eq!(out["status"], "executed");
    assert_eq!(requests.lock().len(), 1);
}

#[tokio::test]
async fn set_policy_at_runtime() {
    let mut exec = executor(make_registry());
    exec.set_approval_handler(Box::new(AlwaysDenyHandler));
    // No policy yet: orders.delete_order declares requires_approval=false, executes.
    assert_eq!(
        exec.call("orders.delete_order", json!({}), None, None)
            .await
            .unwrap()["status"],
        "executed"
    );

    exec.set_policy(Some(ExecutionPolicy::new(vec![PolicyRule::new(
        "orders.delete_*",
    )
    .unwrap()
    .with_requires_approval(true)])));
    let err = exec
        .call("orders.delete_order", json!({}), None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ApprovalDenied);

    exec.set_policy(None);
    assert_eq!(
        exec.call("orders.delete_order", json!({}), None, None)
            .await
            .unwrap()["status"],
        "executed"
    );
}

#[tokio::test]
async fn policy_gated_request_upholds_annotations_contract() {
    // ApprovalRequest.annotations must keep "requires_approval is guaranteed
    // true" (PROTOCOL_SPEC §7) even when the gate was policy-forced on a module
    // that declares requires_approval=false.
    let (handler, requests) = RecordingHandler::new("approved");
    let mut exec = executor(make_registry());
    exec.set_approval_handler(Box::new(handler));
    exec.set_policy(Some(ExecutionPolicy::new(vec![PolicyRule::new(
        "orders.delete_*",
    )
    .unwrap()
    .with_requires_approval(true)])));

    exec.call("orders.delete_order", json!({}), None, None)
        .await
        .unwrap();
    let reqs = requests.lock();
    assert_eq!(reqs.len(), 1);
    assert!(reqs[0].annotations.requires_approval); // effective, not the raw false
    assert!(reqs[0].annotations.destructive); // module's own value untouched
}

#[tokio::test]
async fn gate_destructive_request_upholds_annotations_contract() {
    let (handler, requests) = RecordingHandler::new("approved");
    let mut exec = executor(make_registry());
    exec.set_approval_handler(Box::new(handler));
    exec.set_policy(Some(
        ExecutionPolicy::new(vec![]).with_gate_destructive(true),
    ));

    exec.call("orders.delete_order", json!({}), None, None)
        .await
        .unwrap();
    let reqs = requests.lock();
    assert_eq!(reqs.len(), 1);
    assert!(reqs[0].annotations.requires_approval);
    assert!(reqs[0].annotations.destructive);
}

#[tokio::test]
async fn policy_destructive_override_visible_to_handler() {
    // A policy that marks a module destructive shows that to the handler.
    let (handler, requests) = RecordingHandler::new("approved");
    let mut exec = executor(make_registry());
    exec.set_approval_handler(Box::new(handler));
    exec.set_policy(Some(ExecutionPolicy::new(vec![PolicyRule::new(
        "admin.reset",
    )
    .unwrap()
    .with_destructive(true)])));

    exec.call("admin.reset", json!({}), None, None)
        .await
        .unwrap();
    let reqs = requests.lock();
    assert_eq!(reqs.len(), 1);
    assert!(reqs[0].annotations.requires_approval);
    assert!(reqs[0].annotations.destructive); // policy-effective, module declared false
}

// ---------------------------------------------------------------------------
// Governance events (apcore#77)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn approved_decision_event() {
    let (mut exec, events, emitter) = executor_with_emitter(make_registry());
    exec.set_approval_handler(Box::new(AutoApproveHandler));
    exec.call("admin.reset", json!({}), None, None)
        .await
        .unwrap();
    emitter.flush(2000).await.unwrap();

    let decisions = events_of_type(&events, "apcore.approval.decision");
    assert_eq!(decisions.len(), 1);
    let ev = &decisions[0];
    assert_eq!(ev.module_id.as_deref(), Some("admin.reset"));
    assert_eq!(ev.severity, "info");
    assert_eq!(ev.data["status"], "approved");
    assert_eq!(ev.data["approved_by"], "auto");
    assert!(ev.data["trace_id"].as_str().is_some_and(|s| !s.is_empty()));
}

#[tokio::test]
async fn rejected_decision_event_severity_warn() {
    let (mut exec, events, emitter) = executor_with_emitter(make_registry());
    exec.set_approval_handler(Box::new(AlwaysDenyHandler));
    let _ = exec.call("admin.reset", json!({}), None, None).await;
    emitter.flush(2000).await.unwrap();

    let decisions = events_of_type(&events, "apcore.approval.decision");
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].severity, "warn");
    assert_eq!(decisions[0].data["status"], "rejected");
}

#[tokio::test]
async fn strict_fail_closed_emits_decision_event() {
    let (mut exec, events, emitter) = executor_with_emitter(make_registry());
    exec.set_policy(Some(ExecutionPolicy::new(vec![]).with_strict(true)));
    let err = exec
        .call("admin.reset", json!({}), None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ApprovalDenied);
    emitter.flush(2000).await.unwrap();

    let decisions = events_of_type(&events, "apcore.approval.decision");
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].severity, "warn");
    assert_eq!(decisions[0].data["status"], "rejected");
}

#[tokio::test]
async fn policy_override_event() {
    let (mut exec, events, emitter) = executor_with_emitter(make_registry());
    exec.set_approval_handler(Box::new(AutoApproveHandler));
    exec.set_policy(Some(ExecutionPolicy::new(vec![PolicyRule::new(
        "orders.delete_*",
    )
    .unwrap()
    .with_requires_approval(true)
    .with_reason("sign-off")])));

    exec.call("orders.delete_order", json!({}), None, None)
        .await
        .unwrap();
    emitter.flush(2000).await.unwrap();

    let overrides = events_of_type(&events, "apcore.policy.override");
    assert_eq!(overrides.len(), 1);
    let data = &overrides[0].data;
    assert_eq!(data["module_id"], "orders.delete_order");
    assert_eq!(data["pattern"], "orders.delete_*");
    assert_eq!(data["requires_approval"], true);
    assert_eq!(data["needs_approval"], true);
    assert_eq!(data["reason"], "sign-off");
    // The final adjudication is a separate event.
    assert_eq!(events_of_type(&events, "apcore.approval.decision").len(), 1);
}

#[tokio::test]
async fn no_events_when_gate_not_involved() {
    let (mut exec, events, emitter) = executor_with_emitter(make_registry());
    exec.set_approval_handler(Box::new(AutoApproveHandler));
    exec.call("orders.list_orders", json!({}), None, None)
        .await
        .unwrap();
    emitter.flush(2000).await.unwrap();

    assert!(events_of_type(&events, "apcore.approval.decision").is_empty());
    assert!(events_of_type(&events, "apcore.policy.override").is_empty());
}

#[tokio::test]
async fn no_decision_event_when_gate_skipped_without_handler() {
    // Non-strict skip (spec §7.4) emits no decision event.
    let (exec, events, emitter) = executor_with_emitter(make_registry());
    exec.call("admin.reset", json!({}), None, None)
        .await
        .unwrap();
    emitter.flush(2000).await.unwrap();
    assert!(events_of_type(&events, "apcore.approval.decision").is_empty());
}

#[tokio::test]
async fn acl_denied_event() {
    let (mut exec, events, emitter) = executor_with_emitter(make_registry());
    exec.set_acl(ACL::new(vec![], "deny", None));
    let err = exec
        .call("orders.list_orders", json!({}), None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ACLDenied);
    emitter.flush(2000).await.unwrap();

    let denied = events_of_type(&events, "apcore.acl.denied");
    assert_eq!(denied.len(), 1);
    assert_eq!(denied[0].severity, "warn");
    assert_eq!(denied[0].data["module_id"], "orders.list_orders");
    assert!(denied[0].data.get("caller_id").is_some());
    assert!(denied[0].data["trace_id"]
        .as_str()
        .is_some_and(|s| !s.is_empty()));
}

#[tokio::test]
async fn acl_denied_event_not_emitted_in_preflight() {
    // validate() runs the ACL step in dry_run — it must NOT emit a denial event.
    let (mut exec, events, emitter) = executor_with_emitter(make_registry());
    exec.set_acl(ACL::new(vec![], "deny", None));
    let _ = exec.validate("orders.list_orders", &json!({}), None).await;
    emitter.flush(2000).await.unwrap();
    assert!(events_of_type(&events, "apcore.acl.denied").is_empty());
}

// ---------------------------------------------------------------------------
// validate() preflight reflects policy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn validate_reports_policy_forced_approval() {
    let mut exec = executor(make_registry());
    exec.set_policy(Some(ExecutionPolicy::new(vec![PolicyRule::new(
        "orders.delete_*",
    )
    .unwrap()
    .with_requires_approval(true)])));
    let preflight = exec
        .validate("orders.delete_order", &json!({}), None)
        .await
        .unwrap();
    assert!(preflight.requires_approval);
}

#[tokio::test]
async fn validate_reports_gate_destructive() {
    let mut exec = executor(make_registry());
    exec.set_policy(Some(
        ExecutionPolicy::new(vec![]).with_gate_destructive(true),
    ));
    let preflight = exec
        .validate("orders.delete_order", &json!({}), None)
        .await
        .unwrap();
    assert!(preflight.requires_approval);
}

#[tokio::test]
async fn validate_reports_policy_exemption() {
    let mut exec = executor(make_registry());
    exec.set_policy(Some(ExecutionPolicy::new(vec![PolicyRule::new(
        "admin.reset",
    )
    .unwrap()
    .with_requires_approval(false)])));
    let preflight = exec
        .validate("admin.reset", &json!({}), None)
        .await
        .unwrap();
    assert!(!preflight.requires_approval);
}

#[tokio::test]
async fn validate_without_policy_unchanged() {
    let exec = executor(make_registry());
    assert!(
        exec.validate("admin.reset", &json!({}), None)
            .await
            .unwrap()
            .requires_approval
    );
    assert!(
        !exec
            .validate("orders.list_orders", &json!({}), None)
            .await
            .unwrap()
            .requires_approval
    );
}
