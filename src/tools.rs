#![deny(warnings)]

// Tool registry and MCP tool definitions.
//
// The registry owns the dynamic script store and the optional audit logger.
// It produces `mcp_core::ToolReply` values directly; the JSON-RPC protocol,
// framing, transports, and `tools/list_changed` emission are owned by mcp-core
// (see `src/service.rs`, which adapts this registry to `mcp_core::McpService`).

use crate::error::{McpError, Result, ScriptError};
use crate::operations::audit::AuditLogger;
use crate::operations::execute::ExecuteResult;
use mcp_core::{ToolDef, ToolReply};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// A stored script that becomes a dynamic MCP tool.
#[derive(Clone, Debug)]
struct StoredScript {
    name: String,
    description: String,
    script: String,
    parameters: Vec<ScriptParameter>,
}

/// A parameter for a stored script, exposed as an env var.
#[derive(Clone, Debug)]
struct ScriptParameter {
    name: String,
    description: String,
    required: bool,
}

/// Built-in tool names that cannot be used as script names.
const BUILTIN_TOOLS: &[&str] = &[
    "terminal_execute",
    "terminal_store_script",
    "terminal_remove_script",
    "terminal_list_scripts",
];

/// The outcome of a tool invocation: the reply plus whether the server's tool
/// set changed (so the service can emit `notifications/tools/list_changed`).
pub struct ToolOutcome {
    /// The reply to return to the client.
    pub reply: ToolReply,
    /// Whether this call mutated the stored-script set.
    pub tools_changed: bool,
}

/// Registry for built-in and dynamic MCP tools.
pub struct ToolRegistry {
    scripts: Arc<RwLock<HashMap<String, StoredScript>>>,
    audit_logger: Option<Arc<AuditLogger>>,
}

impl ToolRegistry {
    /// Create a new registry with audit logging disabled.
    pub fn new() -> Self {
        Self::new_with_audit(None)
    }

    /// Create a new registry with optional audit logging.
    pub fn new_with_audit(audit_logger: Option<Arc<AuditLogger>>) -> Self {
        Self {
            scripts: Arc::new(RwLock::new(HashMap::new())),
            audit_logger,
        }
    }

    /// List available tools: the four built-ins plus one `script_<name>` tool
    /// per stored script (read from the current store).
    ///
    /// Synchronous: the store is a `std::sync::RwLock` whose critical sections
    /// never span an `.await`, so `McpService::tools()` (a sync method) can read
    /// it directly without bouncing through the async runtime.
    pub fn list_tools(&self) -> Vec<ToolDef> {
        let mut tools = vec![
            terminal_execute_tool(),
            terminal_store_script_tool(),
            terminal_remove_script_tool(),
            terminal_list_scripts_tool(),
        ];

        let scripts = self.scripts.read().expect("script store lock poisoned");
        for stored in scripts.values() {
            tools.push(dynamic_script_tool(stored));
        }

        tools
    }

    /// Execute a tool by name and return its [`ToolOutcome`].
    pub async fn execute_tool(&self, name: &str, arguments: &Value) -> Result<ToolOutcome> {
        let args = arguments.as_object().ok_or_else(|| {
            McpError::InvalidToolParameters("Arguments must be an object".to_string())
        })?;

        match name {
            "terminal_execute" => {
                let reply = self.exec_terminal_execute(args).await?;
                Ok(ToolOutcome {
                    reply,
                    tools_changed: false,
                })
            }
            "terminal_store_script" => {
                let reply = self.exec_store_script(args).await?;
                Ok(ToolOutcome {
                    reply,
                    tools_changed: true,
                })
            }
            "terminal_remove_script" => {
                let reply = self.exec_remove_script(args).await?;
                Ok(ToolOutcome {
                    reply,
                    tools_changed: true,
                })
            }
            "terminal_list_scripts" => {
                let reply = self.exec_list_scripts().await?;
                Ok(ToolOutcome {
                    reply,
                    tools_changed: false,
                })
            }
            _ => {
                if let Some(script_name) = name.strip_prefix("script_") {
                    let reply = self.exec_dynamic_script(script_name, args).await?;
                    Ok(ToolOutcome {
                        reply,
                        tools_changed: false,
                    })
                } else {
                    Err(McpError::ToolNotFound(name.to_string()).into())
                }
            }
        }
    }

