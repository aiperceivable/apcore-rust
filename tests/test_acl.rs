//! Tests for ACL types, construction, and check() behavior.

use apcore::acl::{ACLRule, ACL};
use apcore::context::{Context, Identity};
use serde_json::Value;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// ACL construction
// ---------------------------------------------------------------------------

#[test]
fn test_acl_new_is_empty() {
    let acl = ACL::new(vec![], "deny", None);
    assert!(acl.rules().is_empty());
}

#[test]
fn test_acl_default_is_empty() {
    let acl = ACL::default();
    assert!(acl.rules().is_empty());
}

// ---------------------------------------------------------------------------
// ACLRule construction
// ---------------------------------------------------------------------------

#[test]
fn test_acl_rule_fields() {
    let rule = ACLRule {
        callers: vec!["admin".to_string()],
        targets: vec!["admin.*".to_string()],
        effect: "allow".to_string(),
        description: None,
        conditions: None,
    };
    assert_eq!(rule.callers, vec!["admin"]);
    assert_eq!(rule.targets, vec!["admin.*"]);
    assert_eq!(rule.effect, "allow");
}

#[test]
fn test_acl_rule_deny() {
    let rule = ACLRule {
        callers: vec!["guest".to_string()],
        targets: vec!["*".to_string()],
        effect: "deny".to_string(),
        description: Some("Deny all guests".to_string()),
        conditions: None,
    };
    assert_eq!(rule.effect, "deny");
    assert_eq!(rule.description.as_deref(), Some("Deny all guests"));
}

#[test]
fn test_acl_rule_with_conditions() {
    let rule = ACLRule {
        callers: vec!["user".to_string()],
        targets: vec!["data.*".to_string()],
        effect: "allow".to_string(),
        description: None,
        conditions: Some(serde_json::json!({"ip_range": "10.0.0.0/8"})),
    };
    assert!(rule.conditions.is_some());
}

#[test]
fn test_acl_rule_serialization_round_trip() {
    let rule = ACLRule {
        callers: vec!["user".to_string(), "admin".to_string()],
        targets: vec!["user.*".to_string()],
        effect: "allow".to_string(),
        description: None,
        conditions: None,
    };
    let json = serde_json::to_string(&rule).unwrap();
    let restored: ACLRule = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.callers, rule.callers);
    assert_eq!(restored.targets, rule.targets);
    assert_eq!(restored.effect, rule.effect);
}

#[test]
fn test_acl_new_with_rules() {
    let rules = vec![ACLRule {
        callers: vec!["admin".to_string()],
        targets: vec!["*".to_string()],
        effect: "allow".to_string(),
        description: None,
        conditions: None,
    }];
    let acl = ACL::new(rules, "deny", None);
    assert_eq!(acl.rules().len(), 1);
}

// ---------------------------------------------------------------------------
// ACL.check() — allow rule matches
// ---------------------------------------------------------------------------

fn make_ctx(id: &str, id_type: &str, roles: Vec<String>) -> Context<Value> {
    Context::<Value>::new(Identity::new(
        id.to_string(),
        id_type.to_string(),
        roles,
        HashMap::default(),
    ))
}

#[test]
fn test_check_allow_rule_matches() {
    let rules = vec![ACLRule {
        callers: vec!["admin".to_string()],
        targets: vec!["secrets.*".to_string()],
        effect: "allow".to_string(),
        description: Some("Admin can access secrets".to_string()),
        conditions: None,
    }];
    let acl = ACL::new(rules, "deny", None);
    let ctx = make_ctx("admin", "user", vec![]);
    let result = acl.check(Some("admin"), "secrets.read", Some(&ctx));
    assert!(result, "Admin should be allowed to access secrets.*");
}

#[test]
fn test_check_allow_without_context() {
    let rules = vec![ACLRule {
        callers: vec!["bot".to_string()],
        targets: vec!["public.*".to_string()],
        effect: "allow".to_string(),
        description: None,
        conditions: None,
    }];
    let acl = ACL::new(rules, "deny", None);
    // check() with ctx=None should still match when there are no conditions
    let result = acl.check(Some("bot"), "public.info", None);
    assert!(result);
}

