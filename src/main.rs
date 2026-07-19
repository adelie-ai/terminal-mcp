#![deny(warnings)]

// Binary entrypoint for terminal-mcp.
//
// mcp-core owns the CLI (`serve --transport <stdio|websocket|unix>`, with
// `--mode` accepted as a back-compat alias), the JSON-RPC protocol, framing,
// transports, and `tools/list_changed` emission. This binary only enforces the
// refuse-to-run-as-root guard, then hands mcp-core a `ServerConfig` and the
// `TerminalService`.

use mcp_core::run_simple;
use terminal_mcp::{build_service, server_config};

#[tokio::main]
async fn main() -> mcp_core::Result<()> {
    // Refuse to run as root — all spawned commands would inherit root
    // privileges. Enforced before any CLI parsing or serving.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = std::fs::metadata("/proc/self")
            && meta.uid() == 0
        {
            eprintln!(
                "error: terminal-mcp must not run as root. \
                 All spawned commands would inherit root privileges.\n\
                 Run as an unprivileged user instead."
            );
            std::process::exit(1);
        }
    }

    // Server-level identity, transport policy, and the model-facing
    // `instructions` blurb live in `terminal_mcp::server_config` so they are
    // unit tested; see that function for the rationale behind each setting.
    let config = server_config();

    // The service owns the shared script store and the optional audit logger;
    // `build_service` is the crate's single zero-config construction path (it
    // wires the audit logger from MCP_TERMINAL_LOG_DIR), and it is fallible
    // when that sink is misconfigured, so it is called inside the build closure.
    run_simple(config, || async { Ok(build_service()?) }).await
}
