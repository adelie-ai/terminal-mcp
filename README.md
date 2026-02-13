# terminal-mcp

`terminal-mcp` is a Rust MCP server that exposes shell execution tools to MCP clients (agents, editor integrations, automation runtimes).

It supports both:

- `stdio` transport for local/editor integration.
- `websocket` transport on `/ws` for network clients.

## What the service provides

- MCP JSON-RPC handling for `initialize`, `initialized`, `tools/list`, `tools/call`, and `shutdown`.
- Built-in execution tool: `terminal_execute`.
- Dynamic script tool lifecycle:
	- `terminal_store_script`
	- `terminal_remove_script`
	- `terminal_list_scripts`
	- runtime `script_<name>` tools for stored scripts.
- Structured command results including:
	- `exit_code`
	- `stdout` / `stderr`
	- `timed_out`
	- `stdout_truncated` / `stderr_truncated`.
- Per-call timeout support and working-directory support (`cwd`, including `~` expansion).
- Output tail limiting via `max_lines` (default 200, `0` = unlimited).
- Optional audit logging via `MCP_TERMINAL_LOG_DIR`.

## Audit logging (optional)

Set a non-empty `MCP_TERMINAL_LOG_DIR` to enable logging:

```bash
export MCP_TERMINAL_LOG_DIR=/var/log/terminal-mcp
terminal-mcp serve --mode stdio
```

When enabled:

- Session metadata log: `<session_id>_<timestamp>_session.log`.
- Per-command output log: `<session_id>_<timestamp>_<NNN>.log`.
- Tool results include `audit_log_file` for command/script executions.

If `MCP_TERMINAL_LOG_DIR` is unset or empty, logging is disabled.

## Build and run

```bash
cargo build --release
./target/release/terminal-mcp serve --mode stdio
```

WebSocket mode:

```bash
./target/release/terminal-mcp serve --mode websocket --host 0.0.0.0 --port 8080
```

## Technical documentation

Detailed technical docs are under [docs/README.md](docs/README.md):

- [Architecture](docs/architecture.md)
- [MCP Protocol and Transports](docs/protocol.md)
- [Tool API](docs/tools.md)
- [Audit Logging](docs/audit-logging.md)

Contributor/agent working rules are in [AGENTS.md](AGENTS.md).

## Testing

```bash
cargo test
```

## License

Apache-2.0. See [LICENSE-APACHE](LICENSE-APACHE).