    // `skip_all` on every handler below: `args` (and `script_name`, where a
    // handler takes one) carry the caller's command line, script body, stdin,
    // or working directory, none of which may reach a span field (mcp-core#40).
    // The tool name is already on mcp-core's own `mcp.tools.call` span.
    #[tracing::instrument(skip_all)]
    async fn exec_terminal_execute(
        &self,
        args: &serde_json::Map<String, Value>,
    ) -> Result<ToolReply> {
        let script = args.get("script").and_then(|v| v.as_str());
        let stdin_input = args.get("stdin").and_then(|v| v.as_str());

        if script.is_some() && stdin_input.is_some() {
            return Err(McpError::InvalidToolParameters(
                "Cannot provide both 'script' and 'stdin' parameters".to_string(),
            )
            .into());
        }

        let cwd = args.get("cwd").and_then(|v| v.as_str());
        let timeout_secs = args.get("timeout_secs").and_then(|v| v.as_u64());
        let inactivity_timeout_secs = args.get("inactivity_timeout_secs").and_then(|v| v.as_u64());
        let max_lines = args
            .get("max_lines")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let detach = args
            .get("detach")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let (result, command_desc) = if let Some(script_body) = script {
            let cmd_args: Option<Vec<String>> = args.get("args").and_then(|v| {
                v.as_array().map(|arr| {
                    arr.iter()
                        .filter_map(|item| item.as_str().map(|s| s.to_string()))
                        .collect()
                })
            });

            let result = crate::operations::execute::execute_script(
                script_body,
                cmd_args.as_deref(),
                cwd,
                timeout_secs,
                inactivity_timeout_secs,
                max_lines,
                None,
                detach,
            )
            .await?;

            let command_desc = match cmd_args {
                Some(values) if !values.is_empty() => {
                    format!("script args=[{}]", values.join(" "))
                }
                _ => "script".to_string(),
            };

            (result, command_desc)
        } else {
            let command = args
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    McpError::InvalidToolParameters(
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

            let result = crate::operations::execute::execute(
                command,
                cmd_args.as_deref(),
                cwd,
                timeout_secs,
                inactivity_timeout_secs,
                stdin_input,
                max_lines,
                detach,
            )
            .await?;

            let command_desc = match cmd_args {
                Some(values) if !values.is_empty() => {
                    format!("{} {}", command, values.join(" "))
                }
                _ => command.to_string(),
            };

            (result, command_desc)
        };

        let audit_log_file = self
            .audit_logger
            .as_ref()
            .map(|logger| logger.log_command(&command_desc, cwd, &result));

        execution_result_reply(&result, audit_log_file.as_deref())
    }

    #[tracing::instrument(skip_all)]
    async fn exec_store_script(&self, args: &serde_json::Map<String, Value>) -> Result<ToolReply> {
        let name = args.get("name").and_then(|v| v.as_str()).ok_or_else(|| {
            McpError::InvalidToolParameters("Missing required parameter: name".to_string())
        })?;

        validate_script_name(name)?;

        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::InvalidToolParameters(
                    "Missing required parameter: description".to_string(),
                )
            })?;

        let script_body = args.get("script").and_then(|v| v.as_str()).ok_or_else(|| {
            McpError::InvalidToolParameters("Missing required parameter: script".to_string())
        })?;

        let parameters = parse_script_parameters(args.get("parameters"))?;

        let stored = StoredScript {
            name: name.to_string(),
            description: description.to_string(),
            script: script_body.to_string(),
            parameters,
        };

        let overwritten = {
            let mut scripts = self.scripts.write().expect("script store lock poisoned");
            scripts.insert(name.to_string(), stored).is_some()
        };

