# Security Audit — terminal-mcp

**Date:** 2026-03-31
**Scope:** Terminal/shell execution MCP server

---

## Design Note

terminal-mcp executes arbitrary shell commands by design. This is inherently high-risk and is its intended purpose. It assumes a trusted local client.

---

## Critical Severity

### 1. No Resource Limits (Fork Bombs, Memory Exhaustion) (CRITICAL)

**File:** `src/operations/execute.rs`

Only a timeout and output byte cap are enforced. No limits on child process count, CPU usage, or file descriptors. A fork bomb can crash the host.

**Recommendation:** Use cgroups or `setrlimit` to restrict spawned processes.

---

## High Severity

### 2. No Authentication on Tool Calls (HIGH)

**File:** `src/main.rs`, `src/server.rs`

No authentication mechanism exists for WebSocket mode. Any connected client can execute commands.

**Recommendation:** Add token-based authentication for WebSocket mode. For stdio mode this is acceptable (parent process controls access).

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
