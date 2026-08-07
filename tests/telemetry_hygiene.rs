//! Acceptance criteria that need a real process rather than an in-process
//! capture: what a default build resolves, and what actually reaches stdout.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// The value the stdout-hygiene test hunts for. It never needs to be absent
/// from stdout (the tool result legitimately echoes it back), only every
/// stdout *line* must still parse as JSON-RPC around it.
const SENTINEL: &str = "MARKER-terminal-secret-9f3d1c2a";

/// AC (epic AC2): a default-feature build resolves no `opentelemetry*` crate.
///
/// The `otel` feature is the only thing that adds one. A stdio-only server
/// that never turns it on must not compile one in, or every `cargo install`
/// in the fleet pays for an exporter it does not use.
#[test]
fn default_build_pulls_no_opentelemetry() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");

    let output = Command::new(cargo)
        .args(["tree", "--edges", "normal", "--prefix", "none", "--locked"])
        .arg("--manifest-path")
        .arg(manifest)
        .output()
        .expect("cargo tree must run");

    assert!(
        output.status.success(),
        "cargo tree failed, so this criterion is unproven: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let tree = String::from_utf8_lossy(&output.stdout);
    let found: Vec<&str> = tree
        .lines()
        .map(str::trim)
        .filter(|line| line.to_ascii_lowercase().starts_with("opentelemetry"))
        .collect();

    assert!(
        found.is_empty(),
        "a default-feature build must resolve no opentelemetry crate, but it resolved: {found:?}"
    );
}

/// AC (mcp-core#40, non-negotiable #3): with `RUST_LOG=trace`, every line
/// terminal-mcp writes to stdout parses as JSON-RPC, and the logs land on
/// stderr instead. This server speaks stdio; one log line on stdout corrupts
/// the protocol stream.
#[test]
fn stdout_carries_only_jsonrpc_at_trace_log_level() {
    let exe = env!("CARGO_BIN_EXE_terminal-mcp");

    let requests = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        // The success path, carrying content that must never surface as a raw
        // log line mixed into the stdout stream.
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "terminal_execute",
                "arguments": {"command": format!("echo {SENTINEL}")},
            },
        }),
        // A tool-level error (isError content) -- a different response shape,
        // which is also what drives the DEBUG "tool returned an error result"
        // line.
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {"name": "terminal_execute", "arguments": {}},
        }),
        // A protocol-level error (unknown tool).
        json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {"name": "nonexistent_tool", "arguments": {}},
        }),
    ];

    let mut child = Command::new(exe)
        .args(["serve", "--transport", "stdio"])
        .env("RUST_LOG", "trace")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("terminal-mcp must start");

    {
        let stdin = child.stdin.as_mut().expect("piped stdin");
        for request in &requests {
            writeln!(stdin, "{request}").expect("terminal-mcp must accept its input");
        }
    }
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("terminal-mcp must finish");
    assert!(
        output.status.success(),
        "terminal-mcp must exit cleanly, otherwise an empty stdout proves nothing: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");

    let mut replies = 0;
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let value: Value = serde_json::from_str(line).unwrap_or_else(|e| {
            panic!("every stdout line must be JSON-RPC, but {line:?} is not: {e}")
        });
        assert_eq!(
            value.get("jsonrpc").and_then(Value::as_str),
            Some("2.0"),
            "every stdout line must carry the JSON-RPC envelope: {line:?}"
        );
        replies += 1;
    }
    assert_eq!(replies, 5, "terminal-mcp must answer all five requests");

    assert!(
        stderr.contains("INFO") || stderr.contains("TRACE") || stderr.contains("DEBUG"),
        "at RUST_LOG=trace the logs must arrive on stderr, or the subscriber was never \
         installed. stderr was: {stderr:?}"
    );
}