        let message = if overwritten {
            format!(
                "Script '{}' updated. Available as tool 'script_{}'.",
                name, name
            )
        } else {
            format!(
                "Script '{}' stored. Available as tool 'script_{}'.",
                name, name
            )
        };
        Ok(ToolReply::text(message))
    }

    #[tracing::instrument(skip_all)]
    async fn exec_remove_script(&self, args: &serde_json::Map<String, Value>) -> Result<ToolReply> {
        let name = args.get("name").and_then(|v| v.as_str()).ok_or_else(|| {
            McpError::InvalidToolParameters("Missing required parameter: name".to_string())
        })?;

        let removed = {
            let mut scripts = self.scripts.write().expect("script store lock poisoned");
            scripts.remove(name).is_some()
        };
        if removed {
            Ok(ToolReply::text(format!("Script '{}' removed.", name)))
        } else {
            Err(ScriptError::NotFound(name.to_string()).into())
        }
    }

    #[tracing::instrument(skip_all)]
    async fn exec_list_scripts(&self) -> Result<ToolReply> {
        let list: Vec<Value> = {
            let scripts = self.scripts.read().expect("script store lock poisoned");
            scripts
                .values()
                .map(|s| {
                    serde_json::json!({
                        "name": s.name,
                        "description": s.description,
                        "parameter_count": s.parameters.len(),
                        "script_preview": script_preview(&s.script),
                    })
                })
                .collect()
        };

        Ok(ToolReply::json(&Value::Array(list))?)
    }

    #[tracing::instrument(skip_all)]
    async fn exec_dynamic_script(
        &self,
        script_name: &str,
        args: &serde_json::Map<String, Value>,
    ) -> Result<ToolReply> {
        let stored = {
            let scripts = self.scripts.read().expect("script store lock poisoned");
            scripts
                .get(script_name)
                .ok_or_else(|| ScriptError::NotFound(script_name.to_string()))?
                .clone()
        };

        // Build env vars from parameters
        let mut env_vars = HashMap::new();
        for param in &stored.parameters {
            if let Some(val) = args.get(&param.name).and_then(|v| v.as_str()) {
                env_vars.insert(param.name.clone(), val.to_string());
            } else if param.required {
                return Err(McpError::InvalidToolParameters(format!(
                    "Missing required parameter: {}",
                    param.name
                ))
                .into());
            }
        }

        let cwd = args.get("cwd").and_then(|v| v.as_str());
        let timeout_secs = args.get("timeout_secs").and_then(|v| v.as_u64());
        let max_lines = args
            .get("max_lines")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let env_ref = if env_vars.is_empty() {
            None
        } else {
            Some(&env_vars)
        };

        let result = crate::operations::execute::execute_script(
            &stored.script,
            None,
            cwd,
            timeout_secs,
            None, // dynamic scripts don't expose inactivity timeout yet
            max_lines,
            env_ref,
            false,
        )
        .await?;

        let command_desc = format!("script_{}", stored.name);
        let audit_log_file = self
            .audit_logger
            .as_ref()
            .map(|logger| logger.log_command(&command_desc, cwd, &result));

        execution_result_reply(&result, audit_log_file.as_deref())
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a short, single-line preview of a stored script body for listings.
/// Collapses internal whitespace and truncates to ~80 chars.
fn script_preview(script: &str) -> String {
    const MAX: usize = 80;
    let collapsed = script.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > MAX {
        let truncated: String = collapsed.chars().take(MAX).collect();
        format!("{}…", truncated)
    } else {
        collapsed
    }
}

/// Build the JSON execution-result reply. The payload is carried both as
/// pretty-printed text and as `structuredContent` (via [`ToolReply::json`]).
fn execution_result_reply(
    result: &ExecuteResult,
    audit_log_file: Option<&str>,
) -> Result<ToolReply> {
    let mut value = serde_json::json!({
        "exit_code": result.exit_code,
        "stdout": result.stdout,
        "stderr": result.stderr,
        "timed_out": result.timed_out,
        "stdout_truncated": result.stdout_truncated,
        "stderr_truncated": result.stderr_truncated,
    });

    if let Some(pid) = result.detached_pid {
        value["detached_pid"] = Value::from(pid);
    }

    if let Some(filename) = audit_log_file {
        value["audit_log_file"] = Value::String(filename.to_string());
    }

    Ok(ToolReply::json(&value)?)
}

fn validate_script_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(ScriptError::InvalidName("Name cannot be empty".to_string()).into());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(ScriptError::InvalidName(format!(
            "Name must be alphanumeric/underscore only, got: '{}'",
            name
        ))
        .into());
    }
    let tool_name = format!("script_{}", name);
    if BUILTIN_TOOLS.contains(&tool_name.as_str()) || BUILTIN_TOOLS.contains(&name) {
        return Err(ScriptError::InvalidName(format!(
            "Name '{}' conflicts with a built-in tool",
            name
        ))
        .into());
    }
    Ok(())
}

/// Environment-variable names that must never be set from script parameters
/// because they alter the dynamic linker / shell startup and enable code
/// injection into the spawned process.
const FORBIDDEN_ENV_NAMES: &[&str] =
    &["LD_PRELOAD", "LD_LIBRARY_PATH", "BASH_ENV", "IFS", "CDPATH"];

