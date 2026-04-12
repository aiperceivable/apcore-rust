// APCore Protocol — Context-aware logging
// Spec reference: Structured logging with execution context

use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::io::Write;

use parking_lot::Mutex;

use crate::context::Context;
use crate::errors::ModuleError;
use crate::middleware::base::Middleware;

/// Redacted placeholder for sensitive values.
pub const REDACTED: &str = "***REDACTED***";

/// Log level numeric values matching Python reference.
fn level_value(level: &str) -> u32 {
    match level.to_lowercase().as_str() {
        "trace" => 0,
        "debug" => 10,
        "warn" | "warning" => 30,
        "error" => 40,
        "fatal" => 50,
        _ => 20, // info and unknown levels default to 20
    }
}

/// Output format for log records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Json,
    Text,
}

/// Logger that injects execution context into log records.
#[derive(Debug)]
pub struct ContextLogger {
    pub name: String,
    pub level: String,
    pub format: LogFormat,
    pub trace_id: Option<String>,
    pub module_id: Option<String>,
    pub caller_id: Option<String>,
}

impl ContextLogger {
    /// Create a new context logger with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            level: "info".to_string(),
            format: LogFormat::Json,
            trace_id: None,
            module_id: None,
            caller_id: None,
        }
    }

    /// Create a logger with context derived from execution context.
    pub fn from_context(ctx: &Context<serde_json::Value>, name: impl Into<String>) -> Self {
        let module_id = ctx.call_chain.last().cloned();
        let caller_id = ctx.caller_id.clone();
        Self {
            name: name.into(),
            level: "info".to_string(),
            format: LogFormat::Json,
            trace_id: Some(ctx.trace_id.clone()),
            module_id,
            caller_id,
        }
    }

    /// Set the minimum log level.
    pub fn set_level(&mut self, level: impl Into<String>) {
        self.level = level.into();
    }

    /// Set the output format.
    pub fn set_format(&mut self, format: LogFormat) {
        self.format = format;
    }

    /// Emit a log record if level meets threshold.
    pub fn emit(
        &self,
        level_name: &str,
        message: &str,
        extra: Option<&HashMap<String, serde_json::Value>>,
    ) {
        let threshold = level_value(&self.level);
        let msg_level = level_value(level_name);
        if msg_level < threshold {
            return;
        }

        match self.format {
            LogFormat::Json => {
                let mut record = serde_json::Map::new();
                record.insert(
                    "timestamp".to_string(),
                    serde_json::Value::String(Utc::now().to_rfc3339()),
                );
                record.insert(
                    "level".to_string(),
                    serde_json::Value::String(level_name.to_uppercase()),
                );
                record.insert(
                    "logger".to_string(),
                    serde_json::Value::String(self.name.clone()),
                );
                record.insert(
                    "message".to_string(),
                    serde_json::Value::String(message.to_string()),
                );
                if let Some(ref trace_id) = self.trace_id {
                    record.insert(
                        "trace_id".to_string(),
                        serde_json::Value::String(trace_id.clone()),
                    );
                }
                if let Some(ref module_id) = self.module_id {
                    record.insert(
                        "module_id".to_string(),
                        serde_json::Value::String(module_id.clone()),
                    );
                }
                if let Some(ref caller_id) = self.caller_id {
                    record.insert(
                        "caller_id".to_string(),
                        serde_json::Value::String(caller_id.clone()),
                    );
                }
                if let Some(extra_map) = extra {
                    for (k, v) in extra_map {
                        if k.starts_with("_secret_") {
                            record
                                .insert(k.clone(), serde_json::Value::String(REDACTED.to_string()));
                        } else {
                            record.insert(k.clone(), v.clone());
                        }
                    }
                }
                let json_str =
                    serde_json::to_string(&serde_json::Value::Object(record)).unwrap_or_default();
                let _ = writeln!(std::io::stderr(), "{json_str}");
            }
            LogFormat::Text => {
                let ts = Utc::now().to_rfc3339();
                let ctx_str = match (&self.trace_id, &self.module_id) {
                    (Some(tid), Some(mid)) => format!(" [trace={tid} module={mid}]"),
                    (Some(tid), None) => format!(" [trace={tid}]"),
                    (None, Some(mid)) => format!(" [module={mid}]"),
                    (None, None) => String::new(),
                };
                let _ = writeln!(
                    std::io::stderr(),
                    "{} {} {}{} {}",
                    ts,
                    level_name.to_uppercase(),
                    self.name,
                    ctx_str,
                    message
                );
            }
        }
    }

    /// Log a trace message.
    pub fn trace(&self, message: &str) {
        self.emit("trace", message, None);
    }

    /// Log a debug message.
    pub fn debug(&self, message: &str) {
        self.emit("debug", message, None);
    }

    /// Log an info message.
    pub fn info(&self, message: &str) {
        self.emit("info", message, None);
    }

    /// Log a warning message.
    pub fn warn(&self, message: &str) {
        self.emit("warn", message, None);
    }

    /// Log a warning message (alias).
    pub fn warning(&self, message: &str) {
        self.emit("warn", message, None);
    }

    /// Log an error message.
    pub fn error(&self, message: &str) {
        self.emit("error", message, None);
    }

    /// Log a fatal message.
    pub fn fatal(&self, message: &str) {
        self.emit("fatal", message, None);
    }
}

