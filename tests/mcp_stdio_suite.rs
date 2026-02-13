#![deny(warnings)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
struct McpStdioClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: u64,
    /// Notifications received between responses
    pending_notifications: Vec<Value>,
}

impl McpStdioClient {
    fn start() -> Self {
        let exe = env!("CARGO_BIN_EXE_terminal-mcp");

        let mut child = Command::new(exe)
            .args(["serve", "--mode", "stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn terminal-mcp serve --mode stdio");

        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");

        Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            pending_notifications: Vec::new(),
        }
    }

    fn send(&mut self, obj: &Value) {
        let s = serde_json::to_string(obj).expect("serialize jsonrpc");
        self.stdin
            .write_all(s.as_bytes())
            .and_then(|_| self.stdin.write_all(b"\n"))
            .and_then(|_| self.stdin.flush())
            .expect("write jsonrpc line");
    }

    fn read_msg(&mut self) -> Value {
        let mut line = String::new();
        loop {
            line.clear();
            let n = self.stdout.read_line(&mut line).expect("read line");
            if n == 0 {
                panic!("mcp server closed stdout");
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                return v;
            }
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;

        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));

        loop {
            let msg = self.read_msg();
            // Check if this is a notification (no id field)
            if msg.get("id").is_none() && msg.get("method").is_some() {
                self.pending_notifications.push(msg);
                continue;
            }
            if msg.get("id").and_then(|v| v.as_u64()) != Some(id) {
                continue;
            }
            if let Some(err) = msg.get("error") {
                return Err(err.to_string());
            }
            return Ok(msg);
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(&json!({"jsonrpc":"2.0","method":method,"params":params}));
    }

    fn initialize(&mut self) {
        self.call(
            "initialize",
            json!({"protocolVersion":"2025-11-25","capabilities":{}}),
        )
        .expect("initialize");
        self.notify("initialized", json!({}));
    }

    fn tool_call(&mut self, name: &str, arguments: Value) -> Result<Value, String> {
        let resp = self.call("tools/call", json!({"name":name,"arguments":arguments}))?;
        resp.get("result")
            .cloned()
            .ok_or_else(|| format!("missing result field: {resp}"))
    }

    /// Drain and return any pending notifications collected since last call.
    fn take_notifications(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.pending_notifications)
    }

    /// Collect notifications by making a cheap no-op call (tools/list) that
    /// forces reading through any pending notification messages on the wire.
    fn collect_notifications(&mut self) -> Vec<Value> {
        // A cheap call forces us to read through any pending notifications
        // that the server sent after the previous response.
        let _ = self.call("tools/list", json!({}));
        std::mem::take(&mut self.pending_notifications)
    }
}

impl Drop for McpStdioClient {
    fn drop(&mut self) {
        let _ = self.call("shutdown", json!({}));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn extract_value(tool_result: &Value) -> Value {
    let content = tool_result
        .get("content")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("expected result.content array, got: {tool_result}"));

    for entry in content {
        if entry.get("type") == Some(&Value::String("json".to_string())) {
            if let Some(v) = entry.get("value") {
                return v.clone();
            }
        }
    }

    for entry in content {
        if entry.get("type") == Some(&Value::String("text".to_string())) {
            if let Some(text) = entry.get("text").and_then(|v| v.as_str()) {
                let trimmed = text.trim();
                if trimmed.starts_with('{') || trimmed.starts_with('[') {
                    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                        return v;
                    }
                }
                return Value::String(text.to_string());
            }
        }
    }

    panic!("no usable content entry in: {tool_result}");
}

fn extract_text(tool_result: &Value) -> String {
    let content = tool_result
        .get("content")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("expected result.content array, got: {tool_result}"));

    for entry in content {
        if entry.get("type") == Some(&Value::String("text".to_string())) {
            if let Some(text) = entry.get("text").and_then(|v| v.as_str()) {
                return text.to_string();
            }
        }
    }

    panic!("no text content entry in: {tool_result}");
}

fn expect_err_contains<T>(res: Result<T, String>, needle: &str) {
    match res {
        Ok(_) => panic!("expected error containing '{needle}', but call succeeded"),
        Err(e) => {
            let lower = e.to_lowercase();
            assert!(
                lower.contains(&needle.to_lowercase()),
                "expected error containing '{needle}', got: {e}"
            );
        }
    }
}