/// Validate a script parameter name before it is used as an environment
/// variable name. Names must match `[A-Za-z_][A-Za-z0-9_]*` and must not be one
/// of the dangerous, injection-enabling variables.
fn validate_param_name(name: &str) -> Result<()> {
    let valid = {
        let mut chars = name.chars();
        match chars.next() {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
            }
            _ => false,
        }
    };
    if !valid {
        return Err(McpError::InvalidToolParameters(format!(
            "Invalid parameter name '{}': must match [A-Za-z_][A-Za-z0-9_]*",
            name
        ))
        .into());
    }
    if FORBIDDEN_ENV_NAMES.contains(&name) {
        return Err(McpError::InvalidToolParameters(format!(
            "Parameter name '{}' is not allowed (injection-enabling environment variable)",
            name
        ))
        .into());
    }
    Ok(())
}

fn parse_script_parameters(val: Option<&Value>) -> Result<Vec<ScriptParameter>> {
    let Some(arr) = val.and_then(|v| v.as_array()) else {
        return Ok(vec![]);
    };

    let mut params = Vec::new();
    for item in arr {
        let name = item.get("name").and_then(|v| v.as_str()).ok_or_else(|| {
            McpError::InvalidToolParameters("Each parameter must have a 'name' string".to_string())
        })?;
        validate_param_name(name)?;
        let description = item
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let required = item
            .get("required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        params.push(ScriptParameter {
            name: name.to_string(),
            description: description.to_string(),
            required,
        });
    }
    Ok(params)
}

fn terminal_execute_tool() -> ToolDef {
    ToolDef::new(
        "terminal_execute",
        "Execute a shell command or script and return stdout/stderr. Use 'command' for direct execution or 'script' for multi-line shell scripts. Returns exit code, stdout, stderr, and timeout status.\n\nExecution modes:\n- Bounded (default): the command runs with a timeout and its output is captured. If the timeout fires, the entire process group is terminated, so children are killed too.\n- Detach (detach=true): the command is launched fire-and-forget in a new session with NO timeout and NO captured output; the call returns immediately with 'detached_pid'. Use this only for processes meant to outlive the call (e.g. opening an app, starting a long background task).\n\nSECURITY: This tool runs arbitrary shell commands with the privileges of the user running the server. Only expose it to trusted clients. By default spawned commands do NOT inherit the server's environment (secrets are scrubbed); set MCP_TERMINAL_INHERIT_ENV=1 to opt back in. An optional allowlist can be configured via MCP_TERMINAL_ALLOWED_COMMANDS (comma-separated; unrestricted by default).",
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "A shell command line, run via `sh -c` (e.g. `pacman -Qm`, `echo $PATH`, `ls /usr/bin/pacman`). Word-splitting, $VAR expansion, pipes, and redirection all work. Use `args` for positional parameters $1, $2, .... Provide exactly one of `command` or `script`."
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Positional parameters passed to the shell as $1, $2, ... (for both `command` and `script` modes; $0 is a placeholder)."
                },
                "script": {
                    "type": "string",
                    "description": "A multi-line shell script to execute via `sh -s`. Provide exactly one of `command` or `script`. Mutually exclusive with `stdin`."
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory for the command. Supports ~ expansion. If not specified, uses the server's current working directory."
                },
                "timeout_secs": {
                    "type": "number",
                    "description": "Absolute wall-clock timeout in seconds. Default: 30 seconds. Ignored when detach=true."
                },
                "inactivity_timeout_secs": {
                    "type": "number",
                    "description": "Kill the command if it produces no stdout/stderr output for this many seconds. Independent of (and evaluated alongside) the absolute timeout_secs; whichever cap is reached first fires. Any output resets the clock, so a command that keeps producing output runs until the absolute timeout. Omit or set to 0 to disable. Ignored when detach=true."
                },
                "stdin": {
                    "type": "string",
                    "description": "Input to send to the process's stdin. Mutually exclusive with 'script'."
                },
                "max_lines": {
                    "type": "number",
                    "description": "Maximum number of lines to return for stdout and stderr (keeps the last N lines). Default: 200. Set to 0 for unlimited."
                },
                "detach": {
                    "type": "boolean",
                    "description": "Launch the process fire-and-forget in a new session: no timeout, no captured output; returns immediately with 'detached_pid'. Default: false."
                }
            }
        }),
    )
}

