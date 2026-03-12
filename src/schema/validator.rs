// APCore Protocol — Schema validator
// Spec reference: JSON Schema validation of module inputs/outputs

use crate::errors::ModuleError;
use crate::module::ValidationResult;

/// Validates data against JSON schemas.
#[derive(Debug)]
pub struct SchemaValidator;

impl SchemaValidator {
    /// Create a new schema validator.
    pub fn new() -> Self {
        Self
    }

    /// Validate a value against a JSON schema.
    pub fn validate(
        &self,
        value: &serde_json::Value,
        schema: &serde_json::Value,
    ) -> ValidationResult {
        // TODO: Implement
        todo!()
    }

    /// Validate and return a ModuleError on failure.
    pub fn validate_or_error(
        &self,
        value: &serde_json::Value,
        schema: &serde_json::Value,
    ) -> Result<(), ModuleError> {
        // TODO: Implement
        todo!()
    }
}

impl Default for SchemaValidator {
    fn default() -> Self {
        Self::new()
    }
}
