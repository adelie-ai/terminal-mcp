#![deny(warnings)]

// Shell command execution

use crate::error::{Result, ShellError};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use tokio::process::Command;

#[derive(Debug, Serialize)]
pub struct ExecuteResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    /// PID of a detached (fire-and-forget) process. `None` for bounded execs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detached_pid: Option<u32>,
}

/// A bounded ring buffer that keeps only the last `capacity` lines.
/// When capacity is 0 (unlimited), stores all lines in a plain Vec.
/// In both modes, total stored bytes are capped at `MAX_BUFFER_BYTES`
/// to prevent memory exhaustion from extremely long lines.
struct TailBuffer {
    /// Ring storage used when capacity > 0.
    ring: Vec<String>,
    /// Monotonic count of lines pushed.
    total_lines: usize,
    /// Max lines to keep (0 = unlimited).
    capacity: usize,
    /// Unbounded storage used when capacity == 0.
    all: Vec<String>,
    /// Whether the last line we saw had a trailing newline.
    trailing_newline: bool,
    /// Approximate number of bytes stored (for the byte cap).
    stored_bytes: usize,
}

impl TailBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            ring: if capacity > 0 {
                Vec::with_capacity(capacity)
            } else {
                Vec::new()
            },
            total_lines: 0,
            capacity,
            all: Vec::new(),
            trailing_newline: false,
            stored_bytes: 0,
        }
    }

    fn push(&mut self, line: &str, had_newline: bool) {
        self.total_lines += 1;
        self.trailing_newline = had_newline;
        // Cap individual lines to prevent a single multi-GB line from
        // exhausting memory.  Truncate to 1 MiB if necessary.
        let line = if line.len() > 1_048_576 {
            &line[..1_048_576]
        } else {
            line
        };
        // Drop lines once the byte budget is exhausted (in unlimited mode).
        if self.capacity == 0 && self.stored_bytes + line.len() > MAX_BUFFER_BYTES {
            return;
        }
        let line = line.to_string();
        self.stored_bytes += line.len();
        if self.capacity == 0 {
            self.all.push(line);
        } else if self.ring.len() < self.capacity {
            self.ring.push(line);
        } else {
            let idx = (self.total_lines - 1) % self.capacity;
            // Subtract evicted line bytes before replacing.
            self.stored_bytes = self.stored_bytes.saturating_sub(self.ring[idx].len());
            self.ring[idx] = line;
        }
    }

    fn finish(self) -> (String, bool) {
        let truncated = self.capacity > 0 && self.total_lines > self.capacity;

        let lines = if self.capacity == 0 {
            self.all
        } else {
            let len = self.ring.len();
            if len == 0 {
                return (String::new(), false);
            }
            if self.total_lines <= self.capacity {
                self.ring
            } else {
                let start = self.total_lines % self.capacity;
                let mut ordered = Vec::with_capacity(len);
                for i in 0..len {
                    ordered.push(self.ring[(start + i) % self.capacity].clone());
                }
                ordered
            }
        };

        if lines.is_empty() {
            return (String::new(), false);
        }

        let mut text = lines.join("\n");
        if self.trailing_newline {
            text.push('\n');
        }
        (text, truncated)
    }
}

/// Default maximum lines returned for stdout/stderr.
pub const DEFAULT_MAX_LINES: usize = 200;

/// Maximum total bytes stored in a TailBuffer before further lines are dropped.
/// This guards against single extremely long lines exhausting memory.
const MAX_BUFFER_BYTES: usize = 10 * 1024 * 1024; // 10 MiB

/// Environment variables preserved when scrubbing the inherited environment.
///
/// Why: foreground execs would otherwise inherit the server's full environment,
/// leaking secrets (e.g. `ANTHROPIC_API_KEY`, `AWS_*`) into every spawned
/// command. We keep only what a normal shell session needs to function.
const SAFE_ENV_KEYS: &[&str] = &["PATH", "HOME", "USER", "TMPDIR", "TERM", "LANG"];

