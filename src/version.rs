// APCore Protocol — Version negotiation
// Spec reference: Protocol version compatibility checks

use crate::errors::{ModuleError, VersionIncompatibleError};

/// The current protocol version supported by this SDK.
pub const PROTOCOL_VERSION: &str = "0.13.0";

/// Negotiate a compatible protocol version between client and server.
///
/// Returns the negotiated version string, or an error if no compatible
/// version can be found.
pub fn negotiate_version(
    client_version: &str,
    server_versions: &[&str],
) -> Result<String, ModuleError> {
    // TODO: Implement — check semver compatibility
    todo!()
}

/// Check if two versions are compatible (same major, server minor >= client minor).
pub fn is_compatible(client_version: &str, server_version: &str) -> bool {
    // TODO: Implement
    todo!()
}