// ---------------------------------------------------------------------------
// ACL.check() — deny rule matches
// ---------------------------------------------------------------------------

#[test]
fn test_check_deny_rule_matches() {
    let rules = vec![ACLRule {
        callers: vec!["guest".to_string()],
        targets: vec!["admin.*".to_string()],
        effect: "deny".to_string(),
        description: Some("Guests cannot access admin".to_string()),
        conditions: None,
    }];
    let acl = ACL::new(rules, "allow", None);
    let ctx = make_ctx("guest", "user", vec![]);
    let result = acl.check(Some("guest"), "admin.panel", Some(&ctx));
    assert!(!result, "Guest should be denied access to admin.*");
}

// ---------------------------------------------------------------------------
// ACL.check() — default effect when no rules match
// ---------------------------------------------------------------------------

#[test]
fn test_check_default_deny_when_no_rules_match() {
    let rules = vec![ACLRule {
        callers: vec!["admin".to_string()],
        targets: vec!["admin.*".to_string()],
        effect: "allow".to_string(),
        description: None,
        conditions: None,
    }];
    let acl = ACL::new(rules, "deny", None);
    // "user1" does not match the "admin" caller pattern
    let result = acl.check(Some("user1"), "admin.panel", None);
    assert!(!result, "Should fall through to default deny");
}

#[test]
fn test_check_default_allow_when_no_rules_match() {
    let rules = vec![ACLRule {
        callers: vec!["blocked".to_string()],
        targets: vec!["*".to_string()],
        effect: "deny".to_string(),
        description: None,
        conditions: None,
    }];
    let acl = ACL::new(rules, "allow", None);
    // "friendly" does not match "blocked"
    let result = acl.check(Some("friendly"), "anything", None);
    assert!(result, "Should fall through to default allow");
}

#[test]
fn test_check_default_effect_with_empty_rules() {
    let acl_deny = ACL::new(vec![], "deny", None);
    assert!(!acl_deny.check(Some("anyone"), "anything", None));

    let acl_allow = ACL::new(vec![], "allow", None);
    assert!(acl_allow.check(Some("anyone"), "anything", None));
}

// ---------------------------------------------------------------------------
// ACL.check() — wildcard pattern matching
// ---------------------------------------------------------------------------

#[test]
fn test_check_wildcard_target_matches_all() {
    let rules = vec![ACLRule {
        callers: vec!["superadmin".to_string()],
        targets: vec!["*".to_string()],
        effect: "allow".to_string(),
        description: None,
        conditions: None,
    }];
    let acl = ACL::new(rules, "deny", None);
    assert!(acl.check(Some("superadmin"), "any.module.here", None));
    assert!(acl.check(Some("superadmin"), "another", None));
}

#[test]
fn test_check_wildcard_caller_matches_all() {
    let rules = vec![ACLRule {
        callers: vec!["*".to_string()],
        targets: vec!["public.health".to_string()],
        effect: "allow".to_string(),
        description: None,
        conditions: None,
    }];
    let acl = ACL::new(rules, "deny", None);
    assert!(acl.check(Some("anyone"), "public.health", None));
    assert!(acl.check(Some("someone_else"), "public.health", None));
}

#[test]
fn test_check_glob_pattern_in_target() {
    let rules = vec![ACLRule {
        callers: vec!["svc".to_string()],
        targets: vec!["data.*".to_string()],
        effect: "allow".to_string(),
        description: None,
        conditions: None,
    }];
    let acl = ACL::new(rules, "deny", None);
    assert!(acl.check(Some("svc"), "data.read", None));
    assert!(acl.check(Some("svc"), "data.write", None));
    assert!(
        !acl.check(Some("svc"), "admin.read", None),
        "Should not match non-data targets"
    );
}

#[test]
fn test_check_none_caller_maps_to_external() {
    let rules = vec![ACLRule {
        callers: vec!["@external".to_string()],
        targets: vec!["public.*".to_string()],
        effect: "allow".to_string(),
        description: None,
        conditions: None,
    }];
    let acl = ACL::new(rules, "deny", None);
    // None caller should be treated as @external
    assert!(acl.check(None, "public.api", None));
    // Explicit non-@external caller should not match
    assert!(!acl.check(Some("user1"), "public.api", None));
}