/// Grace period between SIGTERM and SIGKILL when terminating a timed-out group.
#[cfg(unix)]
const TERM_GRACE: std::time::Duration = std::time::Duration::from_millis(200);

/// Apply environment scrubbing to a command unless the operator opts back in to
/// full inheritance via `MCP_TERMINAL_INHERIT_ENV=1`.
///
/// When scrubbing, the command starts from an empty environment plus
/// [`SAFE_ENV_KEYS`]; caller-supplied `env_vars` are layered on afterwards by
/// the caller, so they always win.
fn apply_env_scrubbing(cmd: &mut Command) {
    let inherit = std::env::var("MCP_TERMINAL_INHERIT_ENV")
        .map(|v| v == "1")
        .unwrap_or(false);
    if inherit {
        return;
    }
    cmd.env_clear();
    for key in SAFE_ENV_KEYS {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }
}

/// Place the child in its own process group / session so the whole tree can be
/// signalled (and detached children survive) independently of the server.
///
/// Why: on timeout we must kill the entire group, not just the direct child,
/// or grandchildren (e.g. a `sleep` spawned by a script) keep running. For
/// detach mode, a fresh session disowns the child entirely.
#[cfg(unix)]
fn set_new_session(cmd: &mut Command) {
    // SAFETY: `setsid` is async-signal-safe and is the canonical way to create
    // a new session/process group between fork and exec. It touches no shared
    // process state beyond the child's own session id.
    unsafe {
        cmd.pre_exec(|| {
            // Detach into a new session; the child becomes its own group leader,
            // so its pid doubles as the process-group id (pgid).
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn set_new_session(_cmd: &mut Command) {}

/// Send `signal` to the process group led by `pid` (its pgid == pid because the
/// child is a session/group leader). Best-effort; ignores ESRCH.
#[cfg(unix)]
fn signal_group(pid: u32, signal: libc::c_int) {
    // SAFETY: `killpg` only delivers a signal to the target process group and
    // has no other side effects. A negative/zero pid is never passed here.
    unsafe {
        libc::killpg(pid as libc::pid_t, signal);
    }
}

/// Execute a command directly, optionally with args/stdin/cwd/timeout/max_lines.
///
/// When `detach` is true the process is launched fire-and-forget: a new session
/// is created, no timeout is enforced, no output is captured, and the call
/// returns promptly with the child's pid in [`ExecuteResult::detached_pid`].
#[allow(clippy::too_many_arguments)]
pub async fn execute(
    command: &str,
    args: Option<&[String]>,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
    stdin_input: Option<&str>,
    max_lines: Option<usize>,
    detach: bool,
) -> Result<ExecuteResult> {
    execute_inner(
        command,
        cwd,
        timeout_secs,
        max_lines,
        detach,
        ExecuteMode::Command { args, stdin_input },
    )
    .await
}

/// Execute a shell script by piping it into `sh -s -- [args]`.
///
/// See [`execute`] for `detach` semantics.
#[allow(clippy::too_many_arguments)]
pub async fn execute_script(
    script: &str,
    args: Option<&[String]>,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
    max_lines: Option<usize>,
    env_vars: Option<&HashMap<String, String>>,
    detach: bool,
) -> Result<ExecuteResult> {
    execute_inner(
        "sh",
        cwd,
        timeout_secs,
        max_lines,
        detach,
        ExecuteMode::Script {
            script,
            script_args: args,
            env_vars,
        },
    )
    .await
}

enum ExecuteMode<'a> {
    Command {
        args: Option<&'a [String]>,
        stdin_input: Option<&'a str>,
    },
    Script {
        script: &'a str,
        script_args: Option<&'a [String]>,
        env_vars: Option<&'a HashMap<String, String>>,
    },
}

/// Inner execution function shared by direct command and script execution paths.
async fn execute_inner(
    command: &str,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
    max_lines: Option<usize>,
    detach: bool,
    mode: ExecuteMode<'_>,
) -> Result<ExecuteResult> {
    if command.is_empty() {
        return Err(ShellError::InvalidCommand("Command cannot be empty".to_string()).into());
    }

    check_command_allowed(command)?;

    let timeout_secs = timeout_secs.unwrap_or(30);

    let mut cmd = Command::new(command);

    // Scrub inherited environment before layering caller-supplied vars so the
    // caller's values always win and secrets are never leaked by default.
    apply_env_scrubbing(&mut cmd);

    let (command_args, env_vars, stdin_input) = match mode {
        ExecuteMode::Command { args, stdin_input } => (args, None, stdin_input),
        ExecuteMode::Script {
            script,
            script_args,
            env_vars,
        } => {
            cmd.arg("-s");
            cmd.arg("--");
            if let Some(extra) = script_args {
                cmd.args(extra);
            }
            (None, env_vars, Some(script))
        }
    };

    if let Some(args) = command_args {
        cmd.args(args);
    }

    if let Some(vars) = env_vars {
        for (k, v) in vars {
            cmd.env(k, v);
        }
    }

    if let Some(cwd) = cwd {
        let cwd_path = resolve_cwd(cwd)?;
        cmd.current_dir(cwd_path);
    }

    // Place the child in its own session/process group so we can terminate the
    // whole tree on timeout (bounded mode) or fully disown it (detach mode).
    set_new_session(&mut cmd);

    if detach {
        return spawn_detached(&mut cmd, command);
    }

    if stdin_input.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    } else {
        cmd.stdin(std::process::Stdio::null());
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return Err(ShellError::CommandNotFound(command.to_string()).into());
            }
            return Err(ShellError::ExecutionFailed(format!(
                "Failed to spawn command '{}': {}",
                command, e
            ))
            .into());
        }
    };

    // PID of the child, which is also the pgid of its process group (it is the
    // group leader thanks to `setsid`). Needed to terminate the whole tree on
    // timeout instead of leaking grandchildren.
    let child_pid = child.id();

    // Write stdin if provided
    if let Some(input) = stdin_input
        && let Some(mut child_stdin) = child.stdin.take()
    {
        use tokio::io::AsyncWriteExt;
        let _ = child_stdin.write_all(input.as_bytes()).await;
        drop(child_stdin);
    }

    // Take pipes out of child before spawning concurrent readers to avoid deadlock
    // when pipe buffers fill up. Each reader drains lines into a TailBuffer that
    // sheds old lines as it goes, bounding memory usage.
    use tokio::io::AsyncBufReadExt;

    let max_lines = max_lines.unwrap_or(DEFAULT_MAX_LINES);

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_max = max_lines;
    let stdout_handle = tokio::spawn(async move {
        let mut buf = TailBuffer::new(stdout_max);
        if let Some(out) = stdout_pipe {
            let mut reader = tokio::io::BufReader::new(out);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let had_newline = line.ends_with('\n');
                        buf.push(line.trim_end_matches('\n'), had_newline);
                    }
                }
            }
        }
        buf.finish()
    });

    let stderr_max = max_lines;
    let stderr_handle = tokio::spawn(async move {
        let mut buf = TailBuffer::new(stderr_max);
        if let Some(err) = stderr_pipe {
            let mut reader = tokio::io::BufReader::new(err);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let had_newline = line.ends_with('\n');
                        buf.push(line.trim_end_matches('\n'), had_newline);
                    }
                }
            }
        }
        buf.finish()
    });

    let timeout_duration = std::time::Duration::from_secs(timeout_secs);
    // Keep `child` owned by this function (not moved into the timeout future) so
    // that on timeout we can still kill its process group.
    let wait_result = tokio::time::timeout(timeout_duration, child.wait()).await;

    match wait_result {
        Ok(status) => {
            // Process exited on its own; collect the buffered output.
            let (stdout, stdout_truncated) = stdout_handle.await.unwrap_or_default();
            let (stderr, stderr_truncated) = stderr_handle.await.unwrap_or_default();
            match status {
                Ok(status) => Ok(ExecuteResult {
                    exit_code: status.code().unwrap_or(-1),
                    stdout,
                    stderr,
                    timed_out: false,
                    stdout_truncated,
                    stderr_truncated,
                    detached_pid: None,
                }),
                Err(e) => Err(ShellError::ExecutionFailed(format!(
                    "Failed to wait for command '{}': {}",
                    command, e
                ))
                .into()),
            }
        }
        Err(_) => {
            // Timeout: terminate the entire process group (SIGTERM, grace,
            // SIGKILL) so the child and any grandchildren are actually killed
            // rather than leaked.
            terminate_group(child_pid, &mut child).await;
            // Abort the reader tasks; their pipes are now closing.
            stdout_handle.abort();
            stderr_handle.abort();
            Ok(ExecuteResult {
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("Command timed out after {} seconds", timeout_secs),
                timed_out: true,
                stdout_truncated: false,
                stderr_truncated: false,
                detached_pid: None,
            })
        }
    }
}