fn terminal_store_script_tool() -> ToolDef {
    ToolDef::new(
        "terminal_store_script",
        "Store a named shell script that becomes available as a dynamic tool 'script_<name>'. Scripts are session-scoped and cleared on server restart.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Script name (alphanumeric and underscores only). The tool will be available as 'script_<name>'."
                },
                "description": {
                    "type": "string",
                    "description": "Human-readable description of what the script does."
                },
                "script": {
                    "type": "string",
                    "description": "The shell script body."
                },
                "parameters": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "Parameter name (will be set as an environment variable)."
                            },
                            "description": {
                                "type": "string",
                                "description": "Description of the parameter."
                            },
                            "required": {
                                "type": "boolean",
                                "description": "Whether this parameter is required. Default: false."
                            }
                        },
                        "required": ["name"]
                    },
                    "description": "Named parameters that will be passed as environment variables to the script."
                }
            },
            "required": ["name", "description", "script"]
        }),
    )
}

fn terminal_remove_script_tool() -> ToolDef {
    ToolDef::new(
        "terminal_remove_script",
        "Remove a stored script, removing its dynamic tool.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the script to remove."
                }
            },
            "required": ["name"]
        }),
    )
}

fn terminal_list_scripts_tool() -> ToolDef {
    ToolDef::new(
        "terminal_list_scripts",
        "List all stored scripts with their names, descriptions, and parameter counts.",
        serde_json::json!({
            "type": "object",
            "properties": {}
        }),
    )
}

