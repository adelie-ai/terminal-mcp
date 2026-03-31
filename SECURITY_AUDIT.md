# Security Audit — terminal-mcp

**Date:** 2026-03-31
**Scope:** Terminal/shell execution MCP server

---

## Design Note

terminal-mcp executes arbitrary shell commands by design. This is inherently high-risk and is its intended purpose. It assumes a trusted local client. The findings below are defense-in-depth recommendations, not design flaws.

---

## Critical Severity

### 1. No Resource Limits (Fork Bombs, Memory Exhaustion)

**File:** `src/operations/execute.rs:291-329`

Only a timeout is enforced. No limits on:
- Number of child processes
- Memory consumption
- CPU usage
- File descriptors

A fork bomb (`:(){ :|:& };:`) or memory bomb can crash the server and host.

**Recommendation:** Use cgroups or `setrlimit` to restrict spawned processes. Add a configurable `max_processes` and `max_memory_bytes`.

---

### 2. No Privilege Check (Root Execution)

**File:** `src/operations/execute.rs:170-230`

No check prevents the server from running as root. If started as root, all commands inherit root privileges.

**Recommendation:** Add a startup check: refuse to start if `getuid() == 0` unless explicitly overridden with a flag.

---

## High Severity

### 3. WebSocket Binds to 0.0.0.0 by Default

**File:** `src/main.rs:60-61`

Default binding exposes the service on all network interfaces. Any network client can execute arbitrary commands.

**Recommendation:** Change default to `127.0.0.1`. Require an explicit flag to bind to all interfaces.

---

### 4. Unbounded Output Memory

**File:** `src/operations/execute.rs:244-289`

`max_lines` limits line count but not line length or total bytes. A command producing a single multi-GB line exhausts memory.

**Recommendation:** Add `max_output_bytes` with a reasonable default (e.g. 10 MiB).

---

### 5. No Authentication on Tool Calls

**File:** `src/main.rs:275-314`, `src/server.rs`

No authentication mechanism exists. Any connected MCP client can execute commands, store scripts, and list scripts.

**Recommendation:** Add token-based authentication for WebSocket mode. For stdio mode this is acceptable (parent process controls access).

---

## Medium Severity

### 6. Stored Script Recursion

Scripts can call other stored scripts, potentially creating infinite loops that only terminate on timeout.

**Recommendation:** Track call depth and reject execution past a configurable limit.

---

### 7. Audit Log Filename Collision Risk

**File:** `src/operations/audit.rs:32-34`

Session ID is truncated to 8 hex chars (32-bit space). Multiple servers starting in the same second could collide.

**Recommendation:** Use full UUID or add PID to filename.

---

### 8. Output Truncation Hides Data Silently

**File:** `src/operations/execute.rs:301-309`

When `max_lines` is exceeded, `stdout_truncated: true` is set but the total line count and number of dropped lines are not reported.

**Recommendation:** Include `total_lines` and `lines_dropped` in the result.

---

## Positive Findings

- Timeout enforcement on all commands
- Audit logging with session tracking
- Script arguments passed via environment variables (not shell interpolation)
- `Command::new()` used directly (no shell invocation for non-script commands)
- Script execution uses `sh -s` with stdin piping (arguments not in command line)
