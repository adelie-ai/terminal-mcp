# MCP protocol and transports

## Supported MCP protocol versions

The protocol layer is provided by [`mcp-core`](https://github.com/adelie-ai/mcp-core),
which negotiates the version during `initialize`. It knows these versions
(newest last):

- `2024-11-05`
- `2025-03-26`
- `2025-06-18`

If the client requests a known version, that version is echoed back; if it
requests an unknown version, mcp-core falls back to the newest known version
(it does not reject the handshake).

## JSON-RPC methods handled

- `initialize`
- `initialized` / `notifications/initialized`
- `ping`
- `tools/list`
- `tools/call`
- `shutdown`

The server enforces initialization before `tools/list`, `tools/call`, and
`shutdown`. (`initialize` also marks the session initialized, so clients that
issue `tools/list` immediately after `initialize` without sending the
`initialized` notification still work.)

## `tools/listChanged` behavior

Server capabilities advertise:

- `capabilities.tools.listChanged = true`

On script add/remove operations, the server emits:

- `notifications/tools/list_changed`

## STDIO transport

`stdio` mode supports both framing styles:

1. Newline-delimited JSON messages.
2. `Content-Length` framed JSON-RPC messages.

Framing is auto-detected from the first incoming message. Responses follow the detected framing mode.

## WebSocket transport

`websocket` mode binds to `host:port` and exposes one endpoint:

- `GET /ws`

Text frames are parsed as JSON-RPC requests. Responses and notifications are sent as text frames.
