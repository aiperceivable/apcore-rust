//! Consolidated integration-test binary (apcore#dev-ergonomics).
//!
//! Cargo would otherwise compile every top-level tests/*.rs into its OWN
//! binary (each re-monomorphizing apcore + linking 277 deps, ~20s each).
//! With autotests=false in Cargo.toml, this single aggregator is the
//! integration-test binary for all files that do NOT touch process-global
//! state. One codegen, one link, shared monomorphization.
//!
//! EXCLUDED (kept as separate [[test]] binaries in Cargo.toml so each runs
//! in its own process — they register into the process-global Config
//! namespace registry / mutate env, which cross-pollutes under one binary):
//!   config_bus, config_bus_spec, config_discovery, conformance_test
//!
//! To add a new test file: drop tests/<name>.rs and add a line below
//! (unless it touches global Config/env state — then add a [[test]] entry).

#![allow(clippy::pedantic, clippy::all)] // per-module allows live in each file

#[path = "acl_system_spec.rs"]
mod acl_system_spec;
#[path = "apcore_client_spec.rs"]
mod apcore_client_spec;
#[path = "approval_system_spec.rs"]
mod approval_system_spec;
#[path = "async_tasks_spec.rs"]
mod async_tasks_spec;
#[path = "call_chain_guard_spec.rs"]
mod call_chain_guard_spec;
#[path = "cancellation_spec.rs"]
mod cancellation_spec;
#[path = "conformance_env.rs"]
mod conformance_env;
#[path = "conformance_high_drift.rs"]
mod conformance_high_drift;
#[path = "context_object_spec.rs"]
mod context_object_spec;
#[path = "context_serialization_test.rs"]
mod context_serialization_test;
#[path = "core_executor_spec.rs"]
mod core_executor_spec;
#[path = "decorator_bindings_spec.rs"]
mod decorator_bindings_spec;
#[path = "error_formatter_registry.rs"]
mod error_formatter_registry;
#[path = "error_system_spec.rs"]
mod error_system_spec;
#[path = "event_system_spec.rs"]
mod event_system_spec;
#[path = "extension_system_spec.rs"]
mod extension_system_spec;
#[path = "identity_immutable_test.rs"]
mod identity_immutable_test;
#[path = "identity_system_spec.rs"]
mod identity_system_spec;
#[path = "integration_test.rs"]
mod integration_test;
#[path = "middleware_system_spec.rs"]
mod middleware_system_spec;
#[path = "module_interface_spec.rs"]
mod module_interface_spec;
#[path = "multi_module_discovery_spec.rs"]
mod multi_module_discovery_spec;
#[path = "observability_spec.rs"]
mod observability_spec;
#[path = "policy_governance.rs"]
mod policy_governance;
#[path = "registry_system_spec.rs"]
mod registry_system_spec;
#[path = "schema_system_spec.rs"]
mod schema_system_spec;
#[path = "streaming_spec.rs"]
mod streaming_spec;
#[path = "sys_manifest_description.rs"]
mod sys_manifest_description;
#[path = "sys_modules_control.rs"]
mod sys_modules_control;
#[path = "sys_modules_empty_module_id.rs"]
mod sys_modules_empty_module_id;
#[path = "system_modules_spec.rs"]
mod system_modules_spec;
#[path = "test_acl.rs"]
mod test_acl;
#[path = "test_acl_conditions.rs"]
mod test_acl_conditions;
#[path = "test_acl_handler_error_conformance.rs"]
mod test_acl_handler_error_conformance;
#[path = "test_acl_panic_boundary.rs"]
mod test_acl_panic_boundary;
#[path = "test_acl_root_discovery.rs"]
mod test_acl_root_discovery;
#[path = "test_acl_root_discovery_conformance.rs"]
mod test_acl_root_discovery_conformance;
#[path = "test_acl_sync_handler_error.rs"]
mod test_acl_sync_handler_error;
#[path = "test_approval.rs"]
mod test_approval;
#[path = "test_approval_decision_event.rs"]
mod test_approval_decision_event;
#[path = "test_approval_request_live_module_metadata.rs"]
mod test_approval_request_live_module_metadata;
#[path = "test_async_task.rs"]
mod test_async_task;
#[path = "test_async_task_cancellation_conformance.rs"]
mod test_async_task_cancellation_conformance;
#[path = "test_async_task_evolution_conformance.rs"]
mod test_async_task_evolution_conformance;
#[path = "test_async_task_hardening.rs"]
mod test_async_task_hardening;
#[path = "test_bindings.rs"]
mod test_bindings;
#[path = "test_builtin_steps.rs"]
mod test_builtin_steps;
#[path = "test_call_chain_guard.rs"]
mod test_call_chain_guard;
#[path = "test_cancel.rs"]
mod test_cancel;
#[path = "test_chunk_shape.rs"]
mod test_chunk_shape;
#[path = "test_client.rs"]
mod test_client;
#[path = "test_compute_delay_ms.rs"]
mod test_compute_delay_ms;
#[path = "test_config_key_governance_conformance.rs"]
mod test_config_key_governance_conformance;
#[path = "test_config_key_restricted.rs"]
mod test_config_key_restricted;
#[path = "test_config_load_coverage_guard.rs"]
mod test_config_load_coverage_guard;
#[path = "test_config_load_executor_namespace.rs"]
mod test_config_load_executor_namespace;
#[path = "test_config_load_framework_sections.rs"]
mod test_config_load_framework_sections;
#[path = "test_config_load_observability_subkeys.rs"]
mod test_config_load_observability_subkeys;
#[path = "test_config_load_sections.rs"]
mod test_config_load_sections;
#[path = "test_config_reload_from_disk.rs"]
mod test_config_reload_from_disk;
#[path = "test_config_unknown_framework_keys.rs"]
mod test_config_unknown_framework_keys;
#[path = "test_context.rs"]
mod test_context;
#[path = "test_context_key.rs"]
mod test_context_key;
#[path = "test_context_key_promotion.rs"]
mod test_context_key_promotion;
#[path = "test_context_keys.rs"]
mod test_context_keys;
#[path = "test_context_logger_schema.rs"]
mod test_context_logger_schema;
#[path = "test_crate_root_exports.rs"]
mod test_crate_root_exports;
#[path = "test_decorator.rs"]
mod test_decorator;
#[path = "test_error_fingerprinting.rs"]
mod test_error_fingerprinting;
#[path = "test_error_history_heap.rs"]
mod test_error_history_heap;
#[path = "test_error_serialization_conformance.rs"]
mod test_error_serialization_conformance;
#[path = "test_errors.rs"]
mod test_errors;
#[path = "test_event_delivery_conformance.rs"]
mod test_event_delivery_conformance;
#[path = "test_event_delivery_semantics.rs"]
mod test_event_delivery_semantics;
#[path = "test_event_management_hardening_conformance.rs"]
mod test_event_management_hardening_conformance;
#[path = "test_event_naming_canonical.rs"]
mod test_event_naming_canonical;
#[path = "test_event_naming_conformance.rs"]
mod test_event_naming_conformance;
#[path = "test_events.rs"]
mod test_events;
#[path = "test_executor.rs"]
mod test_executor;
#[path = "test_executor_hardening.rs"]
mod test_executor_hardening;
#[path = "test_executor_trace_cancellation_conformance.rs"]
mod test_executor_trace_cancellation_conformance;
#[path = "test_extensions.rs"]
mod test_extensions;
#[path = "test_inject_inbound_flags.rs"]
mod test_inject_inbound_flags;
#[path = "test_inject_malformed_parent_id.rs"]
mod test_inject_malformed_parent_id;
#[path = "test_middleware.rs"]
mod test_middleware;
#[path = "test_middleware_duplicate_detection.rs"]
mod test_middleware_duplicate_detection;
#[path = "test_middleware_hardening_conformance.rs"]
mod test_middleware_hardening_conformance;
#[path = "test_middleware_on_error_single_site.rs"]
mod test_middleware_on_error_single_site;
#[path = "test_module.rs"]
mod test_module;
#[path = "test_multi_module_discovery_conformance.rs"]
mod test_multi_module_discovery_conformance;
#[path = "test_observability.rs"]
mod test_observability;
#[path = "test_observability_hardening_conformance.rs"]
mod test_observability_hardening_conformance;
#[path = "test_openai_strict.rs"]
mod test_openai_strict;
#[path = "test_openai_strict_compat_conformance.rs"]
mod test_openai_strict_compat_conformance;
#[path = "test_overrides_store_conformance.rs"]
mod test_overrides_store_conformance;
#[path = "test_overrides_store_trait.rs"]
mod test_overrides_store_trait;
#[path = "test_pipeline_configuration_error.rs"]
mod test_pipeline_configuration_error;
#[path = "test_pipeline_failfast.rs"]
mod test_pipeline_failfast;
#[path = "test_pipeline_failfast_config_conformance.rs"]
mod test_pipeline_failfast_config_conformance;
#[path = "test_pipeline_hardening_conformance.rs"]
mod test_pipeline_hardening_conformance;
#[path = "test_pipeline_step_middleware.rs"]
mod test_pipeline_step_middleware;
#[path = "test_pipeline_step_middleware_conformance.rs"]
mod test_pipeline_step_middleware_conformance;
#[path = "test_pipeline_tasks.rs"]
mod test_pipeline_tasks;
#[path = "test_pipeline_types.rs"]
mod test_pipeline_types;
#[path = "test_preflight_disclosure_conformance.rs"]
mod test_preflight_disclosure_conformance;
#[path = "test_redaction_config.rs"]
mod test_redaction_config;
#[path = "test_redaction_config_conformance.rs"]
mod test_redaction_config_conformance;
#[path = "test_redaction_config_keys.rs"]
mod test_redaction_config_keys;
#[path = "test_redaction_default_keys.rs"]
mod test_redaction_default_keys;
#[path = "test_register_module_annotations.rs"]
mod test_register_module_annotations;
#[path = "test_registry.rs"]
mod test_registry;
#[path = "test_registry_load_ordering.rs"]
mod test_registry_load_ordering;
#[path = "test_registry_ordering_conformance.rs"]
mod test_registry_ordering_conformance;
#[path = "test_reload_order_registration_paths.rs"]
mod test_reload_order_registration_paths;
#[path = "test_reload_path_filter.rs"]
mod test_reload_path_filter;
#[path = "test_reload_path_filter_conformance.rs"]
mod test_reload_path_filter_conformance;
#[path = "test_retry_config_default.rs"]
mod test_retry_config_default;
#[path = "test_retry_signal.rs"]
mod test_retry_signal;
#[path = "test_schema_export_envelope_conformance.rs"]
mod test_schema_export_envelope_conformance;
#[path = "test_schema_exporter.rs"]
mod test_schema_exporter;
#[path = "test_schema_hardening_conformance.rs"]
mod test_schema_hardening_conformance;
#[path = "test_schema_keyword_parity_conformance.rs"]
mod test_schema_keyword_parity_conformance;
#[path = "test_schema_loader.rs"]
mod test_schema_loader;
#[path = "test_schema_resolver.rs"]
mod test_schema_resolver;
#[path = "test_schema_strict_conversion_conformance.rs"]
mod test_schema_strict_conversion_conformance;
#[path = "test_schema_validator.rs"]
mod test_schema_validator;
#[path = "test_serde_wire_format.rs"]
mod test_serde_wire_format;
#[path = "test_spec_decision_followup.rs"]
mod test_spec_decision_followup;
#[path = "test_storage_backend.rs"]
mod test_storage_backend;
#[path = "test_storage_backend_conformance.rs"]
mod test_storage_backend_conformance;
#[path = "test_stream_chunk_reject.rs"]
mod test_stream_chunk_reject;
#[path = "test_stream_phase1_on_error.rs"]
mod test_stream_phase1_on_error;
#[path = "test_streaming_interface.rs"]
mod test_streaming_interface;
#[path = "test_subscriber_retry_config.rs"]
mod test_subscriber_retry_config;
#[path = "test_suspend_resume.rs"]
mod test_suspend_resume;
#[path = "test_sync_parity_critical.rs"]
mod test_sync_parity_critical;
#[path = "test_sync_parity_warning.rs"]
mod test_sync_parity_warning;
#[path = "test_system_modules_hardening_conformance.rs"]
mod test_system_modules_hardening_conformance;
#[path = "test_trace_context.rs"]
mod test_trace_context;
#[path = "test_trace_context_conformance.rs"]
mod test_trace_context_conformance;
#[path = "test_true_streaming.rs"]
mod test_true_streaming;
#[path = "test_unified_registry.rs"]
mod test_unified_registry;
#[path = "test_usage_collector_trend_period.rs"]
mod test_usage_collector_trend_period;
#[path = "test_usage_exporter.rs"]
mod test_usage_exporter;
#[path = "test_usage_exporter_conformance.rs"]
mod test_usage_exporter_conformance;
#[path = "test_usage_hourly_padding.rs"]
mod test_usage_hourly_padding;
#[path = "test_v0_21_preview_and_ephemeral.rs"]
mod test_v0_21_preview_and_ephemeral;
#[path = "test_webhook_http_retry_contract.rs"]
mod test_webhook_http_retry_contract;