fn dynamic_script_tool(stored: &StoredScript) -> ToolDef {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    for param in &stored.parameters {
        properties.insert(
            param.name.clone(),
            serde_json::json!({
                "type": "string",
                "description": param.description
            }),
        );
        if param.required {
            required.push(Value::String(param.name.clone()));
        }
    }

    // Add common execution parameters
    properties.insert(
        "cwd".to_string(),
        serde_json::json!({
            "type": "string",
            "description": "Working directory for the script."
        }),
    );
    properties.insert(
        "timeout_secs".to_string(),
        serde_json::json!({
            "type": "number",
            "description": "Timeout in seconds. Default: 30."
        }),
    );
    properties.insert(
        "max_lines".to_string(),
        serde_json::json!({
            "type": "number",
            "description": "Maximum output lines. Default: 200. 0 for unlimited."
        }),
    );

    ToolDef::new(
        format!("script_{}", stored.name),
        stored.description.clone(),
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse the JSON payload carried in a reply's `structuredContent`.
    fn outcome_value(outcome: &ToolOutcome) -> Value {
        outcome
            .reply
            .structured_content
            .clone()
            .expect("reply carries structuredContent")
    }

    /// Return the text of a reply's first content block.
    fn outcome_text(outcome: &ToolOutcome) -> String {
        match outcome.reply.content.first().expect("a content block") {
            mcp_core::Content::Text(t) => t.clone(),
            mcp_core::Content::Raw(v) => v.to_string(),
        }
    }

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

    #[tokio::test]
    async fn test_script_param_basic() {
        let registry = ToolRegistry::new();
        let args = serde_json::json!({"script": "echo hello_script"});
        let outcome = registry
            .execute_tool("terminal_execute", &args)
            .await
            .unwrap();
        assert!(!outcome.tools_changed);
        let val = outcome_value(&outcome);
        assert_eq!(val["exit_code"], 0);
        assert!(val["stdout"].as_str().unwrap().contains("hello_script"));
    }

    #[tokio::test]
    async fn test_script_and_stdin_mutually_exclusive() {
        let registry = ToolRegistry::new();
        let args = serde_json::json!({"script": "echo hi", "stdin": "data"});
        let res = registry.execute_tool("terminal_execute", &args).await;
        assert!(res.is_err());
        let msg = format!("{}", res.err().unwrap());
        assert!(msg.contains("script") && msg.contains("stdin"));
    }

    #[tokio::test]
    async fn test_store_and_list_scripts() {
        let registry = ToolRegistry::new();
        let store_args = serde_json::json!({
            "name": "my_script",
            "description": "A test script",
            "script": "echo stored",
        });
        let outcome = registry
            .execute_tool("terminal_store_script", &store_args)
            .await
            .unwrap();
        assert!(outcome.tools_changed);

        let list_outcome = registry
            .execute_tool("terminal_list_scripts", &serde_json::json!({}))
            .await
            .unwrap();
        let scripts = outcome_value(&list_outcome);
        let arr = scripts.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "my_script");
    }

    #[tokio::test]
    async fn test_store_and_call_dynamic_script() {
        let registry = ToolRegistry::new();
        registry
            .execute_tool(
                "terminal_store_script",
                &serde_json::json!({
                    "name": "greet",
                    "description": "Greet",
                    "script": "echo hello_dynamic",
                }),
            )
            .await
            .unwrap();

        let outcome = registry
            .execute_tool("script_greet", &serde_json::json!({}))
            .await
            .unwrap();
        let val = outcome_value(&outcome);
        assert_eq!(val["exit_code"], 0);
        assert!(val["stdout"].as_str().unwrap().contains("hello_dynamic"));
    }

    #[tokio::test]
    async fn test_remove_script() {
        let registry = ToolRegistry::new();
        registry
            .execute_tool(
                "terminal_store_script",
                &serde_json::json!({
                    "name": "to_remove",
                    "description": "temp",
                    "script": "echo temp",
                }),
            )
            .await
            .unwrap();

        let outcome = registry
            .execute_tool(
                "terminal_remove_script",
                &serde_json::json!({"name": "to_remove"}),
            )
            .await
            .unwrap();
        assert!(outcome.tools_changed);

        // Calling removed script should fail
        let res = registry
            .execute_tool("script_to_remove", &serde_json::json!({}))
            .await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_dangerous_param_name_rejected() {
        let registry = ToolRegistry::new();
        for bad in ["LD_PRELOAD", "BASH_ENV", "1BAD", "has-dash", "IFS"] {
            let res = registry
                .execute_tool(
                    "terminal_store_script",
                    &serde_json::json!({
                        "name": "with_bad_param",
                        "description": "desc",
                        "script": "echo hi",
                        "parameters": [{"name": bad}]
                    }),
                )
                .await;
            assert!(res.is_err(), "param name '{bad}' should be rejected");
        }
    }

    #[tokio::test]
    async fn test_invalid_script_name() {
        let registry = ToolRegistry::new();
        let res = registry
            .execute_tool(
                "terminal_store_script",
                &serde_json::json!({
                    "name": "bad-name",
                    "description": "desc",
                    "script": "echo hi",
                }),
            )
            .await;
        assert!(res.is_err());
        let msg = format!("{}", res.err().unwrap());
        assert!(msg.to_lowercase().contains("invalid"));
    }

    #[tokio::test]
    async fn test_dynamic_tool_appears_in_list() {
        let registry = ToolRegistry::new();
        registry
            .execute_tool(
                "terminal_store_script",
                &serde_json::json!({
                    "name": "listed",
                    "description": "A listed script",
                    "script": "echo listed",
                }),
            )
            .await
            .unwrap();

        let tools = registry.list_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"script_listed"));
    }

    #[tokio::test]
    async fn test_script_with_parameters() {
        let registry = ToolRegistry::new();
        registry
            .execute_tool(
                "terminal_store_script",
                &serde_json::json!({
                    "name": "parameterized",
                    "description": "Script with params",
                    "script": "echo $GREETING $TARGET",
                    "parameters": [
                        {"name": "GREETING", "description": "The greeting", "required": true},
                        {"name": "TARGET", "description": "Who to greet", "required": false}
                    ]
                }),
            )
            .await
            .unwrap();

        let outcome = registry
            .execute_tool(
                "script_parameterized",
                &serde_json::json!({"GREETING": "hello", "TARGET": "world"}),
            )
            .await
            .unwrap();
        let val = outcome_value(&outcome);
        assert_eq!(val["exit_code"], 0);
        assert!(val["stdout"].as_str().unwrap().contains("hello world"));
    }

    #[tokio::test]
    async fn test_script_overwrite() {
        let registry = ToolRegistry::new();
        registry
            .execute_tool(
                "terminal_store_script",
                &serde_json::json!({
                    "name": "overwrite_me",
                    "description": "v1",
                    "script": "echo v1",
                }),
            )
            .await
            .unwrap();

        let store_outcome = registry
            .execute_tool(
                "terminal_store_script",
                &serde_json::json!({
                    "name": "overwrite_me",
                    "description": "v2",
                    "script": "echo v2",
                }),
            )
            .await
            .unwrap();
        let text = outcome_text(&store_outcome);
        assert!(text.contains("updated"));

        let outcome = registry
            .execute_tool("script_overwrite_me", &serde_json::json!({}))
            .await
            .unwrap();
        let val = outcome_value(&outcome);
        assert!(val["stdout"].as_str().unwrap().contains("v2"));
    }
}
