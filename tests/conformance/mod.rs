// Cross-language conformance tests driven by canonical JSON fixtures.
//
// Fixture source: apcore/conformance/fixtures/*.json (single source of truth).
//
// Fixture discovery order:
//   1. APCORE_SPEC_REPO env var
//   2. Sibling ../apcore/ directory (standard workspace layout & CI)

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

use apcore::acl::{ACLRule, ACL};
use apcore::context::{Context, Identity};
use apcore::utils::{calculate_specificity, guard_call_chain_with_repeat, match_pattern};

fn find_fixtures_root() -> PathBuf {
    // 1. APCORE_SPEC_REPO env var
    if let Ok(spec_repo) = std::env::var("APCORE_SPEC_REPO") {
        let p = PathBuf::from(&spec_repo).join("conformance").join("fixtures");
        if p.is_dir() {
            return p;
        }
        panic!(
            "APCORE_SPEC_REPO={} does not contain conformance/fixtures/",
            spec_repo
        );
    }

    // 2. Sibling ../apcore/ directory
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let sibling = manifest_dir
        .parent()
        .unwrap()
        .join("apcore")
        .join("conformance")
        .join("fixtures");
    if sibling.is_dir() {
        return sibling;
    }

    panic!(
        "Cannot find apcore conformance fixtures.\n\
         Fix one of:\n\
         1. Set APCORE_SPEC_REPO to the apcore spec repo path\n\
         2. Clone apcore as a sibling: git clone <apcore-url> {}\n",
        manifest_dir.parent().unwrap().join("apcore").display()
    );
}

fn load_fixture(name: &str) -> Value {
    let path = find_fixtures_root().join(format!("{}.json", name));
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("Failed to read fixture: {}", path.display()));
    serde_json::from_str(&content).unwrap_or_else(|e| panic!("Invalid JSON in {}: {}", name, e))
}

// ---------------------------------------------------------------------------
// 1. Pattern Matching (A09)
// ---------------------------------------------------------------------------

