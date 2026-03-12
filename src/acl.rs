// APCore Protocol — Access Control Lists
// Spec reference: ACL rules, audit entries

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::context::Identity;
use crate::errors::ModuleError;

/// Defines an access control rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ACLRule {
    pub id: String,
    pub module_pattern: String,
    #[serde(default)]
    pub allowed_roles: Vec<String>,
    #[serde(default)]
    pub denied_roles: Vec<String>,
    #[serde(default)]
    pub allowed_identities: Vec<String>,
    #[serde(default)]
    pub denied_identities: Vec<String>,
    pub priority: i32,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Record of an ACL evaluation for audit purposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp: DateTime<Utc>,
    pub identity_id: String,
    pub module_name: String,
    pub rule_id: String,
    pub allowed: bool,
    pub reason: String,
}

/// Access control list manager.
#[derive(Debug, Clone)]
pub struct ACL {
    rules: Vec<ACLRule>,
    audit_log: Vec<AuditEntry>,
}

impl ACL {
    /// Create a new empty ACL.
    pub fn new() -> Self {
        Self {
            rules: vec![],
            audit_log: vec![],
        }
    }

    /// Add a rule to the ACL.
    pub fn add_rule(&mut self, rule: ACLRule) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Remove a rule by ID.
    pub fn remove_rule(&mut self, rule_id: &str) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Check whether the given identity is allowed to access the named module.
    pub fn check(&mut self, identity: &Identity, module_name: &str) -> Result<bool, ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Return the audit log entries.
    pub fn audit_log(&self) -> &[AuditEntry] {
        &self.audit_log
    }
}

impl Default for ACL {
    fn default() -> Self {
        Self::new()
    }
}