/// Middleware that logs before/after execution.
///
/// WARNING: The internal start-time stack is not safe for concurrent use on
/// the same middleware instance. Use separate instances per concurrent pipeline.
#[derive(Debug)]
pub struct ObsLoggingMiddleware {
    logger: ContextLogger,
    log_inputs: bool,
    log_outputs: bool,
    starts: Mutex<HashMap<String, std::time::Instant>>,
}

impl ObsLoggingMiddleware {
    /// Create a new logging middleware.
    pub fn new(logger: ContextLogger) -> Self {
        Self {
            logger,
            log_inputs: true,
            log_outputs: true,
            starts: Mutex::new(HashMap::new()),
        }
    }

    /// Create with explicit input/output logging flags.
    pub fn with_options(logger: ContextLogger, log_inputs: bool, log_outputs: bool) -> Self {
        Self {
            logger,
            log_inputs,
            log_outputs,
            starts: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl Middleware for ObsLoggingMiddleware {
    fn name(&self) -> &'static str {
        "logging"
    }

    async fn before(
        &self,
        module_id: &str,
        inputs: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        // Store start time keyed by trace_id for concurrency safety
        {
            let mut starts = self.starts.lock();
            starts.insert(ctx.trace_id.clone(), std::time::Instant::now());
        }

        let mut extra = HashMap::new();
        extra.insert(
            "module".to_string(),
            serde_json::Value::String(module_id.to_string()),
        );
        extra.insert(
            "trace_id".to_string(),
            serde_json::Value::String(ctx.trace_id.clone()),
        );
        if self.log_inputs {
            extra.insert("input".to_string(), inputs.clone());
        }
        self.logger
            .emit("info", &format!("Starting {module_id}"), Some(&extra));

        Ok(None)
    }

    async fn after(
        &self,
        module_id: &str,
        _inputs: serde_json::Value,
        _output: serde_json::Value,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        // Remove start time by trace_id and compute duration
        let duration_ms = {
            let mut starts = self.starts.lock();
            starts
                .remove(&ctx.trace_id)
                .map_or(0.0, |s| s.elapsed().as_secs_f64() * 1000.0)
        };

        let mut extra = HashMap::new();
        extra.insert(
            "module".to_string(),
            serde_json::Value::String(module_id.to_string()),
        );
        extra.insert(
            "trace_id".to_string(),
            serde_json::Value::String(ctx.trace_id.clone()),
        );
        extra.insert("duration_ms".to_string(), serde_json::json!(duration_ms));
        if self.log_outputs {
            extra.insert("output".to_string(), _output.clone());
        }
        self.logger.emit(
            "info",
            &format!("Completed {module_id} in {duration_ms:.2}ms"),
            Some(&extra),
        );

        Ok(None)
    }

    async fn on_error(
        &self,
        module_id: &str,
        _inputs: serde_json::Value,
        error: &ModuleError,
        ctx: &Context<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, ModuleError> {
        // Remove start time by trace_id and compute duration
        let duration_ms = {
            let mut starts = self.starts.lock();
            starts
                .remove(&ctx.trace_id)
                .map_or(0.0, |s| s.elapsed().as_secs_f64() * 1000.0)
        };

        let mut extra = HashMap::new();
        extra.insert(
            "module".to_string(),
            serde_json::Value::String(module_id.to_string()),
        );
        extra.insert(
            "trace_id".to_string(),
            serde_json::Value::String(ctx.trace_id.clone()),
        );
        extra.insert("duration_ms".to_string(), serde_json::json!(duration_ms));
        extra.insert(
            "error".to_string(),
            serde_json::Value::String(error.message.clone()),
        );
        extra.insert(
            "error_code".to_string(),
            serde_json::Value::String(format!("{:?}", error.code)),
        );
        self.logger.emit(
            "error",
            &format!(
                "Error in {} after {:.2}ms: {}",
                module_id, duration_ms, error.message
            ),
            Some(&extra),
        );

        Ok(None)
    }
}
