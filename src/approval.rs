// APCore Protocol — Approval workflow
// Spec reference: Approval requests, results, and handler trait

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::context::Context;
use crate::errors::ModuleError;

/// Approval request sent before a sensitive operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: Uuid,
    pub module_name: String,
    pub action: String,
    pub description: String,
    pub requested_by: String,
    pub requested_at: DateTime<Utc>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Outcome of an approval request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalResult {
    pub request_id: Uuid,
    pub approved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
    pub decided_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Trait for handling approval requests.
#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    /// Request approval for an operation. Returns the result.
    async fn request_approval(
        &self,
        request: &ApprovalRequest,
        ctx: &Context<serde_json::Value>,
    ) -> Result<ApprovalResult, ModuleError>;
}

/// An approval handler that automatically approves all requests.
#[derive(Debug, Clone)]
pub struct AutoApproveHandler;

#[async_trait]
impl ApprovalHandler for AutoApproveHandler {
    async fn request_approval(
        &self,
        request: &ApprovalRequest,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<ApprovalResult, ModuleError> {
        // TODO: Implement
        todo!()
    }
}

/// An approval handler that automatically denies all requests.
#[derive(Debug, Clone)]
pub struct AutoDenyHandler;

#[async_trait]
impl ApprovalHandler for AutoDenyHandler {
    async fn request_approval(
        &self,
        request: &ApprovalRequest,
        _ctx: &Context<serde_json::Value>,
    ) -> Result<ApprovalResult, ModuleError> {
        // TODO: Implement
        todo!()
    }
}