fn run_case(f: impl FnOnce(&mut McpStdioClient)) {
    let mut client = McpStdioClient::start();
    client.initialize();
    f(&mut client);
}

// -----------------
// End-to-end MCP stdio parity suite
// -----------------

#[test]
fn terminal_execute_simple_command() {
    run_case(|client| {
        let res = client
            .tool_call(
                "terminal_execute",
                json!({"command": "echo", "args": ["hello"]}),
            )
            .unwrap();
        let v = extract_value(&res);
        assert_eq!(v.get("exit_code").and_then(|x| x.as_i64()), Some(0));
        assert_eq!(v.get("stdout").and_then(|x| x.as_str()).unwrap().trim(), "hello");
        assert_eq!(v.get("timed_out").and_then(|x| x.as_bool()), Some(false));
    });
}

#[test]
fn terminal_execute_capture_stderr() {
    run_case(|client| {
        let res = client
            .tool_call(
                "terminal_execute",
                json!({"command": "sh", "args": ["-c", "echo err >&2"]}),
            )
            .unwrap();
        let v = extract_value(&res);
        assert_eq!(v.get("exit_code").and_then(|x| x.as_i64()), Some(0));
        assert!(v.get("stdout").and_then(|x| x.as_str()).unwrap().is_empty());
        assert_eq!(
            v.get("stderr").and_then(|x| x.as_str()).unwrap().trim(),
            "err"
        );
    });
}

#[test]
fn terminal_execute_non_zero_exit() {
    run_case(|client| {
        let res = client
            .tool_call("terminal_execute", json!({"command": "false"}))
            .unwrap();
        let v = extract_value(&res);
        assert_ne!(v.get("exit_code").and_then(|x| x.as_i64()), Some(0));
        assert_eq!(v.get("timed_out").and_then(|x| x.as_bool()), Some(false));
    });
}

#[test]
fn terminal_execute_custom_cwd() {
    run_case(|client| {
        let res = client
            .tool_call(
                "terminal_execute",
                json!({"command": "pwd", "cwd": "/tmp"}),
            )
            .unwrap();
        let v = extract_value(&res);
        assert_eq!(v.get("exit_code").and_then(|x| x.as_i64()), Some(0));
        assert!(v.get("stdout").and_then(|x| x.as_str()).unwrap().contains("tmp"));
    });
}

#[test]
fn terminal_execute_timeout() {
    run_case(|client| {
        let res = client
            .tool_call(
                "terminal_execute",
                json!({"command": "sleep", "args": ["10"], "timeout_secs": 1}),
            )
            .unwrap();
        let v = extract_value(&res);
        assert_eq!(v.get("timed_out").and_then(|x| x.as_bool()), Some(true));
        assert_eq!(v.get("exit_code").and_then(|x| x.as_i64()), Some(-1));
    });
}

#[test]
fn terminal_execute_with_args() {
    run_case(|client| {
        let res = client
            .tool_call(
                "terminal_execute",
                json!({"command": "echo", "args": ["one", "two", "three"]}),
            )
            .unwrap();
        let v = extract_value(&res);
        assert_eq!(v.get("exit_code").and_then(|x| x.as_i64()), Some(0));
        assert_eq!(
            v.get("stdout").and_then(|x| x.as_str()).unwrap().trim(),
            "one two three"
        );
    });
}

#[test]
fn terminal_execute_stdin_piping() {
    run_case(|client| {
        let res = client
            .tool_call(
                "terminal_execute",
                json!({"command": "cat", "stdin": "hello from stdin"}),
            )
            .unwrap();
        let v = extract_value(&res);
        assert_eq!(v.get("exit_code").and_then(|x| x.as_i64()), Some(0));
        assert_eq!(
            v.get("stdout").and_then(|x| x.as_str()).unwrap(),
            "hello from stdin"
        );
    });
}

#[test]
fn terminal_execute_command_not_found() {
    run_case(|client| {
        let res = client.tool_call(
            "terminal_execute",
            json!({"command": "nonexistent_command_xyz_12345"}),
        );
        expect_err_contains(res, "not found");
    });
}

