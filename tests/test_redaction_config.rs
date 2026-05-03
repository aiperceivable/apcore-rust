//! Issue #43 §5 — Config-driven redaction rules.
//!
//! `RedactionConfig::from_config()` reads
//!   - `observability.redaction.regex_patterns: Vec<String>`
//!   - `observability.redaction.sensitive_keys: Vec<String>` (glob patterns)
//!
//! and applies a default key-list (`_secret_*`, `api_key`, `token`,
//!     `authorization`, `password`) when the user-supplied list is missing.

use apcore::config::Config;
use apcore::observability::redaction::RedactionConfig;
use serde_json::json;

#[test]
fn defaults_redact_common_sensitive_keys() {
    let cfg = Config::from_defaults();
    let redaction = RedactionConfig::from_config(&cfg);

    let mut payload = json!({
        "api_key": "sk-test-1234",
        "token": "bearer-xyz",
        "authorization": "Bearer abc",
        "password": "hunter2",
        "_secret_db_url": "postgres://...",
        "username": "alice"
    });
    redaction.redact(&mut payload);

    assert_eq!(payload["api_key"], json!(redaction.replacement()));
    assert_eq!(payload["token"], json!(redaction.replacement()));
    assert_eq!(payload["authorization"], json!(redaction.replacement()));
    assert_eq!(payload["password"], json!(redaction.replacement()));
    assert_eq!(payload["_secret_db_url"], json!(redaction.replacement()));
    assert_eq!(payload["username"], json!("alice"));
}

#[test]
fn user_sensitive_keys_extend_defaults() {
    let mut cfg = Config::from_defaults();
    cfg.set(
        "observability.redaction.sensitive_keys",
        json!(["custom_token"]),
    );

    let redaction = RedactionConfig::from_config(&cfg);
    let mut payload = json!({
        "custom_token": "abc",
        "api_key": "should still match default",
        "username": "alice"
    });
    redaction.redact(&mut payload);
    assert_eq!(payload["custom_token"], json!(redaction.replacement()));
    assert_eq!(
        payload["api_key"],
        json!(redaction.replacement()),
        "default sensitive_keys should still apply"
    );
    assert_eq!(payload["username"], json!("alice"));
}

#[test]
fn user_regex_patterns_redact_values() {
    let mut cfg = Config::from_defaults();
    cfg.set(
        "observability.redaction.regex_patterns",
        json!([r"^Bearer\s+\S+"]),
    );

    let redaction = RedactionConfig::from_config(&cfg);
    let mut payload = json!({
        "url": "https://api.example.com/data",
        "auth_header": "Bearer abc123",
    });
    redaction.redact(&mut payload);
    assert_eq!(payload["auth_header"], json!(redaction.replacement()));
    assert_eq!(payload["url"], json!("https://api.example.com/data"));
}

#[test]
fn case_insensitive_regex() {
    let mut cfg = Config::from_defaults();
    cfg.set(
        "observability.redaction.regex_patterns",
        json!([r"secret-\w+"]),
    );

    let redaction = RedactionConfig::from_config(&cfg);
    let mut payload = json!({
        "raw": "SECRET-XYZ",
    });
    redaction.redact(&mut payload);
    assert_eq!(
        payload["raw"],
        json!(redaction.replacement()),
        "regex_patterns should compile case-insensitive"
    );
}
