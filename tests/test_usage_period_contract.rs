//! `period` is a filter, not an echo (PROTOCOL_SPEC 6.7.1.1).
//!
//! Both usage modules used to read `period`, echo it back, and compute every
//! statistic over the full retained history — `get_all_summaries()` for the
//! summary, and `get_module_summary` / `get_p99_latency_ms` /
//! `get_caller_breakdown` / `get_hourly_distribution` for the detail module,
//! none of which took a period. Silent by construction: the response names the
//! window it did not apply.

use std::collections::HashMap;
use std::sync::Arc;

use apcore::context::{Context, Identity};
use apcore::module::{Module, ModuleAnnotations};
use apcore::registry::registry::{ModuleDescriptor, Registry};
use apcore::{UsageCollector, UsageModule, UsageSummaryModule};
use chrono::{Duration, Utc};
use serde_json::json;

fn dummy_ctx() -> Context<serde_json::Value> {
    Context::<serde_json::Value>::new(Identity::new(
        "@test".to_string(),
        "test".to_string(),
        vec![],
        HashMap::default(),
    ))
}

fn register_dummy(registry: &Arc<Registry>, id: &str) {
    struct DummyModule;
    #[async_trait::async_trait]
    impl Module for DummyModule {
        fn description(&self) -> &'static str {
            "dummy"
        }
        fn input_schema(&self) -> serde_json::Value {
            json!({})
        }
        fn output_schema(&self) -> serde_json::Value {
            json!({})
        }
        async fn execute(
            &self,
            _inputs: serde_json::Value,
            _ctx: &Context<serde_json::Value>,
        ) -> Result<serde_json::Value, apcore::errors::ModuleError> {
            Ok(json!({}))
        }
    }

    let descriptor = ModuleDescriptor {
        module_id: id.to_string(),
        name: None,
        description: String::new(),
        documentation: None,
        input_schema: json!({}),
        output_schema: json!({}),
        version: "1.0.0".to_string(),
        tags: vec![],
        annotations: Some(ModuleAnnotations::default()),
        examples: vec![],
        metadata: HashMap::new(),
        display: None,
        sunset_date: None,
        dependencies: vec![],
        enabled: true,
    };
    registry
        .register_internal(id, Box::new(DummyModule), descriptor)
        .expect("register_internal should succeed");
}

/// One call inside the 24h window, one well outside it.
fn collector_with_split_history(module_id: &str) -> UsageCollector {
    let collector = UsageCollector::new();
    let now = Utc::now();
    collector.record_at(
        module_id,
        Some("caller-in"),
        10.0,
        true,
        now - Duration::hours(2),
    );
    collector.record_at(
        module_id,
        Some("caller-out"),
        500.0,
        false,
        now - Duration::days(6),
    );
    collector
}

#[tokio::test]
async fn summary_totals_are_filtered_by_period() {
    let collector = collector_with_split_history("math.add");
    let module = UsageSummaryModule::new(collector);

    let result = module
        .execute(json!({"period": "24h"}), &dummy_ctx())
        .await
        .expect("summary should succeed");

    assert_eq!(result["period"], "24h");
    assert_eq!(
        result["total_calls"].as_u64(),
        Some(1),
        "the 6-day-old record is outside a 24h window; full history would answer 2"
    );
    assert_eq!(result["total_errors"].as_u64(), Some(0));
}

#[tokio::test]
async fn summary_widens_with_the_period() {
    // The control case. Without it an implementation that returns nothing at
    // all passes the filtering test for the wrong reason.
    let collector = collector_with_split_history("math.add");
    let module = UsageSummaryModule::new(collector);

    let result = module
        .execute(json!({"period": "30d"}), &dummy_ctx())
        .await
        .expect("summary should succeed");

    assert_eq!(result["total_calls"].as_u64(), Some(2));
    assert_eq!(result["total_errors"].as_u64(), Some(1));
}

