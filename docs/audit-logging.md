# Audit logging

Audit logging is optional and controlled by environment variable.

## Enablement

Set `MCP_TERMINAL_LOG_DIR` to a non-empty directory path.

If unset or empty, audit logging is disabled.

## Files created

Per server session, logs share a prefix:

- `<session_id>_<timestamp>`

Where:

- `session_id` is 8 chars from a UUID v4.
- `timestamp` is UTC format `YYYYMMDDTHHMMSS`.

### Session metadata log

- Filename: `<prefix>_session.log`
- Contains metadata-only entries such as:
  - tool invocations
  - initialize/shutdown lifecycle entries
  - summarized results (`exit_code`, timeout state, tools_changed, log file name)

This file is intended for control-plane visibility, not full output capture.

### Per-command output logs

- Filename: `<prefix>_<NNN>.log`
- Created for command/script executions.
- Contains:
  1. command header (`$ ...`)
  2. optional `(cwd: ...)`
  3. `--- stdout ---`
  4. `--- stderr ---`
  5. `--- exit code: X ---` or `--- timed out ---`

## MCP result correlation

When enabled, execution results include:

- `audit_log_file`

This value is the per-command log filename and can be used to correlate tool results with files on disk.
