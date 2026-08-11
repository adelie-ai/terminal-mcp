# Security Audit - terminal-mcp

**Date:** 2026-03-31
**Scope:** Terminal/shell execution MCP server

---

## Design Note

terminal-mcp executes arbitrary shell commands by design. This is inherently high-risk and is its intended purpose. It assumes a trusted local client.

---

## Medium Severity

### 1. No Per-Process Resource Limits (DOWNGRADED - MEDIUM)

**File:** `src/operations/execute.rs`

**Status:** Accepted risk (2026-03-31)
**Rationale:** Timeouts kill runaway processes, output is capped at 10 MiB, and the server refuses to run as root. The remaining gap is cgroup/rlimit enforcement against fork bombs or memory exhaustion in child processes. Adding `setrlimit` would require `Command::pre_exec()` (unsafe) and `rlimit` is not exposed by tokio's `Command`. Cgroup delegation requires systemd or root setup. A fork bomb can still hurt the host, but only with the current user's privileges and only until the timeout fires. Since the server assumes a trusted local client, this is acceptable.

**Recommendation (defense-in-depth):** Run terminal-mcp under a systemd slice with `MemoryMax=`, `TasksMax=`, and `CPUQuota=` to restrict all spawned processes at the cgroup level.

---

## High Severity

### 2. No Authentication on Tool Calls (CLOSED - was HIGH)

**File:** `src/main.rs`, `src/lib.rs`, `Cargo.toml` (protocol/transport now in `mcp-core`)

**Status:** Closed 2026-08-11 (acknowledged 2026-03-31; revisited 2026-06-08
after the mcp-core migration). The finding rested on the WebSocket transport,
which this server stopped serving on 2026-06-09 (MF-12), one day after that
revisit.
**Rationale:** WebSocket mode had no authentication. It was mitigated by
defaulting to localhost (`--host 127.0.0.1`, via mcp-core's `CommonServeArgs`).
The server now serves the `stdio` transport and no other: `Cargo.toml` does not
enable mcp-core's `websocket` feature, and `server_config()` leaves both the
websocket and the unix transports out of the server's transport policy. `serve
--transport websocket` exits non-zero instead of binding a listener, which
`tests/transport_refusals.rs` holds. No unauthenticated network path is left.
Stdio is unaffected - the parent process starts the server and controls access
to it.

**Recommendation:** Design authentication before any later change enables a
network transport.

---

## Medium Severity

### 3. Stored Script Recursion (MEDIUM)

Scripts can call other stored scripts, potentially creating infinite loops that only terminate on timeout.

**Recommendation:** Track call depth and reject execution past a configurable limit.

---

### 4. Output Truncation Hides Data Silently (MEDIUM)

When `max_lines` is exceeded, `stdout_truncated: true` is set but total line count and dropped lines are not reported.

**Recommendation:** Include `total_lines` and `lines_dropped` in the result.

---

## Positive Findings

- Timeout enforcement on all commands
- Audit logging with session tracking
- Script arguments passed via environment variables (not shell interpolation)
- `Command::new()` used directly (no shell invocation for non-script commands)
- Output byte cap prevents memory exhaustion from long lines
