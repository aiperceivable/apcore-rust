// APCore Protocol — Helper utilities
// Spec reference: Pattern matching, call chain guards, error propagation

use crate::context::Context;
use crate::errors::{ErrorCode, ModuleError};

/// Match a string against a glob-like pattern (supports `*` wildcards).
///
/// Ported from apcore-python `utils/pattern.py::match_pattern`.
pub fn match_pattern(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') {
        return pattern == value;
    }

    let segments: Vec<&str> = pattern.split('*').collect();
    let mut pos: usize = 0;

    // If pattern does not start with '*', value must start with the first segment.
    if !pattern.starts_with('*') {
        if !value.starts_with(segments[0]) {
            return false;
        }
        pos = segments[0].len();
    }

    // Check each interior segment can be found in order.
    for segment in &segments[1..] {
        if segment.is_empty() {
            continue;
        }
        match value[pos..].find(segment) {
            Some(idx) => {
                pos += idx + segment.len();
            }
            None => return false,
        }
    }

    // If pattern does not end with '*', value must end with the last segment.
    if !pattern.ends_with('*') && !value.ends_with(segments[segments.len() - 1]) {
        return false;
    }

    true
}

/// Guard against call depth and circular call violations.
///
/// - If `call_chain.len() >= max_depth`, returns `CallDepthExceeded`.
/// - If `module_name` appears >= `max_module_repeat` times in `call_chain`, returns `CallFrequencyExceeded`.
/// - If `module_name` is already in `call_chain`, returns `CircularCall`.
pub fn guard_call_chain(
    ctx: &Context<serde_json::Value>,
    module_name: &str,
    max_depth: u32,
) -> Result<(), ModuleError> {
    guard_call_chain_with_repeat(ctx, module_name, max_depth, 3)
}

/// Guard against call depth, frequency, and circular call violations with configurable repeat limit.
///
/// Note: Frequency is checked before circular calls. When `max_module_repeat` is 1,
/// a module appearing once triggers `CallFrequencyExceeded` rather than `CircularCall`.
/// This is intentional — the frequency limit subsumes circular detection at that threshold.
pub fn guard_call_chain_with_repeat(
    ctx: &Context<serde_json::Value>,
    module_name: &str,
    max_depth: u32,
    max_module_repeat: usize,
) -> Result<(), ModuleError> {
    // Check call depth (chain length must not exceed max_depth)
    if ctx.call_chain.len() as u32 > max_depth {
        return Err(ModuleError::new(
            ErrorCode::CallDepthExceeded,
            format!(
                "Call depth exceeded: chain length {} > max_depth {}",
                ctx.call_chain.len(),
                max_depth
            ),
        ));
    }

    // Circular detection: strict cycles of length >= 2 in prior chain.
    // The call chain's last entry is the current module (added by child()),
    // so check prior entries for a previous occurrence forming A->...->A.
    let prior = if ctx.call_chain.last().map(|s| s.as_str()) == Some(module_name) {
        &ctx.call_chain[..ctx.call_chain.len() - 1]
    } else {
        &ctx.call_chain[..]
    };
    if let Some(last_idx) = prior.iter().rposition(|n| n.as_str() == module_name) {
        let subsequence = &prior[last_idx + 1..];
        if !subsequence.is_empty() {
            return Err(ModuleError::new(
                ErrorCode::CircularCall,
                format!(
                    "Circular call detected: '{}' already in call chain {:?}",
                    module_name, ctx.call_chain
                ),
            ));
        }
    }

    // Frequency throttle: module must not appear more than max_module_repeat times.
    let count = ctx
        .call_chain
        .iter()
        .filter(|name| name.as_str() == module_name)
        .count();

    if count >= max_module_repeat {
        return Err(ModuleError::new(
            ErrorCode::CallFrequencyExceeded,
            format!(
                "Module '{}' called {} times, exceeds max repeat limit of {}",
                module_name, count, max_module_repeat
            ),
        ));
    }

    Ok(())
}

/// Propagate an error with additional context appended to the message.
pub fn propagate_error(error: ModuleError, context: &str) -> ModuleError {
    let mut new_error = error.clone();
    new_error.message = format!("{} [context: {}]", error.message, context);
    new_error
}

/// Convert a single segment to snake_case by detecting case boundaries.
///
/// Matches Algorithm A02 from the apcore protocol spec:
/// - Inserts `_` before an uppercase letter preceded by a lowercase/digit.
/// - Inserts `_` between consecutive uppercase letters when followed by a lowercase letter
///   (e.g., "HTTPClient" -> "http_client", "HTMLParser" -> "html_parser").
/// - Collapses any resulting double underscores.
fn to_snake_case(segment: &str) -> String {
    let chars: Vec<char> = segment.chars().collect();
    let mut result = String::with_capacity(segment.len() + 4);

    for (i, &ch) in chars.iter().enumerate() {
        if i > 0 {
            let prev = chars[i - 1];
            if (prev.is_lowercase() || prev.is_ascii_digit()) && ch.is_uppercase() {
                result.push('_');
            } else if prev.is_uppercase() && ch.is_uppercase() {
                if i + 1 < chars.len() && chars[i + 1].is_lowercase() {
                    result.push('_');
                }
            }
        }
        result.push(ch.to_lowercase().next().unwrap_or(ch));
    }

    result.replace("__", "_")
}

/// Language-specific separators used to split local IDs into segments.
fn separator_for_language(language: &str) -> &'static str {
    match language {
        "rust" => "::",
        // Python, Go, Java, TypeScript all use "."
        _ => ".",
    }
}

/// Normalize a local module identifier to its canonical dotted snake_case form.
///
/// Implements Algorithm A02 from the apcore protocol spec.
/// Splits `local_id` by the language-specific separator, converts each segment
/// to snake_case, and joins with `"."`.
pub fn normalize_to_canonical_id(local_id: &str, language: &str) -> String {
    let separator = separator_for_language(language);
    local_id
        .split(separator)
        .map(to_snake_case)
        .collect::<Vec<_>>()
        .join(".")
}

/// Calculate the specificity of a pattern for ACL rule ordering.
///
/// Ported from apcore-python `utils/pattern.py::calculate_specificity`.
/// - Wildcard-only `"*"` returns 0.
/// - Each dot-separated segment scores: exact literal = 2, partial wildcard = 1, pure `"*"` = 0.
pub fn calculate_specificity(pattern: &str) -> u32 {
    if pattern == "*" {
        return 0;
    }
    let mut score: u32 = 0;
    for segment in pattern.split('.') {
        if segment == "*" {
            // +0
        } else if segment.contains('*') {
            score += 1;
        } else {
            score += 2;
        }
    }
    score
}
