#![deny(warnings)]

// Tool registry and MCP tool definitions

use crate::error::{McpError, Result, ScriptError};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

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

/// Tool registry that manages all available tools
pub struct ToolRegistry {
    scripts: Arc<RwLock<HashMap<String, StoredScript>>>,
}

impl ToolRegistry {
    /// Create a new tool registry
    pub fn new() -> Self {
        Self {
            scripts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get all tools in MCP format
    pub async fn list_tools(&self) -> Value {
        let mut tools = vec![
            terminal_execute_schema(),
            terminal_store_script_schema(),
            terminal_remove_script_schema(),
            terminal_list_scripts_schema(),
        ];

        let scripts = self.scripts.read().await;
        for stored in scripts.values() {
            tools.push(dynamic_script_tool_schema(stored));
        }

        Value::Array(tools)
    }

    /// Execute a tool by name. Returns (result_value, tools_changed).
    pub async fn execute_tool(&self, name: &str, arguments: &Value) -> Result<(Value, bool)> {
        let args = arguments.as_object().ok_or_else(|| {
            McpError::InvalidToolParameters("Arguments must be an object".to_string())
        })?;

        match name {
            "terminal_execute" => {
                let result = self.exec_terminal_execute(args).await?;
                Ok((result, false))
            }
            "terminal_store_script" => {
                let result = self.exec_store_script(args).await?;
                Ok((result, true))
            }
            "terminal_remove_script" => {
                let result = self.exec_remove_script(args).await?;
                Ok((result, true))
            }
            "terminal_list_scripts" => {
                let result = self.exec_list_scripts().await?;
                Ok((result, false))
            }
            _ => {
                // Check for dynamic script tool: script_<name>
                if let Some(script_name) = name.strip_prefix("script_") {
                    let result = self.exec_dynamic_script(script_name, args).await?;
                    Ok((result, false))
                } else {
                    Err(McpError::ToolNotFound(name.to_string()).into())
                }
            }
        }
    }

    async fn exec_terminal_execute(
        &self,
        args: &serde_json::Map<String, Value>,
    ) -> Result<Value> {
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
        let max_lines = args
            .get("max_lines")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        let result = if let Some(script_body) = script {
            let cmd_args: Option<Vec<String>> = args.get("args").and_then(|v| {
                v.as_array().map(|arr| {
                    arr.iter()
                        .filter_map(|item| item.as_str().map(|s| s.to_string()))
                        .collect()
                })
            });

            crate::operations::execute::execute_script(
                script_body,
                cmd_args.as_deref(),
                cwd,
                timeout_secs,
                max_lines,
                None,
            )
            .await?
        } else {
            let command =
                args.get("command")
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

            crate::operations::execute::execute(
                command,
                cmd_args.as_deref(),
                cwd,
                timeout_secs,
                stdin_input,
                max_lines,
            )
            .await?
        };

        Ok(execution_result_json(&result))
    }

    async fn exec_store_script(
        &self,
        args: &serde_json::Map<String, Value>,
    ) -> Result<Value> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
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

        let script_body = args
            .get("script")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::InvalidToolParameters("Missing required parameter: script".to_string())
            })?;

        let parameters = parse_script_parameters(args.get("parameters"))?;

        let stored = StoredScript {
            name: name.to_string(),
            description: description.to_string(),
            script: script_body.to_string(),
            parameters,
        };

        let mut scripts = self.scripts.write().await;
        let overwritten = scripts.insert(name.to_string(), stored).is_some();

        Ok(serde_json::json!({
            "content": [{
                "type": "text",
                "text": if overwritten {
                    format!("Script '{}' updated. Available as tool 'script_{}'.", name, name)
                } else {
                    format!("Script '{}' stored. Available as tool 'script_{}'.", name, name)
                }
            }]
        }))
    }

    async fn exec_remove_script(
        &self,
        args: &serde_json::Map<String, Value>,
    ) -> Result<Value> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::InvalidToolParameters("Missing required parameter: name".to_string())
            })?;

        let mut scripts = self.scripts.write().await;
        if scripts.remove(name).is_some() {
            Ok(serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": format!("Script '{}' removed.", name)
                }]
            }))
        } else {
            Err(ScriptError::NotFound(name.to_string()).into())
        }
    }

    async fn exec_list_scripts(&self) -> Result<Value> {
        let scripts = self.scripts.read().await;
        let list: Vec<Value> = scripts
            .values()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "description": s.description,
                    "parameter_count": s.parameters.len(),
                })
            })
            .collect();

        Ok(serde_json::json!({
            "content": [{
                "type": "json",
                "value": list
            }]
        }))
    }

    async fn exec_dynamic_script(
        &self,
        script_name: &str,
        args: &serde_json::Map<String, Value>,
    ) -> Result<Value> {
        let scripts = self.scripts.read().await;
        let stored = scripts
            .get(script_name)
            .ok_or_else(|| ScriptError::NotFound(script_name.to_string()))?
            .clone();
        drop(scripts);

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
            max_lines,
            env_ref,
        )
        .await?;

        Ok(execution_result_json(&result))
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn execution_result_json(result: &crate::operations::execute::ExecuteResult) -> Value {
    serde_json::json!({
        "content": [{
            "type": "json",
            "value": {
                "exit_code": result.exit_code,
                "stdout": result.stdout,
                "stderr": result.stderr,
                "timed_out": result.timed_out,
                "stdout_truncated": result.stdout_truncated,
                "stderr_truncated": result.stderr_truncated,
            }
        }]
    })
}

