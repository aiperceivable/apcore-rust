// APCore Protocol — Access Control Lists
// Spec reference: ACL rules, audit entries

use serde::{Deserialize, Serialize};

use crate::context::Context;
use crate::errors::ModuleError;

/// Defines an access control rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ACLRule {
    #[serde(default)]
    pub callers: Vec<String>,
    #[serde(default)]
    pub targets: Vec<String>,
    pub effect: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<serde_json::Value>,
}

/// Access control list manager.
#[derive(Debug, Clone)]
pub struct ACL {
    rules: Vec<ACLRule>,
    default_effect: String,
}

impl ACL {
    /// Create a new ACL with the given rules and default effect.
    pub fn new(rules: Vec<ACLRule>, default_effect: impl Into<String>) -> Self {
        Self {
            rules,
            default_effect: default_effect.into(),
        }
    }

    /// Add a rule to the ACL.
    pub fn add_rule(&mut self, rule: ACLRule) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Remove rules matching the given callers and targets.
    pub fn remove_rule(&mut self, callers: &[String], targets: &[String]) -> bool {
        // TODO: Implement
        todo!()
    }

    /// Check whether the given caller is allowed to access the target.
    pub fn check(
        &self,
        caller_id: Option<&str>,
        target_id: &str,
        ctx: Option<&Context<serde_json::Value>>,
    ) -> Result<bool, ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Load ACL rules from a YAML/JSON file.
    pub fn load(path: &str) -> Result<Self, ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Reload rules from the source file.
    pub fn reload(&mut self) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }

    /// Return a reference to the current rules.
    pub fn rules(&self) -> &[ACLRule] {
        &self.rules
    }
}

impl Default for ACL {
    fn default() -> Self {
        Self::new(vec![], "deny")
    }
}
