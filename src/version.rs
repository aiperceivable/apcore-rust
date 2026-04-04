// APCore Protocol — Version negotiation
// Spec reference: Protocol version compatibility checks

use crate::errors::{ErrorCode, ModuleError, VersionIncompatibleError};

/// The current protocol version supported by this SDK.
pub const PROTOCOL_VERSION: &str = "0.16.0";

/// A parsed semver version with optional prerelease suffix.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SemVer {
    major: u64,
    minor: u64,
    patch: u64,
    /// `None` means a release version; `Some(tag)` means a prerelease.
    prerelease: Option<String>,
}

impl SemVer {
    /// Format back to a version string.
    fn to_string_repr(&self) -> String {
        match &self.prerelease {
            Some(pre) => format!("{}.{}.{}-{}", self.major, self.minor, self.patch, pre),
            None => format!("{}.{}.{}", self.major, self.minor, self.patch),
        }
    }
}

/// Semver ordering: major > minor > patch, then prerelease < release.
/// Among prereleases with the same major.minor.patch, compare lexicographically.
impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
            .then(match (&self.prerelease, &other.prerelease) {
                (None, None) => std::cmp::Ordering::Equal,
                // Release > prerelease
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Parse a semver string into a `SemVer` struct.
///
/// Accepts versions like `"1.2.3"` and `"1.2.3-alpha"`.
fn parse_semver(version: &str) -> Option<SemVer> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let major = parts[0].parse::<u64>().ok()?;
    let minor = parts[1].parse::<u64>().ok()?;

    // The patch segment may contain a prerelease suffix: "3-alpha"
    let (patch, prerelease) = if let Some(dash_pos) = parts[2].find('-') {
        let patch_str = &parts[2][..dash_pos];
        let pre_str = &parts[2][dash_pos + 1..];
        if pre_str.is_empty() {
            return None;
        }
        (patch_str.parse::<u64>().ok()?, Some(pre_str.to_string()))
    } else {
        (parts[2].parse::<u64>().ok()?, None)
    };

    Some(SemVer {
        major,
        minor,
        patch,
        prerelease,
    })
}

/// Negotiate a compatible protocol version between a declared version and
/// the SDK version (Algorithm A14).
///
/// Returns the effective version string, or an error if the versions are
/// incompatible (major mismatch or declared minor exceeds SDK minor).
pub fn negotiate_version(
    declared_version: &str,
    sdk_version: &str,
) -> Result<String, ModuleError> {
    let declared = parse_semver(declared_version).ok_or_else(|| {
        ModuleError::new(
            ErrorCode::VersionIncompatible,
            format!("Invalid declared version: {}", declared_version),
        )
    })?;

    let sdk = parse_semver(sdk_version).ok_or_else(|| {
        ModuleError::new(
            ErrorCode::VersionIncompatible,
            format!("Invalid SDK version: {}", sdk_version),
        )
    })?;

    // Major mismatch -> error
    if declared.major != sdk.major {
        let err = VersionIncompatibleError {
            message: format!(
                "Major version mismatch: declared {} vs SDK {}",
                declared_version, sdk_version
            ),
        };
        return Err(err.to_module_error());
    }

    // Declared minor > SDK minor -> error (SDK too old)
    if declared.minor > sdk.minor {
        let err = VersionIncompatibleError {
            message: format!(
                "Declared minor version {} exceeds SDK minor version {} (SDK too old)",
                declared.minor, sdk.minor
            ),
        };
        return Err(err.to_module_error());
    }

    // Declared minor < SDK minor -> return declared (backward compat)
    if declared.minor < sdk.minor {
        return Ok(declared_version.to_string());
    }

    // Same minor -> return max(declared, sdk)
    let effective = std::cmp::max(&declared, &sdk);
    Ok(effective.to_string_repr())
}

/// Check if two versions are compatible (same major, server minor >= client minor).
pub fn is_compatible(client_version: &str, server_version: &str) -> bool {
    let client = parse_semver(client_version);
    let server = parse_semver(server_version);

    match (client, server) {
        (Some(c), Some(s)) => c.major == s.major && s.minor >= c.minor,
        _ => false,
    }
}
