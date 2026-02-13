# Tool API

## Built-in tools

The server exposes these built-ins at startup:

- `terminal_execute`
- `terminal_store_script`
- `terminal_remove_script`
- `terminal_list_scripts`

Stored scripts are exposed dynamically as:

- `script_<name>`

## `terminal_execute`

Executes either:

- a direct command (`command`, optional `args`), or
- a shell script (`script`, optional `args` passed as positional params)

### Parameters

- `command: string` (required unless `script` is provided)
- `args: string[]` (optional)
- `script: string` (mutually exclusive with `command` and `stdin`)
- `cwd: string` (optional)
- `timeout_secs: number` (optional, default `30`)
- `stdin: string` (optional, mutually exclusive with `script`)
- `max_lines: number` (optional, default `200`, `0` for unlimited)

### Result payload

`result.content[0].value` includes:

- `exit_code: number`
- `stdout: string`
- `stderr: string`
- `timed_out: boolean`
- `stdout_truncated: boolean`
- `stderr_truncated: boolean`
- `audit_log_file: string` (only when audit logging is enabled)

## `terminal_store_script`

Stores a script in-memory and creates a dynamic tool.

### Parameters

- `name: string`
- `description: string`
- `script: string`
- `parameters: { name, description?, required? }[]` (optional)

### Notes

- Name must be ASCII alphanumeric/underscore.
- Names conflicting with built-ins are rejected.
- Reusing the same name overwrites the existing script.

## `terminal_remove_script`

Removes a stored script and its dynamic tool.

### Parameters

- `name: string`

## `terminal_list_scripts`

Lists stored scripts as JSON records containing:

- `name`
- `description`
- `parameter_count`

## Dynamic tools: `script_<name>`

Calling a dynamic script tool:

- injects declared parameters as environment variables
- supports common execution params: `cwd`, `timeout_secs`, `max_lines`
- returns the same execution result shape as `terminal_execute`