#[test]
fn conformance_pattern_matching() {
    let fixture = load_fixture("pattern_matching");
    for tc in fixture["test_cases"].as_array().unwrap() {
        let id = tc["id"].as_str().unwrap();
        let pattern = tc["pattern"].as_str().unwrap();
        let value = tc["value"].as_str().unwrap();
        let expected = tc["expected"].as_bool().unwrap();

        assert_eq!(
            match_pattern(pattern, value),
            expected,
            "FAIL [{}]: match_pattern({:?}, {:?}) expected {}",
            id,
            pattern,
            value,
            expected
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Specificity Scoring (A10)
// ---------------------------------------------------------------------------

#[test]
fn conformance_specificity() {
    let fixture = load_fixture("specificity");
    for tc in fixture["test_cases"].as_array().unwrap() {
        let id = tc["id"].as_str().unwrap();
        let pattern = tc["pattern"].as_str().unwrap();
        let expected = tc["expected_score"].as_u64().unwrap() as u32;

        assert_eq!(
            calculate_specificity(pattern),
            expected,
            "FAIL [{}]: calculate_specificity({:?}) expected {}",
            id,
            pattern,
            expected
        );
    }
}

// ---------------------------------------------------------------------------
// 3. Call Chain Safety (A20)
// ---------------------------------------------------------------------------

#[test]
fn conformance_call_chain() {
    let fixture = load_fixture("call_chain");
    for tc in fixture["test_cases"].as_array().unwrap() {
        let id = tc["id"].as_str().unwrap();
        let module_id = tc["module_id"].as_str().unwrap();
        let call_chain: Vec<String> = tc["call_chain"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let max_depth = tc
            .get("max_call_depth")
            .and_then(|v| v.as_u64())
            .unwrap_or(32) as u32;
        let max_repeat = tc
            .get("max_module_repeat")
            .and_then(|v| v.as_u64())
            .unwrap_or(3) as usize;

        // Build a context with the call chain
        let identity = Identity::new(
            "test".to_string(),
            "user".to_string(),
            vec![],
            HashMap::new(),
        );
        let mut ctx: Context<Value> = Context::create(identity, Value::Null, None, None);
        ctx.call_chain = call_chain;

        let result = guard_call_chain_with_repeat(&ctx, module_id, max_depth, max_repeat);

        if let Some(expected_error) = tc.get("expected_error").and_then(|v| v.as_str()) {
            assert!(
                result.is_err(),
                "FAIL [{}]: expected error {} but got Ok",
                id,
                expected_error
            );
            let err = result.unwrap_err();
            let err_lower = format!("{}", err).to_lowercase();
            match expected_error {
                "CALL_DEPTH_EXCEEDED" => assert!(
                    err_lower.contains("depth"),
                    "FAIL [{}]: expected depth error, got: {}",
                    id,
                    err_lower
                ),
                "CIRCULAR_CALL" => assert!(
                    err_lower.contains("circular"),
                    "FAIL [{}]: expected circular error, got: {}",
                    id,
                    err_lower
                ),
                "CALL_FREQUENCY_EXCEEDED" => assert!(
                    err_lower.contains("frequency"),
                    "FAIL [{}]: expected frequency error, got: {}",
                    id,
                    err_lower
                ),
                _ => panic!("Unknown expected_error: {}", expected_error),
            }
        } else {
            assert!(
                result.is_ok(),
                "FAIL [{}]: expected Ok but got Err({})",
                id,
                result.unwrap_err()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 4. ACL Evaluation
// ---------------------------------------------------------------------------

#[test]
fn conformance_acl_evaluation() {
    ACL::init_builtin_handlers();
    let fixture = load_fixture("acl_evaluation");
    for tc in fixture["test_cases"].as_array().unwrap() {
        let id = tc["id"].as_str().unwrap();
        let caller_id_val = &tc["caller_id"];
        let target_id = tc["target_id"].as_str().unwrap();
        let expected = tc["expected"].as_bool().unwrap();
        let default_effect = tc["default_effect"].as_str().unwrap();

        let rules: Vec<ACLRule> = tc["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| ACLRule {
                callers: r["callers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect(),
                targets: r["targets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect(),
                effect: r["effect"].as_str().unwrap().to_string(),
                description: r.get("description").and_then(|v| v.as_str()).map(String::from),
                conditions: r.get("conditions").cloned(),
            })
            .collect();

        let acl = ACL::new(rules, default_effect, None);

        // Build context if needed
        let needs_context = tc.get("caller_identity").is_some()
            || tc.get("call_depth").and_then(|v| v.as_u64()).unwrap_or(0) > 0
            || tc["rules"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r.get("conditions").is_some());

        let ctx: Option<Context<Value>> = if needs_context {
            let identity = if let Some(id_data) = tc.get("caller_identity") {
                Identity::new(
                    caller_id_val
                        .as_str()
                        .unwrap_or("unknown")
                        .to_string(),
                    id_data
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("user")
                        .to_string(),
                    id_data
                        .get("roles")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().map(|v| v.as_str().unwrap().to_string()).collect())
                        .unwrap_or_default(),
                    HashMap::new(),
                )
            } else {
                Identity::new(
                    "anonymous".to_string(),
                    "user".to_string(),
                    vec![],
                    HashMap::new(),
                )
            };

            let mut ctx: Context<Value> =
                Context::create(identity, Value::Null, None, None);

            let call_depth = tc
                .get("call_depth")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            for i in 0..call_depth {
                ctx.call_chain.push(format!("_depth_{}", i));
            }

            Some(ctx)
        } else {
            None
        };

        let caller_id = if caller_id_val.is_null() {
            None
        } else {
            Some(caller_id_val.as_str().unwrap())
        };

        let result = acl
            .check(caller_id, target_id, ctx.as_ref())
            .unwrap_or(false);

        assert_eq!(
            result, expected,
            "FAIL [{}]: ACL check(caller={:?}, target={:?}) returned {}, expected {}",
            id, caller_id, target_id, result, expected
        );
    }
}

// ---------------------------------------------------------------------------
// 5. Context Serialization
// ---------------------------------------------------------------------------

#[test]
fn conformance_context_serialization() {
    let fixture = load_fixture("context_serialization");
    for tc in fixture["test_cases"].as_array().unwrap() {
        let id = tc["id"].as_str().unwrap();

        // Skip sub_cases (handled separately)
        if tc.get("sub_cases").is_some() {
            continue;
        }

        let input = &tc["input"];
        let expected = &tc["expected"];

        if id == "deserialization_round_trip" {
            let ctx: Context<Value> = Context::deserialize(input.clone()).unwrap();
            assert_eq!(ctx.trace_id, expected["trace_id"].as_str().unwrap(), "FAIL [{}]", id);
            assert_eq!(
                ctx.caller_id.as_deref(),
                expected["caller_id"].as_str(),
                "FAIL [{}]",
                id
            );
            if let Some(expected_id) = expected.get("identity_id").and_then(|v| v.as_str()) {
                let identity = ctx.identity.as_ref().unwrap();
                assert_eq!(identity.id(), expected_id, "FAIL [{}]", id);
                assert_eq!(
                    identity.identity_type(),
                    expected["identity_type"].as_str().unwrap(),
                    "FAIL [{}]",
                    id
                );
            }
            continue;
        }

        if id == "unknown_context_version_warns_but_proceeds" {
            let ctx: Context<Value> = Context::deserialize(input.clone()).unwrap();
            assert_eq!(ctx.trace_id, expected["trace_id"].as_str().unwrap(), "FAIL [{}]", id);
            continue;
        }

        if id == "redacted_inputs_serialized" {
            let identity: Option<Identity> = input.get("identity").and_then(|v| {
                if v.is_null() {
                    None
                } else {
                    Some(Identity::new(
                        v["id"].as_str().unwrap().to_string(),
                        v.get("type").and_then(|t| t.as_str()).unwrap_or("user").to_string(),
                        v.get("roles")
                            .and_then(|r| r.as_array())
                            .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
                            .unwrap_or_default(),
                        HashMap::new(),
                    ))
                }
            });

            let mut ctx: Context<Value> = if let Some(id_val) = identity {
                Context::create(id_val, Value::Null, input["caller_id"].as_str().map(String::from), None)
            } else {
                let dummy = Identity::new("anon".into(), "user".into(), vec![], HashMap::new());
                let mut c: Context<Value> = Context::create(dummy, Value::Null, None, None);
                c.identity = None;
                c.caller_id = input["caller_id"].as_str().map(String::from);
                c
            };

            ctx.trace_id = input["trace_id"].as_str().unwrap().to_string();
            ctx.call_chain = input["call_chain"]
                .as_array()
                .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
                .unwrap_or_default();

            if let Some(ri) = input.get("redacted_inputs") {
                if !ri.is_null() {
                    ctx.redacted_inputs = serde_json::from_value(ri.clone()).ok();
                }
            }

            let serialized = ctx.serialize();
            assert_eq!(
                serialized["trace_id"].as_str().unwrap(),
                expected["trace_id"].as_str().unwrap(),
                "FAIL [{}]",
                id
            );
            assert_eq!(
                serialized["redacted_inputs"], expected["redacted_inputs"],
                "FAIL [{}]",
                id
            );
            continue;
        }

        // Standard: build context, serialize, compare
        let identity: Option<Identity> = input.get("identity").and_then(|v| {
            if v.is_null() {
                None
            } else {
                Some(Identity::new(
                    v["id"].as_str().unwrap().to_string(),
                    v.get("type").and_then(|t| t.as_str()).unwrap_or("user").to_string(),
                    v.get("roles")
                        .and_then(|r| r.as_array())
                        .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
                        .unwrap_or_default(),
                    v.get("attrs")
                        .and_then(|a| serde_json::from_value(a.clone()).ok())
                        .unwrap_or_default(),
                ))
            }
        });

        let mut ctx: Context<Value> = if let Some(id_val) = identity {
            Context::create(id_val, Value::Null, input["caller_id"].as_str().map(String::from), None)
        } else {
            let dummy = Identity::new("anon".into(), "user".into(), vec![], HashMap::new());
            let mut c: Context<Value> = Context::create(dummy, Value::Null, None, None);
            c.identity = None;
            c.caller_id = input["caller_id"].as_str().map(String::from);
            c
        };

        ctx.trace_id = input["trace_id"].as_str().unwrap().to_string();
        ctx.call_chain = input["call_chain"]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
            .unwrap_or_default();

        // Set data
        if let Some(data_obj) = input.get("data").and_then(|d| d.as_object()) {
            let mut data = ctx.data.write().unwrap();
            for (k, v) in data_obj {
                data.insert(k.clone(), v.clone());
            }
        }

        let serialized = ctx.serialize();

        assert_eq!(
            serialized["_context_version"],
            expected["_context_version"],
            "FAIL [{}] _context_version",
            id
        );
        assert_eq!(
            serialized["trace_id"], expected["trace_id"],
            "FAIL [{}] trace_id",
            id
        );
        assert_eq!(
            serialized["caller_id"], expected["caller_id"],
            "FAIL [{}] caller_id",
            id
        );
        assert_eq!(
            serialized["call_chain"], expected["call_chain"],
            "FAIL [{}] call_chain",
            id
        );
        assert_eq!(
            serialized["identity"], expected["identity"],
            "FAIL [{}] identity",
            id
        );
        assert_eq!(
            serialized["data"], expected["data"],
            "FAIL [{}] data",
            id
        );
    }
}

#[test]
fn conformance_context_identity_types() {
    let fixture = load_fixture("context_serialization");
    for tc in fixture["test_cases"].as_array().unwrap() {
        if let Some(sub_cases) = tc.get("sub_cases").and_then(|v| v.as_array()) {
            for sub in sub_cases {
                let id_data = &sub["input_identity"];
                let expected_type = sub["expected_type"].as_str().unwrap();

                let identity = Identity::new(
                    id_data["id"].as_str().unwrap().to_string(),
                    id_data["type"].as_str().unwrap().to_string(),
                    id_data["roles"]
                        .as_array()
                        .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
                        .unwrap_or_default(),
                    HashMap::new(),
                );

                let ctx: Context<Value> =
                    Context::create(identity, Value::Null, None, None);
                let serialized = ctx.serialize();

                assert_eq!(
                    serialized["identity"]["type"].as_str().unwrap(),
                    expected_type,
                    "FAIL identity type {}",
                    expected_type
                );

                let restored: Context<Value> = Context::deserialize(serialized).unwrap();
                assert_eq!(
                    restored.identity.as_ref().unwrap().identity_type(),
                    expected_type,
                    "FAIL roundtrip identity type {}",
                    expected_type
                );
            }
        }
    }
}
