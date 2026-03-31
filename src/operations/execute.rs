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

/// Execute a command directly, optionally with args/stdin/cwd/timeout/max_lines.
pub async fn execute(
    command: &str,
    args: Option<&[String]>,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
    stdin_input: Option<&str>,
    max_lines: Option<usize>,
) -> Result<ExecuteResult> {
    execute_inner(
        command,
        cwd,
        timeout_secs,
        max_lines,
        ExecuteMode::Command { args, stdin_input },
    )
    .await
}

/// Execute a shell script by piping it into `sh -s -- [args]`.
pub async fn execute_script(
    script: &str,
    args: Option<&[String]>,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
    max_lines: Option<usize>,
    env_vars: Option<&HashMap<String, String>>,
) -> Result<ExecuteResult> {
    execute_inner(
        "sh",
        cwd,
        timeout_secs,
        max_lines,
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
    mode: ExecuteMode<'_>,
) -> Result<ExecuteResult> {
    if command.is_empty() {
        return Err(ShellError::InvalidCommand("Command cannot be empty".to_string()).into());
    }

    let timeout_secs = timeout_secs.unwrap_or(30);

    let mut cmd = Command::new(command);

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
        let expanded = shellexpand::tilde(cwd);
        let cwd_path = Path::new(expanded.as_ref());
        if !cwd_path.exists() {
            return Err(ShellError::ExecutionFailed(format!(
                "Working directory does not exist: {}",
                cwd
            ))
            .into());
        }
        cmd.current_dir(cwd_path);
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

    // Write stdin if provided
    if let Some(input) = stdin_input
        && let Some(mut child_stdin) = child.stdin.take() {
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
    let result = tokio::time::timeout(timeout_duration, async {
        let stdout_result = stdout_handle.await.unwrap_or_default();
        let stderr_result = stderr_handle.await.unwrap_or_default();
        let status = child.wait().await;
        (stdout_result, stderr_result, status)
    })
    .await;

    match result {
        Ok(((stdout, stdout_truncated), (stderr, stderr_truncated), Ok(status))) => {
            Ok(ExecuteResult {
                exit_code: status.code().unwrap_or(-1),
                stdout,
                stderr,
                timed_out: false,
                stdout_truncated,
                stderr_truncated,
            })
        }
        Ok((_, _, Err(e))) => Err(ShellError::ExecutionFailed(format!(
            "Failed to wait for command '{}': {}",
            command, e
        ))
        .into()),
        Err(_) => {
            // Timeout - tasks will be dropped, killing pipe reads.
            // We can't easily kill the child here since it moved into the task,
            // but dropping the task will drop the Child, closing pipes.
            Ok(ExecuteResult {
                exit_code: -1,
                stdout: String::new(),
                stderr: format!("Command timed out after {} seconds", timeout_secs),
                timed_out: true,
                stdout_truncated: false,
                stderr_truncated: false,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_execution() {
        let result = execute("echo", Some(&["hello".to_string()]), None, None, None, None)
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
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.is_empty());
        assert_eq!(result.stderr.trim(), "err");
    }

    #[tokio::test]
    async fn test_non_zero_exit_code() {
        let result = execute("false", None, None, None, None, None).await.unwrap();
        assert_ne!(result.exit_code, 0);
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn test_timeout() {
        let result = execute("sleep", Some(&["10".to_string()]), None, Some(1), None, None)
            .await
            .unwrap();
        assert!(result.timed_out);
        assert_eq!(result.exit_code, -1);
    }

    #[tokio::test]
    async fn test_custom_cwd() {
        let result = execute("pwd", None, Some("/tmp"), None, None, None)
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("tmp"));
    }

    #[tokio::test]
    async fn test_command_not_found() {
        let result = execute("nonexistent_command_xyz_12345", None, None, None, None, None).await;
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("not found") || err.contains("Command not found"));
    }

    #[tokio::test]
    async fn test_empty_command() {
        let result = execute("", None, None, None, None, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_stdin_piping() {
        let result = execute("cat", None, None, None, Some("hello from stdin"), None)
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
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_max_lines_truncation() {
        // Generate 10 lines, keep last 3
        let result = execute(
            "sh",
            Some(&["-c".to_string(), "for i in $(seq 1 10); do echo line$i; done".to_string()]),
            None,
            None,
            None,
            Some(3),
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
        let result = execute("echo", Some(&["hello".to_string()]), None, None, None, Some(5))
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(!result.stdout_truncated);
        assert_eq!(result.stdout.trim(), "hello");
    }

    #[tokio::test]
    async fn test_execute_script_basic() {
        let result = execute_script("echo hello\necho world", None, None, None, None, None)
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
        let result = execute_script("echo $MY_VAR", None, None, None, None, Some(&env))
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "hello_env");
    }

    #[tokio::test]
    async fn test_max_lines_zero_means_unlimited() {
        let result = execute(
            "sh",
            Some(&["-c".to_string(), "for i in $(seq 1 10); do echo line$i; done".to_string()]),
            None,
            None,
            None,
            Some(0),
        )
        .await
        .unwrap();
        assert!(!result.stdout_truncated);
        let lines: Vec<&str> = result.stdout.trim().lines().collect();
        assert_eq!(lines.len(), 10);
    }
}
