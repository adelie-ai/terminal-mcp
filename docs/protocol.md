# MCP protocol and transports

## Supported MCP protocol versions

The server accepts these protocol versions during `initialize`:

- `2024-11-05`
- `2025-06-18`
- `2025-11-25`

Any other value is rejected.

## JSON-RPC methods handled

- `initialize`
- `initialized` / `notifications/initialized`
- `tools/list`
- `tools/call`
- `shutdown`

The server enforces initialization before `tools/list`, `tools/call`, and `shutdown`.

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
