# Architecture

## Runtime shape

`terminal-mcp` runs as a single process. The JSON-RPC protocol, message
framing, transports, version negotiation, and `tools/list_changed` emission are
owned by the shared [`mcp-core`](https://github.com/adelie-ai/mcp-core) crate.
mcp-core implements three transports; this server enables only `stdio` and
refuses the other two (see [protocol.md](protocol.md)). This crate supplies only
the domain and an `mcp_core::McpService` adapter:

- `src/main.rs` enforces the refuse-to-run-as-root guard, then hands mcp-core a
  `ServerConfig` and the service via `mcp_core::run_simple` (mcp-core owns the
  `serve` CLI, including `--transport`/`--mode`). It also reports a startup
  failure: it prints the error, adds the transport this server serves when the
  error is a configuration error, and exits non-zero.
- `src/service.rs` implements `mcp_core::McpService` (`TerminalService`): it
  advertises the tool set, dispatches calls to the registry, maps domain errors
  onto `mcp_core::CallError`, and does session-level audit logging.
- `src/tools.rs` implements the tool registry (`ToolRegistry`) and tool
  execution routing, including the dynamic `script_<name>` tools.
- `src/operations/execute.rs` performs shell process execution.
- `src/operations/audit.rs` implements optional audit logging.
- `src/error.rs` holds the crate's domain errors (shell / script / tool-param).

## Request flow

1. mcp-core's transport receives and frames a JSON-RPC message.
2. mcp-core dispatches `initialize` / `tools/list` / `tools/call` / `shutdown`.
3. `tools/list` calls `TerminalService::tools()` -> `ToolRegistry::list_tools()`
   (built-ins plus one `script_<name>` per stored script).
4. `tools/call` calls `TerminalService::call_tool()` -> `ToolRegistry`.
5. Execution tools call operation functions and return a `ToolReply`.
6. After a successful `terminal_store_script` / `terminal_remove_script`, the
   reply is marked `tools_changed()` and mcp-core emits
   `notifications/tools/list_changed`.

## State model

- `TerminalService` holds:
  - tool registry (`Arc<ToolRegistry>`)
  - optional audit logger (`Option<Arc<AuditLogger>>`)
  - (the MCP `initialized` handshake state lives per-connection inside mcp-core)
- `ToolRegistry` holds the dynamic script store behind a `std::sync::RwLock`
  (its critical sections never span an `.await`, so `tools()` reads it directly).
- Dynamic scripts are in-memory and session-scoped.
  - They are not persisted across process restarts.

## Concurrency and behavior

- Command execution uses `tokio::process::Command`.
- stdout/stderr are drained concurrently into bounded tail buffers.
- `max_lines` controls output retention per stream:
  - default `200`
  - `0` means unlimited
- Timeout behavior returns a timed-out result and does not return partial output payloads.
