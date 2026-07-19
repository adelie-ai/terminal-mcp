#![deny(warnings)]

// Library crate for terminal-mcp.
//
// The JSON-RPC protocol, transports, and CLI are owned by mcp-core; this crate
// supplies the domain (shell execution, the dynamic script registry, audit
// logging) and the `mcp_core::McpService` adapter in `service`.

pub mod error;
pub mod operations;
pub mod service;
pub mod tools;

use mcp_core::ServerConfig;

/// Server-level MCP `instructions`: the model-facing hint returned from
/// `initialize` (and captured by the daemon as this server's searchable
/// description). It frames what the server does, when to reach for it, the key
/// tools by name, and the safety/config note the model needs before running
/// shell.
pub const SERVER_INSTRUCTIONS: &str = "Runs shell commands and scripts on the local machine and returns their exit code, stdout, and stderr. Reach for this whenever a request means actually doing something on this computer -- checking system or package state, listing or editing files, running a build or test, git operations, or launching an app -- rather than answering from memory. `terminal_execute` runs a one-off command or a multi-line script (with args, a working directory, a timeout, stdin, or detach=true to launch a long-running process fire-and-forget); `terminal_store_script`, `terminal_list_scripts`, and `terminal_remove_script` save reusable named scripts that appear as their own `script_<name>` tools until the server restarts. Commands run arbitrary shell as the server's non-root user and, by default, do not inherit its environment (secrets are scrubbed unless MCP_TERMINAL_INHERIT_ENV=1); an optional MCP_TERMINAL_ALLOWED_COMMANDS allowlist can restrict what may run.";

/// Build the [`ServerConfig`] handed to mcp-core at startup.
///
/// Centralised here (rather than inline in `main`) so the wiring is unit
/// testable: the server-level `instructions` blurb and the transport policy are
/// asserted in tests without standing up a transport.
///
/// Why the settings: the dynamic script set changes at runtime
/// (`terminal_store_script` / `terminal_remove_script`), so `listChanged` is
/// advertised; the websocket transport is refused because terminal-mcp executes
/// arbitrary shell and mcp-core's websocket transport is unauthenticated (MF-12).
pub fn server_config() -> ServerConfig {
    ServerConfig::new("terminal-mcp", env!("CARGO_PKG_VERSION"))
        .tools_list_changed(true)
        .without_websocket()
        .instructions(SERVER_INSTRUCTIONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_core::McpService;

    /// Acceptance (da#538 Phase C): the crate exposes a zero-config
    /// `build_service` constructor that yields a ready service with the
    /// built-in defaults -- the audit logger is absent unless
    /// `MCP_TERMINAL_LOG_DIR` points at a usable directory -- and that service
    /// advertises the built-in `terminal_execute` tool. This is the single
    /// default-construction path an in-process host calls.
    #[test]
    fn build_service_exposes_builtin_tools_with_defaults() {
        let service = build_service().expect("build_service must succeed with built-in defaults");
        let tool_names: Vec<String> = service.tools().into_iter().map(|t| t.name).collect();
        assert!(
            tool_names.iter().any(|n| n == "terminal_execute"),
            "default service must advertise the built-in terminal_execute tool, got {tool_names:?}"
        );
    }

    #[test]
    fn server_config_has_nonempty_instructions() {
        let config = server_config();
        let instructions = config
            .instructions
            .as_deref()
            .expect("server_config must set an instructions blurb");
        assert!(
            !instructions.trim().is_empty(),
            "instructions blurb must not be blank"
        );
    }

    /// The blurb is the model-facing "when to reach for this server" hint and
    /// the daemon's searchable server description, so it must name the primary
    /// tool and carry the non-root / scrubbed-environment safety note. These
    /// substrings also drive tool-search recall.
    #[test]
    fn server_instructions_name_tools_and_safety_note() {
        let config = server_config();
        let blurb = config
            .instructions
            .expect("server_config must set an instructions blurb")
            .to_lowercase();
        for term in [
            "terminal_execute",
            "script",
            "root",
            "mcp_terminal_inherit_env",
        ] {
            assert!(
                blurb.contains(term),
                "instructions blurb should mention '{term}'"
            );
        }
    }
}