#[test]
fn terminal_execute_empty_command() {
    run_case(|client| {
        let res = client.tool_call("terminal_execute", json!({"command": ""}));
        expect_err_contains(res, "invalid command");
    });
}

#[test]
fn terminal_execute_invalid_cwd() {
    run_case(|client| {
        let res = client.tool_call(
            "terminal_execute",
            json!({"command": "echo", "args": ["hi"], "cwd": "/nonexistent_dir_xyz"}),
        );
        expect_err_contains(res, "does not exist");
    });
}

#[test]
fn terminal_execute_missing_command_param() {
    run_case(|client| {
        let res = client.tool_call("terminal_execute", json!({}));
        expect_err_contains(res, "command");
    });
}

#[test]
fn terminal_execute_tool_not_found() {
    run_case(|client| {
        let res = client.tool_call("nonexistent_tool", json!({"command": "echo"}));
        expect_err_contains(res, "not found");
    });
}

#[test]
fn terminal_execute_max_lines_truncation() {
    run_case(|client| {
        let res = client
            .tool_call(
                "terminal_execute",
                json!({"command": "sh", "args": ["-c", "for i in $(seq 1 10); do echo line$i; done"], "max_lines": 3}),
            )
            .unwrap();
        let v = extract_value(&res);
        assert_eq!(v.get("exit_code").and_then(|x| x.as_i64()), Some(0));
        assert_eq!(v.get("stdout_truncated").and_then(|x| x.as_bool()), Some(true));
        let stdout = v.get("stdout").and_then(|x| x.as_str()).unwrap();
        let lines: Vec<&str> = stdout.trim().lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "line8");
        assert_eq!(lines[2], "line10");
    });
}

#[test]
fn terminal_execute_max_lines_default_not_truncated() {
    run_case(|client| {
        let res = client
            .tool_call(
                "terminal_execute",
                json!({"command": "echo", "args": ["hello"]}),
            )
            .unwrap();
        let v = extract_value(&res);
        assert_eq!(v.get("stdout_truncated").and_then(|x| x.as_bool()), Some(false));
        assert_eq!(v.get("stderr_truncated").and_then(|x| x.as_bool()), Some(false));
    });
}

#[test]
fn terminal_execute_max_lines_zero_unlimited() {
    run_case(|client| {
        let res = client
            .tool_call(
                "terminal_execute",
                json!({"command": "sh", "args": ["-c", "for i in $(seq 1 300); do echo line$i; done"], "max_lines": 0}),
            )
            .unwrap();
        let v = extract_value(&res);
        assert_eq!(v.get("stdout_truncated").and_then(|x| x.as_bool()), Some(false));
        let stdout = v.get("stdout").and_then(|x| x.as_str()).unwrap();
        let lines: Vec<&str> = stdout.trim().lines().collect();
        assert_eq!(lines.len(), 300);
    });
}

// -----------------
// Script parameter tests
// -----------------

#[test]
fn terminal_execute_script_param_basic() {
    run_case(|client| {
        let res = client
            .tool_call(
                "terminal_execute",
                json!({"script": "echo line1\necho line2"}),
            )
            .unwrap();
        let v = extract_value(&res);
        assert_eq!(v.get("exit_code").and_then(|x| x.as_i64()), Some(0));
        let stdout = v.get("stdout").and_then(|x| x.as_str()).unwrap();
        let lines: Vec<&str> = stdout.trim().lines().collect();
        assert_eq!(lines, vec!["line1", "line2"]);
    });
}

#[test]
fn terminal_execute_script_and_stdin_error() {
    run_case(|client| {
        let res = client.tool_call(
            "terminal_execute",
            json!({"script": "echo hi", "stdin": "data"}),
        );
        expect_err_contains(res, "script");
    });
}

#[test]
fn terminal_execute_script_with_args() {
    run_case(|client| {
        let res = client
            .tool_call(
                "terminal_execute",
                json!({"script": "echo \"$1 $2\"", "args": ["hello", "world"]}),
            )
            .unwrap();
        let v = extract_value(&res);
        assert_eq!(v.get("exit_code").and_then(|x| x.as_i64()), Some(0));
        assert_eq!(
            v.get("stdout").and_then(|x| x.as_str()).unwrap().trim(),
            "hello world"
        );
    });
}