/// Spawn a fire-and-forget process: new session, no timeout, no captured
/// output. Returns immediately with the child's pid.
fn spawn_detached(cmd: &mut Command, command: &str) -> Result<ExecuteResult> {
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());

    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return Err(ShellError::CommandNotFound(command.to_string()).into());
            }
            return Err(ShellError::ExecutionFailed(format!(
                "Failed to spawn command '{}': {}",
                command, e
            ))
            .into());
        }
    };

    let pid = child.id();
    // Drop the handle without killing: the child is in its own session and is
    // intentionally disowned. tokio's `Child` does not kill on drop by default.
    drop(child);

    Ok(ExecuteResult {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        stdout_truncated: false,
        stderr_truncated: false,
        detached_pid: pid,
    })
}

/// Terminate the timed-out child's process group, then reap the direct child.
///
/// On Unix we signal the whole group (SIGTERM, brief grace, SIGKILL). On other
/// platforms we fall back to killing the direct child.
async fn terminate_group(child_pid: Option<u32>, child: &mut tokio::process::Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child_pid {
            signal_group(pid, libc::SIGTERM);
            tokio::time::sleep(TERM_GRACE).await;
            signal_group(pid, libc::SIGKILL);
        } else {
            let _ = child.start_kill();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child_pid;
        let _ = child.start_kill();
    }
    // Reap the direct child so it does not linger as a zombie.
    let _ = child.wait().await;
}

