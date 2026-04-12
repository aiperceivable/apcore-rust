//! End-to-end tests for true incremental streaming on `Executor::stream`.
//!
//! These tests assert that:
//!   1. Chunks arrive at the caller *before* the inner stream finishes
//!      (i.e. no Vec buffering — true streaming).
//!   2. Chunks arrive in order and none are dropped.
//!   3. Modules that do not implement `stream()` return an error chunk.
//!   4. Schema validation failures in Phase 3 are surfaced as the final
//!      `Err` item of the output stream.

use std::time::{Duration, Instant};

use apcore::context::Context;
use apcore::errors::ErrorCode;
use apcore::module::Module;
use apcore::{APCore, ChunkStream, ModuleError};
use async_stream::stream;
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::time::{sleep, timeout};

// ---------------------------------------------------------------------------
// Streaming test modules
// ---------------------------------------------------------------------------

/// Yields 5 chunks, sleeping 100ms between each. Total runtime ~500ms.
/// Used to prove that the first chunk arrives well before the stream ends.
struct SlowStreamingModule;

#[async_trait]
impl Module for SlowStreamingModule {
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn description(&self) -> &str {
        "slow streaming test module"
    }
    async fn execute(&self, _inputs: Value, _ctx: &Context<Value>) -> Result<Value, ModuleError> {
        Ok(json!({}))
    }
    fn stream(&self, _inputs: Value, _ctx: &Context<Value>) -> Option<ChunkStream> {
        Some(Box::pin(stream! {
            for i in 0..5u32 {
                sleep(Duration::from_millis(100)).await;
                yield Ok(json!({ "chunk": i }));
            }
        }))
    }
}

/// Non-streaming module: only implements `execute()`. Used to check that
/// `stream()` on such a module yields the "not supported" error.
struct NonStreamingModule;

#[async_trait]
impl Module for NonStreamingModule {
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn output_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn description(&self) -> &str {
        "non-streaming module"
    }
    async fn execute(&self, _inputs: Value, _ctx: &Context<Value>) -> Result<Value, ModuleError> {
        Ok(json!({"ok": true}))
    }
}

/// Yields chunks whose merged output violates the output schema. Used to
/// verify that Phase 3 validation errors appear as the final `Err` item.
struct BadSchemaStreamingModule;

#[async_trait]
impl Module for BadSchemaStreamingModule {
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn output_schema(&self) -> Value {
        // Require a "result" field of type integer — but the merged chunks
        // below won't contain it, so validation MUST fail in Phase 3.
        json!({
            "type": "object",
            "properties": {
                "result": { "type": "integer" }
            },
            "required": ["result"]
        })
    }
    fn description(&self) -> &str {
        "streaming module with bad schema"
    }
    async fn execute(&self, _inputs: Value, _ctx: &Context<Value>) -> Result<Value, ModuleError> {
        Ok(json!({}))
    }
    fn stream(&self, _inputs: Value, _ctx: &Context<Value>) -> Option<ChunkStream> {
        Some(Box::pin(stream! {
            yield Ok(json!({"partial": "a"}));
            yield Ok(json!({"partial": "b"}));
        }))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn first_chunk_arrives_before_stream_completes() {
    let apcore = APCore::new();
    apcore
        .register("slow.stream", Box::new(SlowStreamingModule))
        .unwrap();

    let start = Instant::now();
    let mut s = apcore
        .executor()
        .stream("slow.stream", json!({}), None, None);

    // The inner module takes ~500ms total, but the first chunk should arrive
    // within roughly 100ms (plus scheduling slack). We allow up to 300ms — far
    // less than the full 500ms, which proves chunks are delivered incrementally.
    let first = timeout(Duration::from_millis(300), s.next())
        .await
        .expect("first chunk should arrive before Vec-collected stream could finish");
    let first_elapsed = start.elapsed();
    let chunk = first
        .expect("stream yielded None before first chunk")
        .expect("first chunk should be Ok");
    assert_eq!(chunk["chunk"], 0);
    assert!(
        first_elapsed < Duration::from_millis(300),
        "first chunk arrived too late: {first_elapsed:?}"
    );
}

#[tokio::test]
async fn all_chunks_arrive_in_order() {
    let apcore = APCore::new();
    apcore
        .register("slow.stream", Box::new(SlowStreamingModule))
        .unwrap();

    let mut s = apcore
        .executor()
        .stream("slow.stream", json!({}), None, None);
    let mut chunks = Vec::new();
    while let Some(item) = s.next().await {
        chunks.push(item.expect("no chunks should be errors"));
    }
    assert_eq!(chunks.len(), 5);
    for (i, c) in chunks.iter().enumerate() {
        assert_eq!(c["chunk"], i as u64);
    }
}

#[tokio::test]
async fn streaming_not_supported_yields_error() {
    let apcore = APCore::new();
    apcore
        .register("plain.mod", Box::new(NonStreamingModule))
        .unwrap();

    let mut s = apcore.executor().stream("plain.mod", json!({}), None, None);
    let first = s
        .next()
        .await
        .expect("stream should yield a single error item");
    let err = first.expect_err("non-streaming modules must surface an error");
    assert_eq!(err.code, ErrorCode::GeneralNotImplemented);
    // No further items should be produced after the error.
    assert!(s.next().await.is_none());
}

#[tokio::test]
async fn phase3_validation_failure_becomes_final_error_item() {
    let apcore = APCore::new();
    apcore
        .register("bad.stream", Box::new(BadSchemaStreamingModule))
        .unwrap();

    let mut s = apcore
        .executor()
        .stream("bad.stream", json!({}), None, None);
    let mut ok_count = 0usize;
    let mut last_err: Option<ModuleError> = None;
    while let Some(item) = s.next().await {
        match item {
            Ok(_) => ok_count += 1,
            Err(e) => {
                last_err = Some(e);
                break;
            }
        }
    }
    assert_eq!(ok_count, 2, "both data chunks should reach the caller");
    let err = last_err.expect("Phase 3 validation must yield a final error item");
    assert_eq!(err.code, ErrorCode::SchemaValidationError);
}