#[test]
fn terminal_execute_script_with_cwd() {
    run_case(|client| {
        let res = client
            .tool_call(
                "terminal_execute",
                json!({"script": "pwd", "cwd": "/tmp"}),
            )
            .unwrap();
        let v = extract_value(&res);
        assert_eq!(v.get("exit_code").and_then(|x| x.as_i64()), Some(0));
        assert!(v.get("stdout").and_then(|x| x.as_str()).unwrap().contains("tmp"));
    });
}

// -----------------
// Stored scripts tests
// -----------------

#[test]
fn store_script_appears_in_tools_list() {
    run_case(|client| {
        client
            .tool_call(
                "terminal_store_script",
                json!({
                    "name": "my_build",
                    "description": "Build the project",
                    "script": "echo building"
                }),
            )
            .unwrap();

        // Drain notifications from store
        client.collect_notifications();

        let resp = client
            .call("tools/list", json!({}))
            .expect("tools/list");
        let tools = resp["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();
        assert!(
            names.contains(&"script_my_build"),
            "Expected script_my_build in tools list, got: {:?}",
            names
        );
    });
}

#[test]
fn call_stored_script_by_dynamic_name() {
    run_case(|client| {
        client
            .tool_call(
                "terminal_store_script",
                json!({
                    "name": "greeter",
                    "description": "Greet",
                    "script": "echo hello_from_stored"
                }),
            )
            .unwrap();

        let res = client
            .tool_call("script_greeter", json!({}))
            .unwrap();
        let v = extract_value(&res);
        assert_eq!(v.get("exit_code").and_then(|x| x.as_i64()), Some(0));
        assert!(v.get("stdout").and_then(|x| x.as_str()).unwrap().contains("hello_from_stored"));
    });
}

#[test]
fn stored_script_with_parameters_sets_env_vars() {
    run_case(|client| {
        client
            .tool_call(
                "terminal_store_script",
                json!({
                    "name": "env_test",
                    "description": "Test env vars",
                    "script": "echo $GREETING $TARGET",
                    "parameters": [
                        {"name": "GREETING", "description": "The greeting", "required": true},
                        {"name": "TARGET", "description": "Who to greet"}
                    ]
                }),
            )
            .unwrap();

        let res = client
            .tool_call(
                "script_env_test",
                json!({"GREETING": "hi", "TARGET": "earth"}),
            )
            .unwrap();
        let v = extract_value(&res);
        assert_eq!(v.get("exit_code").and_then(|x| x.as_i64()), Some(0));
        assert_eq!(
            v.get("stdout").and_then(|x| x.as_str()).unwrap().trim(),
            "hi earth"
        );
    });
}

#[test]
fn remove_script_disappears_from_tools_list() {
    run_case(|client| {
        client
            .tool_call(
                "terminal_store_script",
                json!({
                    "name": "to_remove",
                    "description": "Will be removed",
                    "script": "echo temp"
                }),
            )
            .unwrap();

        client
            .tool_call(
                "terminal_remove_script",
                json!({"name": "to_remove"}),
            )
            .unwrap();

        // Drain notifications
        client.collect_notifications();

        let resp = client
            .call("tools/list", json!({}))
            .expect("tools/list");
        let tools = resp["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();
        assert!(
            !names.contains(&"script_to_remove"),
            "script_to_remove should not be in tools list"
        );
    });
}

#[test]
fn list_scripts_returns_stored_scripts() {
    run_case(|client| {
        client
            .tool_call(
                "terminal_store_script",
                json!({
                    "name": "script_a",
                    "description": "Script A",
                    "script": "echo a"
                }),
            )
            .unwrap();
        client
            .tool_call(
                "terminal_store_script",
                json!({
                    "name": "script_b",
                    "description": "Script B",
                    "script": "echo b",
                    "parameters": [{"name": "X", "description": "x"}]
                }),
            )
            .unwrap();

        let res = client
            .tool_call("terminal_list_scripts", json!({}))
            .unwrap();
        let v = extract_value(&res);
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        let names: Vec<&str> = arr.iter().filter_map(|s| s["name"].as_str()).collect();
        assert!(names.contains(&"script_a"));
        assert!(names.contains(&"script_b"));
    });
}

#[test]
fn list_changed_notification_after_store() {
    run_case(|client| {
        // Clear any existing notifications
        client.take_notifications();

        client
            .tool_call(
                "terminal_store_script",
                json!({
                    "name": "notif_test",
                    "description": "test",
                    "script": "echo test"
                }),
            )
            .unwrap();

        let notifications = client.collect_notifications();
        assert!(
            notifications.iter().any(|n| {
                n.get("method").and_then(|m| m.as_str())
                    == Some("notifications/tools/list_changed")
            }),
            "Expected list_changed notification, got: {:?}",
            notifications
        );
    });
}

#[test]
fn list_changed_notification_after_remove() {
    run_case(|client| {
        client
            .tool_call(
                "terminal_store_script",
                json!({
                    "name": "remove_notif",
                    "description": "test",
                    "script": "echo test"
                }),
            )
            .unwrap();

        // Drain store notification
        client.collect_notifications();

        client
            .tool_call(
                "terminal_remove_script",
                json!({"name": "remove_notif"}),
            )
            .unwrap();

        let notifications = client.collect_notifications();
        assert!(
            notifications.iter().any(|n| {
                n.get("method").and_then(|m| m.as_str())
                    == Some("notifications/tools/list_changed")
            }),
            "Expected list_changed notification after remove, got: {:?}",
            notifications
        );
    });
}

#[test]
fn invalid_script_name_rejected() {
    run_case(|client| {
        let res = client.tool_call(
            "terminal_store_script",
            json!({
                "name": "bad-name!",
                "description": "desc",
                "script": "echo hi"
            }),
        );
        expect_err_contains(res, "invalid");
    });
}

#[test]
fn duplicate_script_name_overwrites() {
    run_case(|client| {
        client
            .tool_call(
                "terminal_store_script",
                json!({
                    "name": "dup",
                    "description": "v1",
                    "script": "echo v1"
                }),
            )
            .unwrap();

        let res = client
            .tool_call(
                "terminal_store_script",
                json!({
                    "name": "dup",
                    "description": "v2",
                    "script": "echo v2"
                }),
            )
            .unwrap();
        let text = extract_text(&res);
        assert!(text.contains("updated"), "Expected 'updated' in: {}", text);

        // Verify v2 runs
        let run_res = client
            .tool_call("script_dup", json!({}))
            .unwrap();
        let v = extract_value(&run_res);
        assert!(v.get("stdout").and_then(|x| x.as_str()).unwrap().contains("v2"));
    });
}

#[test]
fn remove_nonexistent_script_errors() {
    run_case(|client| {
        let res = client.tool_call(
            "terminal_remove_script",
            json!({"name": "does_not_exist"}),
        );
        expect_err_contains(res, "not found");
    });
}

#[test]
fn stored_script_missing_required_param() {
    run_case(|client| {
        client
            .tool_call(
                "terminal_store_script",
                json!({
                    "name": "req_test",
                    "description": "test required params",
                    "script": "echo $MUST_HAVE",
                    "parameters": [
                        {"name": "MUST_HAVE", "description": "required", "required": true}
                    ]
                }),
            )
            .unwrap();

        // Call without required param
        let res = client.tool_call("script_req_test", json!({}));
        expect_err_contains(res, "MUST_HAVE");
    });
}

#[test]
fn initialize_reports_list_changed_true() {
    let mut client = McpStdioClient::start();
    let resp = client
        .call(
            "initialize",
            json!({"protocolVersion":"2025-11-25","capabilities":{}}),
        )
        .expect("initialize");
    let caps = &resp["result"]["capabilities"]["tools"];
    assert_eq!(
        caps["listChanged"],
        Value::Bool(true),
        "Expected listChanged: true in capabilities"
    );
}

#[test]
fn builtin_tools_present_in_list() {
    run_case(|client| {
        let resp = client
            .call("tools/list", json!({}))
            .expect("tools/list");
        let tools = resp["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();
        assert!(names.contains(&"terminal_execute"));
        assert!(names.contains(&"terminal_store_script"));
        assert!(names.contains(&"terminal_remove_script"));
        assert!(names.contains(&"terminal_list_scripts"));
    });
}
