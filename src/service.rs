#![deny(warnings)]

//! The [`mcp_core::McpService`] implementation for terminal-mcp.
//!
//! mcp-core owns the JSON-RPC protocol, framing, transports, version
//! negotiation, `isError` result shaping, and `tools/list_changed` emission.
//! This service supplies the dynamic tool set and tool execution, mapping the
//! crate's domain errors onto [`mcp_core::CallError`], and threads the optional
//! audit logger (session-level lines here; per-command files inside the
//! registry).

use crate::error::{McpError, TerminalMcpError};
use crate::operations::audit::AuditLogger;
use crate::tools::ToolRegistry;
use mcp_core::{CallError, McpService, ToolDef, ToolReply, async_trait};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

/// terminal-mcp's MCP service: a shared dynamic-script registry plus an
/// optional audit logger.
pub struct TerminalService {
    registry: Arc<ToolRegistry>,
    audit_logger: Option<Arc<AuditLogger>>,
}

impl TerminalService {
    /// Build the service, wiring an audit logger from `MCP_TERMINAL_LOG_DIR`
    /// when that variable points at a usable directory.
    ///
    /// Fallible: if the log dir is set but the logger cannot be created, that
    /// surfaces as an error so the operator notices a misconfigured audit sink.
    pub fn from_env() -> std::io::Result<Self> {
        let audit_logger = load_audit_logger_from_env()?;
        Ok(Self {
            registry: Arc::new(ToolRegistry::new_with_audit(audit_logger.clone())),
            audit_logger,
        })
    }

    fn log_tool_call(&self, tool_name: &str, arguments: &Value) {
        if let Some(logger) = &self.audit_logger {
            logger.log_tool_call(tool_name, arguments);
        }
    }

    fn log_tool_result(&self, summary: &str) {
        if let Some(logger) = &self.audit_logger {
            logger.log_tool_result(summary);
        }
    }
}

#[async_trait]
impl McpService for TerminalService {
    fn tools(&self) -> Vec<ToolDef> {
        // Built-ins plus one script_<name> per stored script. The registry's
        // store is a sync RwLock, so this reads the current set directly.
        self.registry.list_tools()
    }

    async fn call_tool(&self, name: &str, arguments: &Value) -> Result<ToolReply, CallError> {
        self.log_tool_call(name, arguments);

        match self.registry.execute_tool(name, arguments).await {
            Ok(outcome) => {
                let summary = tool_result_summary(name, &outcome.reply, outcome.tools_changed);
                self.log_tool_result(&summary);
                if outcome.tools_changed {
                    Ok(outcome.reply.tools_changed())
                } else {
                    Ok(outcome.reply)
                }
            }
            Err(err) => {
                self.log_tool_result(&format!("{} error {}", name, err));
                Err(map_error(err))
            }
        }
    }
}

/// Map a domain error onto the right [`CallError`] variant. Bad/missing
/// parameters are `-32602` (`InvalidParams`); everything else the model should
/// see and react to (unknown tool, not-found script, execution failure) is a
/// tool-level error surfaced as `isError` content.
fn map_error(err: TerminalMcpError) -> CallError {
    match err {
        // Missing/unparseable parameters → JSON-RPC -32602.
        TerminalMcpError::Mcp(McpError::InvalidToolParameters(msg)) => {
            CallError::invalid_params(msg)
        }
        // A serialize failure building a reply is a genuine internal fault.
        TerminalMcpError::Json(e) => CallError::internal(e.to_string()),
        // Unknown tool, not-found script, execution failures → isError content
        // the model should see and react to.
        other => CallError::tool(other.to_string()),
    }
}

/// Build the audit session-log summary line from a successful reply. Execution
/// replies carry their structured payload in `structuredContent`, so the
/// exit-code/timeout/log-file fields are read from there.
fn tool_result_summary(tool_name: &str, reply: &ToolReply, tools_changed: bool) -> String {
    if let Some(val) = &reply.structured_content {
        let exit_code = val.get("exit_code").and_then(|v| v.as_i64());
        let timed_out = val.get("timed_out").and_then(|v| v.as_bool());
        let log_file = val.get("audit_log_file").and_then(|v| v.as_str());

        if let (Some(code), Some(timeout)) = (exit_code, timed_out) {
            if let Some(file) = log_file {
                return format!(
                    "{} ok exit_code={} timed_out={} tools_changed={} log_file={}",
                    tool_name, code, timeout, tools_changed, file
                );
            }
            return format!(
                "{} ok exit_code={} timed_out={} tools_changed={}",
                tool_name, code, timeout, tools_changed
            );
        }
    }

    format!("{} ok tools_changed={}", tool_name, tools_changed)
}

/// Construct an audit logger from `MCP_TERMINAL_LOG_DIR`. Returns `Ok(None)`
/// when the variable is unset or empty, and an error when the directory is set
/// but the logger cannot be created.
fn load_audit_logger_from_env() -> std::io::Result<Option<Arc<AuditLogger>>> {
    let Ok(raw) = std::env::var("MCP_TERMINAL_LOG_DIR") else {
        return Ok(None);
    };

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let log_dir = PathBuf::from(trimmed);
    let logger = AuditLogger::new(log_dir)?;
    Ok(Some(Arc::new(logger)))
}
