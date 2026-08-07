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

## Logging

Traces, metrics and logs come from `mcp-core`, which installs the subscriber
and holds the guard: this server calls nothing to turn logging on. Full
mechanics (subscriber setup, the metrics facade, span-close events, shutdown
timing) are documented once in the [mcp-core
README](https://github.com/adelie-ai/mcp-core#logging); this section covers
what is specific to `terminal-mcp`.

### Where it goes

**stderr, always.** The stdio transport frames JSON-RPC on stdout, so a log
line there would corrupt the protocol. This holds at every level, including
`RUST_LOG=trace`.

```bash
RUST_LOG=debug terminal-mcp serve --transport stdio
```

### The level contract, and why it matters more here

| Level | Carries |
|---|---|
| INFO | ids, counts, durations, model names, tool names. **Never content.** |
| DEBUG | tool arguments, and the reason a tool declined or failed. |

This server runs arbitrary shell commands, so its tool arguments are the
highest-value content in the fleet: paths, flags, and sometimes secrets. A
command line, its arguments, its stdout, its stderr, and the working
directory it ran in never reach a span field or an INFO line, at any log
level. `RUST_LOG=debug` is what it takes to see the assembled tool arguments
(via mcp-core's own dispatch layer, sanitised and size-capped) -- and that is
deliberate, not this server's addition.

### What this server emits

mcp-core's dispatch layer already covers the JSON-RPC request and the tool
call: `mcp.request` and `mcp.tools.call` spans, and the `mcp.requests` /
`mcp.tools.call` / `mcp.tools.call.duration` metrics, all keyed by method or
tool name, never by argument content.

On top of that, terminal-mcp's own execution path (shared by `command`,
`script`, and every stored `script_<name>` tool) adds:

- A `terminal.execute` span around each spawn, carrying only `detach` and
  `timeout_secs` -- never the command line.
- A `terminal.execute` counter, labelled `outcome`: `ok`, `nonzero_exit`,
  `timeout`, `detached`, or `error`. A nonzero shell exit is domain
  information mcp-core's own protocol-level outcome cannot see, since the
  JSON-RPC call itself still succeeds.
- A `terminal.execute.duration` histogram.

### Exporting to a collector

Off by default (`otel` feature, see `Cargo.toml`). With it off, no
opentelemetry crate is resolved at all. With it on, configure export with the
standard `OTEL_EXPORTER_OTLP_*` environment variables -- there are no CLI
flags and no server-specific variables. See the [mcp-core
README](https://github.com/adelie-ai/mcp-core#exporting-to-a-collector) for
the full variable reference.

```bash
cargo build --release --features otel
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf \
  ./target/release/terminal-mcp serve --transport stdio
```

With no collector configured, the periodic metrics summary still writes to
stderr, so a default-feature install from `cargo install` gets real numbers
in the journal.

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
