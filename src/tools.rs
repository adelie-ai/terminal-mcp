#![deny(warnings)]

// Tool registry and MCP tool definitions

use crate::error::Result;
use serde_json::Value;

/// Tool registry that manages all available tools
pub struct ToolRegistry;

impl ToolRegistry {
    /// Create a new tool registry
    pub fn new() -> Self {
        Self
    }

    /// Get all tools in MCP format
    pub fn list_tools(&self) -> Value {
        serde_json::json!([
            {
                "name": "terminal_execute",
                "description": "Execute a shell command and return stdout/stderr. Use this to run commands in a terminal. The command is executed directly (not via a shell) unless you use 'sh -c'. Returns the exit code, stdout, stderr, and whether the command timed out.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The command to execute. This is the program name (e.g., 'ls', 'git', 'python3'). Use 'sh' with args ['-c', 'your shell command'] to run shell pipelines or use shell features."
                        },
                        "args": {
                            "type": "array",
                            "items": {
                                "type": "string"
                            },
                            "description": "Arguments to pass to the command. Each argument is a separate string in the array."
                        },
                        "cwd": {
                            "type": "string",
                            "description": "Working directory for the command. Supports ~ expansion. If not specified, uses the server's current working directory. The directory must exist."
                        },
                        "timeout_secs": {
                            "type": "number",
                            "description": "Timeout in seconds. If the command doesn't complete within this time, it will be killed and timed_out will be true. Default: 30 seconds."
                        },
                        "stdin": {
                            "type": "string",
                            "description": "Input to send to the process's stdin. The stdin will be closed after writing this input."
                        }
                    },
                    "required": ["command"]
                }
            }
        ])
    }

    /// Execute a tool by name
    pub async fn execute_tool(&self, name: &str, arguments: &Value) -> Result<Value> {
        let args = arguments.as_object().ok_or_else(|| {
            crate::error::McpError::InvalidToolParameters("Arguments must be an object".to_string())
        })?;

        match name {
            "terminal_execute" => {
                let command =
                    args.get("command")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            crate::error::McpError::InvalidToolParameters(
                                "Missing required parameter: command".to_string(),
                            )
                        })?;

                let cmd_args: Option<Vec<String>> = args.get("args").and_then(|v| {
                    v.as_array().map(|arr| {
                        arr.iter()
                            .filter_map(|item| item.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                });

                let cwd = args.get("cwd").and_then(|v| v.as_str());

                let timeout_secs = args.get("timeout_secs").and_then(|v| v.as_u64());

                let stdin_input = args.get("stdin").and_then(|v| v.as_str());

                let result = crate::operations::execute::execute(
                    command,
                    cmd_args.as_deref(),
                    cwd,
                    timeout_secs,
                    stdin_input,
                )
                .await?;

                Ok(serde_json::json!({
                    "content": [{
                        "type": "json",
                        "value": {
                            "exit_code": result.exit_code,
                            "stdout": result.stdout,
                            "stderr": result.stderr,
                            "timed_out": result.timed_out,
                        }
                    }]
                }))
            }
            _ => Err(crate::error::McpError::ToolNotFound(name.to_string()).into()),
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_tool_missing_command() {
        let registry = ToolRegistry::new();
        let args = serde_json::json!({});
        let res = registry.execute_tool("terminal_execute", &args).await;
        assert!(res.is_err());
        let msg = format!("{}", res.err().unwrap());
        assert!(msg.contains("command"));
    }

    #[tokio::test]
    async fn test_tool_not_found() {
        let registry = ToolRegistry::new();
        let args = serde_json::json!({"command": "echo"});
        let res = registry.execute_tool("nonexistent_tool", &args).await;
        assert!(res.is_err());
        let msg = format!("{}", res.err().unwrap());
        assert!(msg.contains("not found"));
    }
}
