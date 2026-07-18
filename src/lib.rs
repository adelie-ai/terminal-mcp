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
}

#[cfg(test)]
mod tests {
    use super::*;

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