// ---------------------------------------------------------------------------
// ACL.check() — first-match-wins ordering
// ---------------------------------------------------------------------------

#[test]
fn test_check_first_match_wins_allow_before_deny() {
    let rules = vec![
        ACLRule {
            callers: vec!["user".to_string()],
            targets: vec!["resource".to_string()],
            effect: "allow".to_string(),
            description: Some("Allow first".to_string()),
            conditions: None,
        },
        ACLRule {
            callers: vec!["user".to_string()],
            targets: vec!["resource".to_string()],
            effect: "deny".to_string(),
            description: Some("Deny second".to_string()),
            conditions: None,
        },
    ];
    let acl = ACL::new(rules, "deny", None);
    let result = acl.check(Some("user"), "resource", None);
    assert!(result, "First matching rule (allow) should win");
}

#[test]
fn test_check_first_match_wins_deny_before_allow() {
    let rules = vec![
        ACLRule {
            callers: vec!["user".to_string()],
            targets: vec!["resource".to_string()],
            effect: "deny".to_string(),
            description: Some("Deny first".to_string()),
            conditions: None,
        },
        ACLRule {
            callers: vec!["user".to_string()],
            targets: vec!["resource".to_string()],
            effect: "allow".to_string(),
            description: Some("Allow second".to_string()),
            conditions: None,
        },
    ];
    let acl = ACL::new(rules, "allow", None);
    let result = acl.check(Some("user"), "resource", None);
    assert!(!result, "First matching rule (deny) should win");
}

#[test]
fn test_check_first_match_skips_non_matching_rules() {
    let rules = vec![
        ACLRule {
            callers: vec!["other".to_string()],
            targets: vec!["resource".to_string()],
            effect: "deny".to_string(),
            description: Some("Does not match caller".to_string()),
            conditions: None,
        },
        ACLRule {
            callers: vec!["user".to_string()],
            targets: vec!["resource".to_string()],
            effect: "allow".to_string(),
            description: Some("Matches".to_string()),
            conditions: None,
        },
    ];
    let acl = ACL::new(rules, "deny", None);
    let result = acl.check(Some("user"), "resource", None);
    assert!(
        result,
        "Should skip non-matching first rule and match second"
    );
}

#[test]
fn test_check_add_rule_inserts_at_front() {
    let mut acl = ACL::new(
        vec![ACLRule {
            callers: vec!["user".to_string()],
            targets: vec!["resource".to_string()],
            effect: "allow".to_string(),
            description: Some("Original allow".to_string()),
            conditions: None,
        }],
        "deny",
        None,
    );

    // add_rule inserts at position 0 — this deny rule should now be first
    acl.add_rule(ACLRule {
        callers: vec!["user".to_string()],
        targets: vec!["resource".to_string()],
        effect: "deny".to_string(),
        description: Some("Added deny".to_string()),
        conditions: None,
    })
    .unwrap();

    let result = acl.check(Some("user"), "resource", None);
    assert!(!result, "Newly added deny rule at front should win");
}

// ---------------------------------------------------------------------------
// A-D-302: ACL::new validates default_effect
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "invalid default_effect")]
fn test_acl_new_panics_on_invalid_default_effect() {
    // ACL::new must validate default_effect — bogus value should panic,
    // matching apcore-python and apcore-typescript constructor-throws
    // behaviour (sync finding A-D-302).
    let _ = ACL::new(vec![], "wrong_value", None);
}

#[test]
fn test_acl_new_accepts_allow_and_deny() {
    // Both legal values must construct successfully without panic.
    let _ = ACL::new(vec![], "allow", None);
    let _ = ACL::new(vec![], "deny", None);
}

#[test]
fn test_acl_load_propagates_invalid_default_effect_as_result() {
    // load() must propagate validation failures via Result rather than
    // panicking — YAML errors must not crash the host.
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new().expect("create tempfile");
    writeln!(tmp, "default_effect: not_a_real_effect\nrules: []\n").expect("write tempfile");
    let path = tmp.path().to_str().expect("utf8 path").to_string();
    let result = ACL::load(&path);
    assert!(
        result.is_err(),
        "load should error on invalid default_effect"
    );
}

