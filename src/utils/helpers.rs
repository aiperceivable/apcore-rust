// APCore Protocol — Helper utilities
// Spec reference: Pattern matching, call chain guards, error propagation

use crate::context::Context;
use crate::errors::{ErrorCode, ModuleError};

/// Default maximum call chain depth before `CallDepthExceeded` is returned.
pub const DEFAULT_MAX_CALL_DEPTH: usize = 32;
/// Default maximum repeat count for a single module in the call chain.
pub const DEFAULT_MAX_MODULE_REPEAT: usize = 3;

/// Match a string against a glob-like pattern (supports `*` wildcards).
///
/// Ported from apcore-python `utils/pattern.py::match_pattern`.
#[must_use]
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
    #[allow(clippy::cast_possible_truncation)]
    // call_chain length is bounded by max_depth which is u32
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
    let prior = if ctx.call_chain.last().map(std::string::String::as_str) == Some(module_name) {
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
                "Module '{module_name}' called {count} times, exceeds max repeat limit of {max_module_repeat}"
            ),
        ));
    }

    Ok(())
}

/// Convert a single segment to `snake_case` by detecting case boundaries.
///
/// Matches Algorithm A02 from the apcore protocol spec:
/// - Inserts `_` before an uppercase letter preceded by a lowercase/digit.
/// - Inserts `_` between consecutive uppercase letters when followed by a lowercase letter
///   (e.g., "`HTTPClient`" -> "`http_client`", "`HTMLParser`" -> "`html_parser`").
/// - Collapses any resulting double underscores.
fn to_snake_case(segment: &str) -> String {
    let chars: Vec<char> = segment.chars().collect();
    let mut result = String::with_capacity(segment.len() + 4);

    for (i, &ch) in chars.iter().enumerate() {
        if i > 0 {
            let prev = chars[i - 1];
            let boundary = if (prev.is_lowercase() || prev.is_ascii_digit()) && ch.is_uppercase() {
                true
            } else {
                prev.is_uppercase()
                    && ch.is_uppercase()
                    && i + 1 < chars.len()
                    && chars[i + 1].is_lowercase()
            };
            if boundary {
                result.push('_');
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

/// Normalize a local module identifier to its canonical dotted `snake_case` form.
///
/// Implements Algorithm A02 from the apcore protocol spec.
/// Splits `local_id` by the language-specific separator, converts each segment
/// to `snake_case`, and joins with `"."`.
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
#[must_use]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Context;
    use crate::errors::ErrorCode;

    #[test]
    fn test_match_pattern_wildcard_matches_everything() {
        assert!(match_pattern("*", "anything"));
        assert!(match_pattern("*", ""));
        assert!(match_pattern("*", "a.b.c"));
    }

    #[test]
    fn test_match_pattern_exact_match() {
        assert!(match_pattern("foo.bar", "foo.bar"));
        assert!(!match_pattern("foo.bar", "foo.baz"));
        assert!(!match_pattern("foo.bar", "foo.bar.baz"));
    }

    #[test]
    fn test_match_pattern_no_wildcards_no_match() {
        assert!(!match_pattern("abc", "def"));
    }

    #[test]
    fn test_match_pattern_prefix_wildcard() {
        assert!(match_pattern("foo.*", "foo.bar"));
        assert!(match_pattern("foo.*", "foo.anything"));
        assert!(!match_pattern("foo.*", "bar.baz"));
    }

    #[test]
    fn test_match_pattern_suffix_wildcard() {
        assert!(match_pattern("*.bar", "foo.bar"));
        assert!(match_pattern("*.bar", "x.y.bar"));
        assert!(!match_pattern("*.bar", "foo.baz"));
    }

    #[test]
    fn test_match_pattern_middle_wildcard() {
        assert!(match_pattern("a.*.c", "a.b.c"));
        assert!(match_pattern("a.*.c", "a.xyz.c"));
        assert!(!match_pattern("a.*.c", "a.b.d"));
    }

    #[test]
    fn test_match_pattern_multiple_wildcards() {
        assert!(match_pattern("a.*.*.d", "a.b.c.d"));
    }

    #[test]
    fn test_guard_call_chain_empty_chain_passes() {
        let ctx = Context::<serde_json::Value>::anonymous();
        assert!(guard_call_chain(&ctx, "mod.a", 10).is_ok());
    }

    #[test]
    fn test_guard_call_chain_depth_exceeded() {
        let mut ctx = Context::<serde_json::Value>::anonymous();
        ctx.call_chain = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        let result = guard_call_chain(&ctx, "e", 3);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::CallDepthExceeded);
    }

    #[test]
    fn test_guard_call_chain_circular_detection() {
        let mut ctx = Context::<serde_json::Value>::anonymous();
        ctx.call_chain = vec!["mod.a".into(), "mod.b".into(), "mod.a".into()];
        let result = guard_call_chain(&ctx, "mod.a", 100);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::CircularCall);
    }

    #[test]
    fn test_guard_call_chain_frequency_exceeded() {
        let mut ctx = Context::<serde_json::Value>::anonymous();
        // Three adjacent occurrences: no cycle of length >= 2 is detected
        // (subsequence between occurrences is empty), but count reaches
        // the default max_module_repeat of 3.
        ctx.call_chain = vec!["mod.a".into(), "mod.a".into(), "mod.a".into()];
        let result = guard_call_chain(&ctx, "mod.a", 100);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::CallFrequencyExceeded);
    }

    #[test]
    fn test_guard_call_chain_with_repeat_custom_limit() {
        let mut ctx = Context::<serde_json::Value>::anonymous();
        ctx.call_chain = vec!["mod.a".into()];
        let result = guard_call_chain_with_repeat(&ctx, "mod.a", 100, 1);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::CallFrequencyExceeded);
    }

    #[test]
    fn test_guard_call_chain_ok_within_limits() {
        let mut ctx = Context::<serde_json::Value>::anonymous();
        ctx.call_chain = vec!["mod.a".into(), "mod.b".into()];
        assert!(guard_call_chain(&ctx, "mod.c", 10).is_ok());
    }

    #[test]
    fn test_normalize_python_dotted() {
        assert_eq!(
            normalize_to_canonical_id("MyModule.SendEmail", "python"),
            "my_module.send_email"
        );
    }

    #[test]
    fn test_normalize_rust_double_colon() {
        assert_eq!(
            normalize_to_canonical_id("MyModule::SendEmail", "rust"),
            "my_module.send_email"
        );
    }

    #[test]
    fn test_normalize_already_snake_case() {
        assert_eq!(
            normalize_to_canonical_id("my_module.send_email", "python"),
            "my_module.send_email"
        );
    }

    #[test]
    fn test_normalize_acronym_handling() {
        assert_eq!(
            normalize_to_canonical_id("HTTPClient", "python"),
            "http_client"
        );
        assert_eq!(
            normalize_to_canonical_id("HTMLParser", "python"),
            "html_parser"
        );
    }

    #[test]
    fn test_normalize_camel_case_boundary() {
        assert_eq!(normalize_to_canonical_id("getValue", "python"), "get_value");
    }

    #[test]
    fn test_normalize_digit_boundary() {
        assert_eq!(normalize_to_canonical_id("log2Base", "python"), "log2_base");
    }

    #[test]
    fn test_specificity_wildcard_only() {
        assert_eq!(calculate_specificity("*"), 0);
    }

    #[test]
    fn test_specificity_exact_segments() {
        assert_eq!(calculate_specificity("foo.bar"), 4);
    }

    #[test]
    fn test_specificity_partial_wildcard() {
        assert_eq!(calculate_specificity("foo.*"), 2);
    }

    #[test]
    fn test_specificity_partial_wildcard_in_segment() {
        assert_eq!(calculate_specificity("foo.ba*"), 3);
    }

    #[test]
    fn test_specificity_single_exact() {
        assert_eq!(calculate_specificity("executor"), 2);
    }

    #[test]
    fn test_specificity_all_wildcards() {
        assert_eq!(calculate_specificity("*.*.*"), 0);
    }

    #[test]
    fn test_specificity_mixed() {
        assert_eq!(calculate_specificity("a.*.b.c*"), 5);
    }
}