/// Resolve and validate a working directory: expand `~`, canonicalize, and
/// reject pseudo-filesystem roots that should never be a working directory.
fn resolve_cwd(cwd: &str) -> Result<std::path::PathBuf> {
    let expanded = shellexpand::tilde(cwd);
    let cwd_path = Path::new(expanded.as_ref());
    if !cwd_path.exists() {
        return Err(ShellError::ExecutionFailed(format!(
            "Working directory does not exist: {}",
            cwd
        ))
        .into());
    }
    let canonical = cwd_path.canonicalize().map_err(|e| {
        ShellError::ExecutionFailed(format!(
            "Failed to resolve working directory '{}': {}",
            cwd, e
        ))
    })?;
    for forbidden in ["/proc", "/sys", "/dev"] {
        if canonical == Path::new(forbidden) || canonical.starts_with(format!("{}/", forbidden)) {
            return Err(ShellError::ExecutionFailed(format!(
                "Refusing to use working directory under {}: {}",
                forbidden, cwd
            ))
            .into());
        }
    }
    Ok(canonical)
}

/// Enforce the optional command allowlist configured via
/// `MCP_TERMINAL_ALLOWED_COMMANDS` (comma-separated). Unrestricted by default.
fn check_command_allowed(command: &str) -> Result<()> {
    let Ok(raw) = std::env::var("MCP_TERMINAL_ALLOWED_COMMANDS") else {
        return Ok(());
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(());
    }
    // Match on the command's basename so absolute paths are handled too.
    let basename = Path::new(command)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(command);
    let allowed = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .any(|c| c == command || c == basename);
    if allowed {
        Ok(())
    } else {
        Err(ShellError::InvalidCommand(format!(
            "Command '{}' is not in MCP_TERMINAL_ALLOWED_COMMANDS",
            command
        ))
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_execution() {
        let result = execute(
            "echo",
            Some(&["hello".to_string()]),
            None,
            None,
            None,
            None,
            false,
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "hello");
        assert!(result.stderr.is_empty());
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn test_stderr_capture() {
        let result = execute(
            "sh",
            Some(&["-c".to_string(), "echo err >&2".to_string()]),
            None,
            None,
            None,
            None,
            false,
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.is_empty());
        assert_eq!(result.stderr.trim(), "err");
    }

    #[tokio::test]
    async fn test_non_zero_exit_code() {
        let result = execute("false", None, None, None, None, None, false)
            .await
            .unwrap();
        assert_ne!(result.exit_code, 0);
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn test_timeout() {
        let result = execute(
            "sleep",
            Some(&["10".to_string()]),
            None,
            Some(1),
            None,
            None,
            false,
        )
        .await
        .unwrap();
        assert!(result.timed_out);
        assert_eq!(result.exit_code, -1);
    }

    /// Regression: a bounded exec that times out must leave NO surviving
    /// process — neither the direct child nor any grandchildren.
    #[tokio::test]
    async fn test_timeout_kills_process_tree() {
        // The script spawns a grandchild `sleep` and writes its PID to a file,
        // then sleeps itself. Both must be dead after the timeout fires.
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("grandchild.pid");
        let pid_file_str = pid_file.to_str().unwrap().to_string();
        let script = format!("sleep 30 & echo $! > {pid_file_str}; sleep 30");

        let result = execute_script(&script, None, None, Some(1), None, None, false)
            .await
            .unwrap();
        assert!(result.timed_out);

        // Give the kernel a moment to deliver SIGKILL and reap.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let pid_text = std::fs::read_to_string(&pid_file).expect("grandchild pid recorded");
        let pid: i32 = pid_text.trim().parse().expect("parse pid");

        // kill -0 returns an error (ESRCH) once the process is gone.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(
            !alive,
            "grandchild pid {pid} survived the timeout — process tree leaked"
        );
    }

    #[tokio::test]
    async fn test_detach_returns_pid_and_survives() {
        // Detached process should return promptly with a pid and keep running.
        let result = execute(
            "sleep",
            Some(&["30".to_string()]),
            None,
            // A tiny timeout to prove it is ignored in detach mode.
            Some(1),
            None,
            None,
            true,
        )
        .await
        .unwrap();
        assert!(!result.timed_out);
        let pid = result.detached_pid.expect("detached pid") as i32;

        // It should be alive right after returning.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(alive, "detached process should be running");

        // Clean up: kill the detached process group.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }

    #[tokio::test]
    async fn test_custom_cwd() {
        let result = execute("pwd", None, Some("/tmp"), None, None, None, false)
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("tmp"));
    }

    #[tokio::test]
    async fn test_cwd_rejects_proc() {
        let result = execute("pwd", None, Some("/proc"), None, None, None, false).await;
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("/proc"),
            "expected /proc rejection, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_command_not_found() {
        let result = execute(
            "nonexistent_command_xyz_12345",
            None,
            None,
            None,
            None,
            None,
            false,
        )
        .await;
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("not found") || err.contains("Command not found"));
    }

    #[tokio::test]
    async fn test_empty_command() {
        let result = execute("", None, None, None, None, None, false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_stdin_piping() {
        let result = execute(
            "cat",
            None,
            None,
            None,
            Some("hello from stdin"),
            None,
            false,
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "hello from stdin");
    }

    #[tokio::test]
    async fn test_invalid_cwd() {
        let result = execute(
            "echo",
            Some(&["hi".to_string()]),
            Some("/nonexistent_dir_xyz"),
            None,
            None,
            None,
            false,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_max_lines_truncation() {
        // Generate 10 lines, keep last 3
        let result = execute(
            "sh",
            Some(&[
                "-c".to_string(),
                "for i in $(seq 1 10); do echo line$i; done".to_string(),
            ]),
            None,
            None,
            None,
            Some(3),
            false,
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout_truncated);
        let lines: Vec<&str> = result.stdout.trim().lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "line8");
        assert_eq!(lines[2], "line10");
    }

    #[tokio::test]
    async fn test_max_lines_no_truncation_when_under() {
        let result = execute(
            "echo",
            Some(&["hello".to_string()]),
            None,
            None,
            None,
            Some(5),
            false,
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(!result.stdout_truncated);
        assert_eq!(result.stdout.trim(), "hello");
    }

    #[tokio::test]
    async fn test_execute_script_basic() {
        let result = execute_script(
            "echo hello\necho world",
            None,
            None,
            None,
            None,
            None,
            false,
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 0);
        let lines: Vec<&str> = result.stdout.trim().lines().collect();
        assert_eq!(lines, vec!["hello", "world"]);
    }

    #[tokio::test]
    async fn test_execute_script_with_args() {
        let result = execute_script(
            "echo \"arg1=$1 arg2=$2\"",
            Some(&["foo".to_string(), "bar".to_string()]),
            None,
            None,
            None,
            None,
            false,
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "arg1=foo arg2=bar");
    }

    #[tokio::test]
    async fn test_execute_script_with_env_vars() {
        let mut env = HashMap::new();
        env.insert("MY_VAR".to_string(), "hello_env".to_string());
        let result = execute_script("echo $MY_VAR", None, None, None, None, Some(&env), false)
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "hello_env");
    }

    /// MF-11: a single line longer than the 1 MiB per-line cap whose byte
    /// 1_048_576 falls in the middle of a multibyte character must be
    /// truncated at a char boundary, not panic. '€' is 3 bytes and
    /// 1_048_576 % 3 == 1, so the naive `&line[..1_048_576]` slice lands
    /// mid-character.
    #[test]
    fn test_overlong_multibyte_line_truncated_at_char_boundary() {
        let line = "€".repeat(350_000); // 1_050_000 bytes, > 1 MiB
        let mut buf = TailBuffer::new(0);
        buf.push(&line, true);
        let (out, _truncated) = buf.finish();
        assert!(out.len() <= 1_048_576, "line must be capped to 1 MiB");
        assert!(!out.is_empty(), "truncation must not drop the line");
        assert!(
            out.chars().all(|c| c == '€'),
            "truncation must not produce partial characters"
        );
    }

    /// MF-11 companion: the same cap in ring-buffer (capacity > 0) mode.
    #[test]
    fn test_overlong_multibyte_line_truncated_in_ring_mode() {
        let line = "€".repeat(350_000);
        let mut buf = TailBuffer::new(2);
        buf.push("short", true);
        buf.push(&line, true);
        let (out, _truncated) = buf.finish();
        assert!(out.starts_with("short\n"));
        assert!(out.len() <= 1_048_576 + "short\n".len());
    }

    #[tokio::test]
    async fn test_max_lines_zero_means_unlimited() {
        let result = execute(
            "sh",
            Some(&[
                "-c".to_string(),
                "for i in $(seq 1 10); do echo line$i; done".to_string(),
            ]),
            None,
            None,
            None,
            Some(0),
            false,
        )
        .await
        .unwrap();
        assert!(!result.stdout_truncated);
        let lines: Vec<&str> = result.stdout.trim().lines().collect();
        assert_eq!(lines.len(), 10);
    }
}
