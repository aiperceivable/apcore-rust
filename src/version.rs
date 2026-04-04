// APCore Protocol — Version negotiation
// Spec reference: Protocol version compatibility checks

use crate::errors::{ErrorCode, ModuleError, VersionIncompatibleError};

/// The current protocol version supported by this SDK.
pub const PROTOCOL_VERSION: &str = "0.16.0";

/// Parse a semver string into (major, minor, patch).
fn parse_semver(version: &str) -> Option<(u64, u64, u64)> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let major = parts[0].parse::<u64>().ok()?;
    let minor = parts[1].parse::<u64>().ok()?;
    let patch = parts[2].parse::<u64>().ok()?;
    Some((major, minor, patch))
}

/// Negotiate a compatible protocol version between client and server.
///
/// Returns the negotiated version string, or an error if no compatible
/// version can be found.
pub fn negotiate_version(
    client_version: &str,
    server_versions: &[&str],
) -> Result<String, ModuleError> {
    let (client_major, client_minor, _) = parse_semver(client_version).ok_or_else(|| {
        ModuleError::new(
            ErrorCode::VersionIncompatible,
            format!("Invalid client version: {}", client_version),
        )
    })?;

    let mut best_match: Option<(u64, u64, u64, String)> = None;

    for &sv in server_versions {
        if let Some((major, minor, patch)) = parse_semver(sv) {
            if major == client_major && minor >= client_minor {
                match &best_match {
                    Some((_, bm_minor, bm_patch, _)) => {
                        // Pick the highest compatible version
                        if minor > *bm_minor || (minor == *bm_minor && patch > *bm_patch) {
                            best_match = Some((major, minor, patch, sv.to_string()));
                        }
                    }
                    None => {
                        best_match = Some((major, minor, patch, sv.to_string()));
                    }
                }
            }
        }
    }

    match best_match {
        Some((_, _, _, version)) => Ok(version),
        None => {
            let err = VersionIncompatibleError {
                message: format!(
                    "No compatible version found for client {} in server versions {:?}",
                    client_version, server_versions
                ),
            };
            Err(err.to_module_error())
        }
    }
}

/// Check if two versions are compatible (same major, server minor >= client minor).
pub fn is_compatible(client_version: &str, server_version: &str) -> bool {
    let client = parse_semver(client_version);
    let server = parse_semver(server_version);

    match (client, server) {
        (Some((c_major, c_minor, _)), Some((s_major, s_minor, _))) => {
            c_major == s_major && s_minor >= c_minor
        }
        _ => false,
    }
}