// Regression: sync finding A-D-022 — structural ACL parse/validation
// errors carry `ErrorCode::ACLRuleError` per spec contract (apcore-python
// and apcore-typescript both raise `ACLRuleError`). Previously Rust used
// `ErrorCode::ConfigInvalid`, which broke cross-language fixtures
// asserting on the error code.
#[test]
fn test_acl_load_uses_acl_rule_error_for_parse_failures() {
    use apcore::errors::ErrorCode;
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new().expect("create tempfile");
    // Malformed YAML: stray colon in a scalar context
    writeln!(tmp, "rules: : :\n").expect("write tempfile");
    let path = tmp.path().to_str().expect("utf8 path").to_string();
    let err = ACL::load(&path).expect_err("load must error on malformed YAML");
    assert_eq!(
        err.code,
        ErrorCode::ACLRuleError,
        "structural ACL parse errors must surface as ACLRuleError, got {:?}",
        err.code
    );
}

#[test]
fn test_acl_load_uses_acl_rule_error_for_missing_rules_key() {
    use apcore::errors::ErrorCode;
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new().expect("create tempfile");
    // Valid YAML but no `rules` key.
    writeln!(tmp, "default_effect: deny\n").expect("write tempfile");
    let path = tmp.path().to_str().expect("utf8 path").to_string();
    let err = ACL::load(&path).expect_err("load must error on missing rules key");
    assert_eq!(err.code, ErrorCode::ACLRuleError);
}

// ---------------------------------------------------------------------------
// A-D-303: ACL::reload doesn't deadlock (borrow scope released before file IO)
// ---------------------------------------------------------------------------

#[test]
fn test_acl_reload_succeeds_from_yaml_path() {
    // Smoke test that reload() picks up changes to the file. The structural
    // requirement for A-D-303 is that the borrow of self.yaml_path ends
    // before Self::load is called; this test ensures the public behavior
    // works end-to-end.
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new().expect("create tempfile");
    writeln!(
        tmp,
        "default_effect: deny\nrules:\n  - callers: [\"user\"]\n    targets: [\"r\"]\n    effect: allow\n"
    )
    .expect("write tempfile");
    let path = tmp.path().to_str().expect("utf8").to_string();

    let mut acl = ACL::load(&path).expect("initial load");
    assert!(acl.check(Some("user"), "r", None));

    // Replace file: now deny everything.
    std::fs::write(
        &path,
        "default_effect: deny\nrules:\n  - callers: [\"user\"]\n    targets: [\"r\"]\n    effect: deny\n",
    )
    .expect("rewrite tempfile");

    acl.reload().expect("reload");
    assert!(!acl.check(Some("user"), "r", None));
}

// ---------------------------------------------------------------------------
// A-D-301: async_check snapshots rules + default_effect at entry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_async_check_uses_snapshot_consistent_with_sync() {
    // async_check must snapshot rules + default_effect at entry, mirroring
    // the sync check() snapshot. This test exercises the basic
    // first-match-wins behaviour through async_check to verify the
    // snapshot path produces the same decisions.
    let rules = vec![
        ACLRule {
            callers: vec!["user".to_string()],
            targets: vec!["resource".to_string()],
            effect: "deny".to_string(),
            description: Some("first deny".to_string()),
            conditions: None,
        },
        ACLRule {
            callers: vec!["user".to_string()],
            targets: vec!["resource".to_string()],
            effect: "allow".to_string(),
            description: Some("second allow".to_string()),
            conditions: None,
        },
    ];
    let acl = ACL::new(rules, "deny", None);
    let r = acl.async_check(Some("user"), "resource", None).await;
    assert!(!r, "First-match deny should win in async_check");
}

#[tokio::test]
async fn test_async_check_no_rules_path() {
    // No-rules path through async_check should also use the snapshotted
    // default_effect.
    let acl = ACL::new(vec![], "allow", None);
    let r = acl.async_check(Some("user"), "resource", None).await;
    assert!(r);

    let acl_deny = ACL::new(vec![], "deny", None);
    let r2 = acl_deny.async_check(Some("user"), "resource", None).await;
    assert!(!r2);
}
