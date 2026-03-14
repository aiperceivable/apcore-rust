//! Tests for ACL types, construction, and check() behavior.

use apcore::acl::{ACLRule, ACL};
use apcore::context::{Context, Identity};
use serde_json::Value;

// ---------------------------------------------------------------------------
// ACL construction
// ---------------------------------------------------------------------------

#[test]
fn test_acl_new_is_empty() {
    let acl = ACL::new(vec![], "deny");
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
    let acl = ACL::new(rules, "deny");
    assert_eq!(acl.rules().len(), 1);
}

// ---------------------------------------------------------------------------
// ACL.check() — allow rule matches
// ---------------------------------------------------------------------------

fn make_ctx(id: &str, id_type: &str, roles: Vec<String>) -> Context<Value> {
    Context::<Value>::new(Identity {
        id: id.to_string(),
        identity_type: id_type.to_string(),
        roles,
        attrs: Default::default(),
    })
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
    let acl = ACL::new(rules, "deny");
    let ctx = make_ctx("admin", "user", vec![]);
    let result = acl
        .check(Some("admin"), "secrets.read", Some(&ctx))
        .unwrap();
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
    let acl = ACL::new(rules, "deny");
    // check() with ctx=None should still match when there are no conditions
    let result = acl.check(Some("bot"), "public.info", None).unwrap();
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
    let acl = ACL::new(rules, "allow");
    let ctx = make_ctx("guest", "user", vec![]);
    let result = acl.check(Some("guest"), "admin.panel", Some(&ctx)).unwrap();
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
    let acl = ACL::new(rules, "deny");
    // "user1" does not match the "admin" caller pattern
    let result = acl.check(Some("user1"), "admin.panel", None).unwrap();
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
    let acl = ACL::new(rules, "allow");
    // "friendly" does not match "blocked"
    let result = acl.check(Some("friendly"), "anything", None).unwrap();
    assert!(result, "Should fall through to default allow");
}

#[test]
fn test_check_default_effect_with_empty_rules() {
    let acl_deny = ACL::new(vec![], "deny");
    assert!(!acl_deny.check(Some("anyone"), "anything", None).unwrap());

    let acl_allow = ACL::new(vec![], "allow");
    assert!(acl_allow.check(Some("anyone"), "anything", None).unwrap());
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
    let acl = ACL::new(rules, "deny");
    assert!(acl
        .check(Some("superadmin"), "any.module.here", None)
        .unwrap());
    assert!(acl.check(Some("superadmin"), "another", None).unwrap());
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
    let acl = ACL::new(rules, "deny");
    assert!(acl.check(Some("anyone"), "public.health", None).unwrap());
    assert!(acl
        .check(Some("someone_else"), "public.health", None)
        .unwrap());
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
    let acl = ACL::new(rules, "deny");
    assert!(acl.check(Some("svc"), "data.read", None).unwrap());
    assert!(acl.check(Some("svc"), "data.write", None).unwrap());
    assert!(
        !acl.check(Some("svc"), "admin.read", None).unwrap(),
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
    let acl = ACL::new(rules, "deny");
    // None caller should be treated as @external
    assert!(acl.check(None, "public.api", None).unwrap());
    // Explicit non-@external caller should not match
    assert!(!acl.check(Some("user1"), "public.api", None).unwrap());
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
    let acl = ACL::new(rules, "deny");
    let result = acl.check(Some("user"), "resource", None).unwrap();
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
    let acl = ACL::new(rules, "allow");
    let result = acl.check(Some("user"), "resource", None).unwrap();
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
    let acl = ACL::new(rules, "deny");
    let result = acl.check(Some("user"), "resource", None).unwrap();
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

    let result = acl.check(Some("user"), "resource", None).unwrap();
    assert!(!result, "Newly added deny rule at front should win");
}