#[tokio::test]
async fn every_detail_statistic_is_filtered_by_period() {
    let registry = Arc::new(Registry::new());
    register_dummy(&registry, "math.add");
    let collector = collector_with_split_history("math.add");
    let module = UsageModule::new(Arc::clone(&registry), collector);

    let result = module
        .execute(
            json!({"module_id": "math.add", "period": "24h"}),
            &dummy_ctx(),
        )
        .await
        .expect("detail should succeed");

    assert_eq!(result["call_count"].as_u64(), Some(1));
    assert_eq!(result["error_count"].as_u64(), Some(0));
    assert_eq!(result["avg_latency_ms"].as_f64(), Some(10.0));
    assert_eq!(
        result["p99_latency_ms"].as_f64(),
        Some(10.0),
        "the out-of-window 500ms sample must not reach p99"
    );

    let callers = result["callers"].as_array().expect("callers array");
    assert_eq!(callers.len(), 1, "only the in-window caller may appear");
    assert_eq!(callers[0]["caller_id"], "caller-in");

    let hourly_total: u64 = result["hourly_distribution"]
        .as_array()
        .expect("hourly array")
        .iter()
        .filter_map(|h| h["call_count"].as_u64())
        .sum();
    assert_eq!(
        hourly_total, 1,
        "hourly buckets must be period-filtered too"
    );
}

#[tokio::test]
async fn detail_widens_with_the_period() {
    let registry = Arc::new(Registry::new());
    register_dummy(&registry, "math.add");
    let collector = collector_with_split_history("math.add");
    let module = UsageModule::new(Arc::clone(&registry), collector);

    let result = module
        .execute(
            json!({"module_id": "math.add", "period": "30d"}),
            &dummy_ctx(),
        )
        .await
        .expect("detail should succeed");

    assert_eq!(result["call_count"].as_u64(), Some(2));
    assert_eq!(result["error_count"].as_u64(), Some(1));
    assert_eq!(
        result["p99_latency_ms"].as_f64(),
        Some(500.0),
        "with the wider window the 500ms sample IS the p99"
    );
}

#[tokio::test]
async fn unattributed_caller_is_the_literal_unknown() {
    // PROTOCOL_SPEC 6.7.1.4 — not null, not omitted, and not the ACL token
    // `@external`.
    let registry = Arc::new(Registry::new());
    register_dummy(&registry, "math.add");
    let collector = UsageCollector::new();
    collector.record("math.add", None, 5.0, true);
    let module = UsageModule::new(Arc::clone(&registry), collector);

    let result = module
        .execute(
            json!({"module_id": "math.add", "period": "24h"}),
            &dummy_ctx(),
        )
        .await
        .expect("detail should succeed");

    let callers = result["callers"].as_array().expect("callers array");
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0]["caller_id"], "unknown");
}

#[tokio::test]
async fn output_schema_declares_a_field_contract() {
    // PROTOCOL_SPEC 6.7.1.6. Both modules returned a bare {"type":"object"},
    // which satisfies 6.7's "equivalent output schemas" only in the sense that
    // any two such declarations are equivalent to each other.
    let registry = Arc::new(Registry::new());
    let summary = UsageSummaryModule::new(UsageCollector::new());
    let detail = UsageModule::new(registry, UsageCollector::new());

    for (label, schema) in [
        ("summary", summary.output_schema()),
        ("detail", detail.output_schema()),
    ] {
        assert!(
            schema.get("properties").is_some(),
            "{label} output_schema must declare properties"
        );
        assert!(
            schema.get("required").is_some(),
            "{label} output_schema must declare required"
        );
    }

    let detail_schema =
        UsageModule::new(Arc::new(Registry::new()), UsageCollector::new()).output_schema();
    let hourly = &detail_schema["properties"]["hourly_distribution"];
    assert_eq!(hourly["minItems"].as_u64(), Some(24));
    assert_eq!(hourly["maxItems"].as_u64(), Some(24));
    assert_eq!(
        hourly["items"]["properties"]["hour"]["pattern"],
        "^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}$"
    );
}

#[tokio::test]
async fn period_pattern_is_declared_in_the_input_schema() {
    // PROTOCOL_SPEC 6.7.1.1 — so a malformed value is rejected at input
    // validation with SCHEMA_VALIDATION_ERROR, uniformly across SDKs.
    let summary = UsageSummaryModule::new(UsageCollector::new());
    let detail = UsageModule::new(Arc::new(Registry::new()), UsageCollector::new());

    for (label, schema) in [
        ("summary", summary.input_schema()),
        ("detail", detail.input_schema()),
    ] {
        assert_eq!(
            schema["properties"]["period"]["pattern"], "^[1-9][0-9]*[hd]$",
            "{label} must declare the period grammar"
        );
    }
}
