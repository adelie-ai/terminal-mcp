# Architecture

## Runtime shape

`terminal-mcp` runs as a single process with one MCP server instance:

- `src/main.rs` parses CLI args and selects transport mode.
- `src/server.rs` owns MCP lifecycle state and dispatches tool calls.
- `src/tools.rs` implements the tool registry and tool execution routing.
- `src/operations/execute.rs` performs shell process execution.
- `src/operations/audit.rs` implements optional audit logging.
- `src/transport.rs` handles stdio framing and read/write behavior.

## Request flow

1. Transport receives JSON-RPC message.
2. Main loop parses JSON and calls `handle_jsonrpc_message`.
3. MCP method is validated and dispatched to `McpServer`.
4. Tool calls delegate to `ToolRegistry`.
5. Execution tools call operation functions and return structured tool results.

## State model

- `McpServer` holds:
  - tool registry (`Arc<ToolRegistry>`)
  - initialized flag (`RwLock<bool>`)
  - optional audit logger (`Option<Arc<AuditLogger>>`)
- Dynamic scripts are in-memory and session-scoped.
  - They are not persisted across process restarts.

## Concurrency and behavior

- Command execution uses `tokio::process::Command`.
- stdout/stderr are drained concurrently into bounded tail buffers.
- `max_lines` controls output retention per stream:
  - default `200`
  - `0` means unlimited
- Timeout behavior returns a timed-out result and does not return partial output payloads.
