//! Tests for ACL types and construction.

use apcore::acl::{ACLRule, ACL};

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
    let rules = vec![
        ACLRule {
            callers: vec!["admin".to_string()],
            targets: vec!["*".to_string()],
            effect: "allow".to_string(),
            description: None,
            conditions: None,
        },
    ];
    let acl = ACL::new(rules, "deny");
    assert_eq!(acl.rules().len(), 1);
}
