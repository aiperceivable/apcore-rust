// APCore Protocol — Approval workflow
// Spec reference: Approval requests, results, and handler trait

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::context::Context;
use crate::errors::ModuleError;
use crate::module::ModuleAnnotations;

/// Approval request sent before a sensitive operation.
/// Spec §7.3.1: required fields are module_id, arguments, context, annotations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub module_id: String,
    pub arguments: serde_json::Value,
    /// The execution context (trace_id, identity, call_chain).
    /// Skipped during serialization as Context contains non-serializable runtime refs.
    #[serde(skip)]
    pub context: Option<Context<serde_json::Value>>,
    /// Module behavior annotations (requires_approval is guaranteed true).
    #[serde(default)]
    pub annotations: ModuleAnnotations,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Outcome of an approval request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResult {
    /// "approved", "rejected", "timeout", or "pending"
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Trait for handling approval requests.
#[async_trait]
pub trait ApprovalHandler: Send + Sync + std::fmt::Debug {
    /// Request approval for an operation. Returns the result.
    async fn request_approval(
        &self,
        request: &ApprovalRequest,
    ) -> Result<ApprovalResult, ModuleError>;

    /// Check the current status of a pending approval by ID.
    async fn check_approval(&self, approval_id: &str) -> Result<ApprovalResult, ModuleError>;
}

/// An approval handler that automatically approves all requests.
#[derive(Debug, Clone)]
pub struct AutoApproveHandler;

#[async_trait]
impl ApprovalHandler for AutoApproveHandler {
    async fn request_approval(
        &self,
        _request: &ApprovalRequest,
    ) -> Result<ApprovalResult, ModuleError> {
        Ok(ApprovalResult {
            status: "approved".to_string(),
            approved_by: Some("auto".to_string()),
            reason: None,
            approval_id: None,
            metadata: None,
        })
    }

    async fn check_approval(&self, _approval_id: &str) -> Result<ApprovalResult, ModuleError> {
        Ok(ApprovalResult {
            status: "approved".to_string(),
            approved_by: Some("auto".to_string()),
            reason: None,
            approval_id: None,
            metadata: None,
        })
    }
}

/// An approval handler that automatically denies all requests.
#[derive(Debug, Clone)]
pub struct AlwaysDenyHandler;

#[async_trait]
impl ApprovalHandler for AlwaysDenyHandler {
    async fn request_approval(
        &self,
        _request: &ApprovalRequest,
    ) -> Result<ApprovalResult, ModuleError> {
        Ok(ApprovalResult {
            status: "rejected".to_string(),
            approved_by: None,
            reason: Some("Always denied".to_string()),
            approval_id: None,
            metadata: None,
        })
    }

    async fn check_approval(&self, _approval_id: &str) -> Result<ApprovalResult, ModuleError> {
        Ok(ApprovalResult {
            status: "rejected".to_string(),
            approved_by: None,
            reason: Some("Always denied".to_string()),
            approval_id: None,
            metadata: None,
        })
    }
}
