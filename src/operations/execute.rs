#![deny(warnings)]

// Shell command execution

use crate::error::{Result, ShellError};
use serde::Serialize;
use std::path::Path;
use tokio::process::Command;

#[derive(Debug, Serialize)]
pub struct ExecuteResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

pub async fn execute(
    command: &str,
    args: Option<&[String]>,
    cwd: Option<&str>,
    timeout_secs: Option<u64>,
    stdin_input: Option<&str>,
) -> Result<ExecuteResult> {
    if command.is_empty() {
        return Err(ShellError::InvalidCommand("Command cannot be empty".to_string()).into());
    }

    let timeout_secs = timeout_secs.unwrap_or(30);

    let mut cmd = Command::new(command);

    if let Some(args) = args {
        cmd.args(args);
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
    if let Some(input) = stdin_input {
        if let Some(mut child_stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let _ = child_stdin.write_all(input.as_bytes()).await;
            drop(child_stdin);
        }
    }

    // Take pipes out of child before spawning concurrent readers to avoid deadlock
    // when pipe buffers fill up.
    use tokio::io::AsyncReadExt;

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();

    let stdout_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(ref mut out) = stdout_pipe {
            let _ = out.read_to_end(&mut buf).await;
        }
        buf
    });

    let stderr_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        if let Some(ref mut err) = stderr_pipe {
            let _ = err.read_to_end(&mut buf).await;
        }
        buf
    });

    let timeout_duration = std::time::Duration::from_secs(timeout_secs);
    let result = tokio::time::timeout(timeout_duration, async {
        let stdout_bytes = stdout_handle.await.unwrap_or_default();
        let stderr_bytes = stderr_handle.await.unwrap_or_default();
        let status = child.wait().await;
        (stdout_bytes, stderr_bytes, status)
    })
    .await;

    match result {
        Ok((stdout_bytes, stderr_bytes, Ok(status))) => Ok(ExecuteResult {
            exit_code: status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&stdout_bytes).to_string(),
            stderr: String::from_utf8_lossy(&stderr_bytes).to_string(),
            timed_out: false,
        }),
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
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_execution() {
        let result = execute("echo", Some(&["hello".to_string()]), None, None, None)
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
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.is_empty());
        assert_eq!(result.stderr.trim(), "err");
    }

    #[tokio::test]
    async fn test_non_zero_exit_code() {
        let result = execute("false", None, None, None, None).await.unwrap();
        assert_ne!(result.exit_code, 0);
        assert!(!result.timed_out);
    }

    #[tokio::test]
    async fn test_timeout() {
        let result = execute("sleep", Some(&["10".to_string()]), None, Some(1), None)
            .await
            .unwrap();
        assert!(result.timed_out);
        assert_eq!(result.exit_code, -1);
    }

    #[tokio::test]
    async fn test_custom_cwd() {
        let result = execute("pwd", None, Some("/tmp"), None, None)
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("tmp"));
    }

    #[tokio::test]
    async fn test_command_not_found() {
        let result = execute("nonexistent_command_xyz_12345", None, None, None, None).await;
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("not found") || err.contains("Command not found"));
    }

    #[tokio::test]
    async fn test_empty_command() {
        let result = execute("", None, None, None, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_stdin_piping() {
        let result = execute("cat", None, None, None, Some("hello from stdin"))
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
        )
        .await;
        assert!(result.is_err());
    }
}
