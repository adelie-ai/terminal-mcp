#![deny(warnings)]

// MCP server implementation

use crate::error::{McpError, Result};
use crate::operations::audit::AuditLogger;
use crate::tools::ToolRegistry;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// MCP server state
pub struct McpServer {
    /// Tool registry
    tool_registry: Arc<ToolRegistry>,
    /// Optional audit logger.
    audit_logger: Option<Arc<AuditLogger>>,
    /// Initialized flag
    initialized: Arc<RwLock<bool>>,
}

impl McpServer {
    /// Create a new MCP server
    pub fn new() -> Self {
        let audit_logger = load_audit_logger_from_env();

        Self {
            tool_registry: Arc::new(ToolRegistry::new_with_audit(audit_logger.clone())),
            audit_logger,
            initialized: Arc::new(RwLock::new(false)),
        }
    }

    /// Handle initialize request
    pub async fn handle_initialize(
        &self,
        protocol_version: &str,
        _client_capabilities: &Value,
    ) -> Result<Value> {
        self.log_tool_call("initialize", &serde_json::json!({
            "protocolVersion": protocol_version,
        }));

        if protocol_version != "2024-11-05"
            && protocol_version != "2025-06-18"
            && protocol_version != "2025-11-25"
        {
            self.log_tool_result(&format!(
                "initialize error unsupported_protocol={} ",
                protocol_version
            ));
            return Err(McpError::InvalidProtocolVersion(protocol_version.to_string()).into());
        }

        let tools = self.tool_registry.list_tools().await;

        let capabilities = serde_json::json!({
            "protocolVersion": protocol_version,
            "serverInfo": {
                "name": "terminal-mcp",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "capabilities": {
                "tools": {
                    "listChanged": true,
                },
            },
            "tools": tools,
        });

        self.log_tool_result("initialize ok");
        Ok(capabilities)
    }

    /// Handle initialized notification
    pub async fn handle_initialized(&self) -> Result<()> {
        self.log_tool_call("initialized", &Value::Null);
        let mut initialized = self.initialized.write().await;
        *initialized = true;
        self.log_tool_result("initialized ok");
        Ok(())
    }

    /// Handle tool call. Returns (result, tools_changed).
    pub async fn handle_tool_call(
        &self,
        tool_name: &str,
        arguments: &Value,
    ) -> Result<(Value, bool)> {
        self.log_tool_call(tool_name, arguments);
        match self.tool_registry.execute_tool(tool_name, arguments).await {
            Ok((result, tools_changed)) => {
                self.log_tool_result(&tool_result_summary(tool_name, &result, tools_changed));
                Ok((result, tools_changed))
            }
            Err(err) => {
                self.log_tool_result(&format!("{} error {}", tool_name, err));
                Err(err)
            }
        }
    }

    /// Handle shutdown request
    pub async fn handle_shutdown(&self) -> Result<()> {
        self.log_tool_call("shutdown", &Value::Null);
        let mut initialized = self.initialized.write().await;
        *initialized = false;
        self.log_tool_result("shutdown ok");
        Ok(())
    }

    /// List tools in MCP schema format
    pub async fn list_tools(&self) -> Value {
        self.log_tool_call("tools/list", &Value::Null);
        self.tool_registry.list_tools().await
    }

    /// Check if server is initialized
    pub async fn is_initialized(&self) -> bool {
        *self.initialized.read().await
    }
}

impl Default for McpServer {
    fn default() -> Self {
        Self::new()
    }
}

fn load_audit_logger_from_env() -> Option<Arc<AuditLogger>> {
    let Ok(raw) = std::env::var("MCP_TERMINAL_LOG_DIR") else {
        return None;
    };

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let log_dir = PathBuf::from(trimmed);
    match AuditLogger::new(log_dir) {
        Ok(logger) => Some(Arc::new(logger)),
        Err(err) => {
            eprintln!("Failed to initialize audit logger from MCP_TERMINAL_LOG_DIR: {}", err);
            None
        }
    }
}

fn tool_result_summary(tool_name: &str, result: &Value, tools_changed: bool) -> String {
    let value = result
        .get("content")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("value"));

    if let Some(val) = value {
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

impl McpServer {
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
