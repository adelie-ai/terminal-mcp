# terminal-mcp

A small, fast, and modular Rust **MCP server** (plus library) that exposes shell command execution over a simple IPC/RPC transport. It is primarily intended for use by **LLM agents** and other automated clients that need the ability to execute shell commands and receive structured output.

## Who this is for

- **LLM agent runtimes** that need tool-driven command execution (run programs, capture output).
- **Automation frameworks** that prefer a single, auditable service boundary for shell access.
- **Editors/CI/sandboxes** that want to delegate command execution to a controlled server.

## Key capabilities

- **Structured output**: Returns exit code, stdout, stderr, and timeout status as structured JSON.
- **Timeout control**: Configurable per-command timeout with automatic process cleanup.
- **Stdin piping**: Send input to command stdin for interactive-style workflows.
- **Working directory**: Execute commands in any directory with ~ expansion support.

## What it is

`terminal-mcp` is both a library and a small **MCP server/CLI**. It implements shell command execution as an MCP tool over IPC/RPC. The primary use case is **LLM agents** that need reliable command execution.

Key components:

- `src/main.rs` - CLI entrypoint / server runner (binary `terminal-mcp`).
- `src/server.rs` - Server orchestration and request handling.
- `src/transport.rs` - Abstractions for the transport mechanism used to accept client requests.
- `src/lib.rs` - Library interface and shared types.
- `src/operations/` - Operation implementations (execute).
- `src/error.rs` - Centralized error types and conversion utilities.

## How it works (high level)

1. A client sends a request over the configured transport to execute a command.
2. The server receives the request and dispatches it to the execute operation handler.
3. The handler spawns the process, captures output, and returns a structured response.
4. The transport layer serializes the response back to the client.

## Build & run

Build the project (requires Rust toolchain):

```bash
cargo build --release
```

Run the server binary:

```bash
# STDIO mode (recommended for local usage)
./target/release/terminal-mcp serve --mode stdio

# WebSocket mode
./target/release/terminal-mcp serve --mode websocket --host 0.0.0.0 --port 8080
```

## Testing

Run the test suite with `cargo test`.

## Contributing

Contributions are welcome. Please follow the repository coding style and include tests for new operations or behavior changes.

## License

This project uses the Apache license. See LICENSE-APACHE for details.
