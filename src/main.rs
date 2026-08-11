#![deny(warnings)]

// Binary entrypoint for terminal-mcp.
//
// mcp-core owns the CLI (`serve --transport <stdio|websocket|unix>`, with
// `--mode` accepted as a back-compat alias), the JSON-RPC protocol, framing,
// transports, and `tools/list_changed` emission. This binary only enforces the
// refuse-to-run-as-root guard and reports startup failures, then hands mcp-core
// a `ServerConfig` and the `TerminalService`.

use mcp_core::run_simple;
use terminal_mcp::{build_service, server_config};

/// What a startup failure tells the reader to do next.
///
/// The CLI accepts three transports because mcp-core defines three, but this
/// server serves only stdio (see `terminal_mcp::server_config`). A reader who
/// follows an out-of-date document therefore meets a refusal, and the refusal is
/// the only thing that tells them whether the flag was wrong or the document
/// was.
const TRANSPORT_HELP: &str = "terminal-mcp serves the stdio transport only. It is the default, so \
     `terminal-mcp serve` and `terminal-mcp serve --transport stdio` are the same command.\n\
     The websocket and unix transports are refused on purpose. This server runs \
     arbitrary shell commands, and neither listener carries authentication in \
     this build, so either one would give a shell to whoever reaches it. The \
     `--host` and `--port` flags belong to the websocket transport and do \
     nothing here.";

#[tokio::main]
async fn main() {
    // Refuse to run as root - all spawned commands would inherit root
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
    let Err(err) = run_simple(config, || async { Ok(build_service()?) }).await else {
        return;
    };

    // `main() -> Result` would print the `Debug` form here, which shows the enum
    // variant and hides the message the error type wrote for a reader. Print
    // `Display` instead. A `Config` error means the requested run does not match
    // this server's configuration, and for this server that is nearly always the
    // transport, so add what it does serve.
    eprintln!("error: {err}");
    if matches!(err, mcp_core::Error::Config(_)) {
        eprintln!("{TRANSPORT_HELP}");
    }
    std::process::exit(1);
}
