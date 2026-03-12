//! Tests for ACL types and construction.

use apcore::acl::{ACLRule, ACL};

// ---------------------------------------------------------------------------
// ACL construction
// ---------------------------------------------------------------------------

#[test]
fn test_acl_new_is_empty() {
    let acl = ACL::new();
    assert!(acl.audit_log().is_empty());
}

#[test]
fn test_acl_default_is_empty() {
    let acl = ACL::default();
    assert!(acl.audit_log().is_empty());
}

// ---------------------------------------------------------------------------
// ACLRule construction
// ---------------------------------------------------------------------------

#[test]
fn test_acl_rule_fields() {
    let rule = ACLRule {
        id: "rule-1".to_string(),
        module_pattern: "admin.*".to_string(),
        allowed_roles: vec!["admin".to_string()],
        denied_roles: vec![],
        allowed_identities: vec![],
        denied_identities: vec![],
        priority: 10,
        enabled: true,
    };
    assert_eq!(rule.id, "rule-1");
    assert_eq!(rule.module_pattern, "admin.*");
    assert_eq!(rule.allowed_roles, vec!["admin"]);
    assert!(rule.enabled);
}

#[test]
fn test_acl_rule_disabled() {
    let rule = ACLRule {
        id: "rule-disabled".to_string(),
        module_pattern: "*".to_string(),
        allowed_roles: vec![],
        denied_roles: vec![],
        allowed_identities: vec![],
        denied_identities: vec![],
        priority: 0,
        enabled: false,
    };
    assert!(!rule.enabled);
}

#[test]
fn test_acl_rule_deny_pattern() {
    let rule = ACLRule {
        id: "deny-all".to_string(),
        module_pattern: "*".to_string(),
        allowed_roles: vec![],
        denied_roles: vec!["guest".to_string()],
        allowed_identities: vec![],
        denied_identities: vec!["anon-*".to_string()],
        priority: 100,
        enabled: true,
    };
    assert_eq!(rule.denied_roles, vec!["guest"]);
    assert_eq!(rule.denied_identities, vec!["anon-*"]);
}

#[test]
fn test_acl_rule_serialization_round_trip() {
    let rule = ACLRule {
        id: "rule-1".to_string(),
        module_pattern: "user.*".to_string(),
        allowed_roles: vec!["user".to_string(), "admin".to_string()],
        denied_roles: vec![],
        allowed_identities: vec![],
        denied_identities: vec![],
        priority: 5,
        enabled: true,
    };
    let json = serde_json::to_string(&rule).unwrap();
    let restored: ACLRule = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.id, rule.id);
    assert_eq!(restored.module_pattern, rule.module_pattern);
    assert_eq!(restored.allowed_roles, rule.allowed_roles);
    assert_eq!(restored.priority, rule.priority);
}

#[test]
fn test_acl_rule_priority_ordering() {
    let mut rules = vec![
        ACLRule {
            id: "low".to_string(),
            module_pattern: "*".to_string(),
            allowed_roles: vec![],
            denied_roles: vec![],
            allowed_identities: vec![],
            denied_identities: vec![],
            priority: 1,
            enabled: true,
        },
        ACLRule {
            id: "high".to_string(),
            module_pattern: "*".to_string(),
            allowed_roles: vec![],
            denied_roles: vec![],
            allowed_identities: vec![],
            denied_identities: vec![],
            priority: 100,
            enabled: true,
        },
    ];
    rules.sort_by_key(|r| std::cmp::Reverse(r.priority));
    assert_eq!(rules[0].id, "high");
}