fn validate_script_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(ScriptError::InvalidName("Name cannot be empty".to_string()).into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
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

fn parse_script_parameters(val: Option<&Value>) -> Result<Vec<ScriptParameter>> {
    let Some(arr) = val.and_then(|v| v.as_array()) else {
        return Ok(vec![]);
    };

    let mut params = Vec::new();
    for item in arr {
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::InvalidToolParameters(
                    "Each parameter must have a 'name' string".to_string(),
                )
            })?;
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

fn terminal_execute_schema() -> Value {
    serde_json::json!({
        "name": "terminal_execute",
        "description": "Execute a shell command or script and return stdout/stderr. Use 'command' for direct execution or 'script' for multi-line shell scripts. Returns exit code, stdout, stderr, and timeout status.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute. This is the program name (e.g., 'ls', 'git', 'python3'). Mutually exclusive with 'script'."
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Arguments to pass to the command (or positional args $1, $2, ... when using 'script')."
                },
                "script": {
                    "type": "string",
                    "description": "A shell script to execute via 'sh -s'. Mutually exclusive with 'command' and 'stdin'. Use this for multi-line scripts instead of 'sh -c'."
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory for the command. Supports ~ expansion. If not specified, uses the server's current working directory."
                },
                "timeout_secs": {
                    "type": "number",
                    "description": "Timeout in seconds. Default: 30 seconds."
                },
                "stdin": {
                    "type": "string",
                    "description": "Input to send to the process's stdin. Mutually exclusive with 'script'."
                },
                "max_lines": {
                    "type": "number",
                    "description": "Maximum number of lines to return for stdout and stderr (keeps the last N lines). Default: 200. Set to 0 for unlimited."
                }
            }
        }
    })
}

fn terminal_store_script_schema() -> Value {
    serde_json::json!({
        "name": "terminal_store_script",
        "description": "Store a named shell script that becomes available as a dynamic tool 'script_<name>'. Scripts are session-scoped and cleared on server restart.",
        "inputSchema": {
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
        }
    })
}

fn terminal_remove_script_schema() -> Value {
    serde_json::json!({
        "name": "terminal_remove_script",
        "description": "Remove a stored script, removing its dynamic tool.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the script to remove."
                }
            },
            "required": ["name"]
        }
    })
}

fn terminal_list_scripts_schema() -> Value {
    serde_json::json!({
        "name": "terminal_list_scripts",
        "description": "List all stored scripts with their names, descriptions, and parameter counts.",
        "inputSchema": {
            "type": "object",
            "properties": {}
        }
    })
}

fn dynamic_script_tool_schema(stored: &StoredScript) -> Value {
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

    serde_json::json!({
        "name": format!("script_{}", stored.name),
        "description": stored.description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required
        }
    })
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

    #[tokio::test]
    async fn test_script_param_basic() {
        let registry = ToolRegistry::new();
        let args = serde_json::json!({"script": "echo hello_script"});
        let (result, changed) = registry
            .execute_tool("terminal_execute", &args)
            .await
            .unwrap();
        assert!(!changed);
        let val = &result["content"][0]["value"];
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
        let (_, changed) = registry
            .execute_tool("terminal_store_script", &store_args)
            .await
            .unwrap();
        assert!(changed);

        let (list_result, _) = registry
            .execute_tool("terminal_list_scripts", &serde_json::json!({}))
            .await
            .unwrap();
        let scripts = &list_result["content"][0]["value"];
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

        let (result, _) = registry
            .execute_tool("script_greet", &serde_json::json!({}))
            .await
            .unwrap();
        let val = &result["content"][0]["value"];
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

        let (_, changed) = registry
            .execute_tool(
                "terminal_remove_script",
                &serde_json::json!({"name": "to_remove"}),
            )
            .await
            .unwrap();
        assert!(changed);

        // Calling removed script should fail
        let res = registry
            .execute_tool("script_to_remove", &serde_json::json!({}))
            .await;
        assert!(res.is_err());
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

        let tools = registry.list_tools().await;
        let arr = tools.as_array().unwrap();
        let names: Vec<&str> = arr
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();
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

        let (result, _) = registry
            .execute_tool(
                "script_parameterized",
                &serde_json::json!({"GREETING": "hello", "TARGET": "world"}),
            )
            .await
            .unwrap();
        let val = &result["content"][0]["value"];
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

        let (store_result, _) = registry
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
        let text = store_result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("updated"));

        let (result, _) = registry
            .execute_tool("script_overwrite_me", &serde_json::json!({}))
            .await
            .unwrap();
        let val = &result["content"][0]["value"];
        assert!(val["stdout"].as_str().unwrap().contains("v2"));
    }
}
