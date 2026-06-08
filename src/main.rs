#![deny(warnings)]

// Binary entrypoint for terminal-mcp.
//
// mcp-core owns the CLI (`serve --transport <stdio|websocket|unix>`, with
// `--mode` accepted as a back-compat alias), the JSON-RPC protocol, framing,
// transports, and `tools/list_changed` emission. This binary only enforces the
// refuse-to-run-as-root guard, then hands mcp-core a `ServerConfig` and the
// `TerminalService`.

use mcp_core::{ServerConfig, run_simple};
use terminal_mcp::service::TerminalService;

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

    let config = ServerConfig::new("terminal-mcp", env!("CARGO_PKG_VERSION"))
        // The dynamic script set changes at runtime (terminal_store_script /
        // terminal_remove_script), so advertise listChanged and let mcp-core
        // emit notifications/tools/list_changed after those calls.
        .tools_list_changed(true);

    // The service owns the shared script store and the optional audit logger;
    // building it is fallible (a misconfigured MCP_TERMINAL_LOG_DIR is an
    // error), so it is constructed inside the build closure.
    run_simple(config, || async { Ok(TerminalService::from_env()?) }).await
}
