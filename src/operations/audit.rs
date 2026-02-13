#![deny(warnings)]

// Audit logging for MCP tool calls

use crate::operations::execute::ExecuteResult;
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

/// Audit logger that writes session logs and per-command output logs
/// to `MCP_TERMINAL_LOG_DIR`.
pub struct AuditLogger {
    log_dir: PathBuf,
    /// Shared prefix: `<session_id>_<timestamp>`
    prefix: String,
    /// Monotonic command counter for NNN suffixes.
    counter: AtomicUsize,
    /// Session log file handle, protected by a mutex for interior mutability.
    session_log: Mutex<File>,
}

impl AuditLogger {
    /// Create a new AuditLogger. Generates a session ID and timestamp,
    /// creates the session log file, and returns `Self`.
    pub fn new(log_dir: PathBuf) -> std::io::Result<Self> {
        fs::create_dir_all(&log_dir)?;

        let session_id = &uuid::Uuid::new_v4().to_string()[..8];
        let timestamp = format_timestamp();
        let prefix = format!("{}_{}", session_id, timestamp);

        let session_log_path = log_dir.join(format!("{}_session.log", prefix));
        let session_log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(session_log_path)?;

        Ok(Self {
            log_dir,
            prefix,
            counter: AtomicUsize::new(0),
            session_log: Mutex::new(session_log),
        })
    }

    /// Append a TOOL line to the session log.
    pub fn log_tool_call(&self, tool_name: &str, arguments: &Value) {
        let now = now_iso8601();
        let args_summary = summarize_arguments(arguments);
        let line = format!("[{}] TOOL {} {}\n", now, tool_name, args_summary);
        self.append_session(&line);
    }

    /// Append a RESULT line to the session log.
    pub fn log_tool_result(&self, summary: &str) {
        let now = now_iso8601();
        let line = format!("[{}] RESULT {}\n", now, summary);
        self.append_session(&line);
    }

    /// Write a numbered command log file with raw output.
    /// Returns the log filename (e.g. "abc12345_20260213T143022_001.log")
    /// for cross-referencing in the session log.
    pub fn log_command(
        &self,
        command_desc: &str,
        cwd: Option<&str>,
        result: &ExecuteResult,
    ) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        let filename = format!("{}_{:03}.log", self.prefix, n);
        let path = self.log_dir.join(&filename);

        let mut content = String::new();
        content.push_str(&format!("$ {}\n", command_desc));
        if let Some(cwd) = cwd {
            content.push_str(&format!("(cwd: {})\n", cwd));
        }
        content.push_str("--- stdout ---\n");
        content.push_str(&result.stdout);
        if !result.stdout.ends_with('\n') && !result.stdout.is_empty() {
            content.push('\n');
        }
        content.push_str("--- stderr ---\n");
        content.push_str(&result.stderr);
        if !result.stderr.ends_with('\n') && !result.stderr.is_empty() {
            content.push('\n');
        }
        if result.timed_out {
            content.push_str("--- timed out ---\n");
        } else {
            content.push_str(&format!("--- exit code: {} ---\n", result.exit_code));
        }

        // Best-effort write; don't fail the tool call if logging fails.
        let _ = fs::write(path, content.as_bytes());
        filename
    }

    fn append_session(&self, line: &str) {
        if let Ok(mut f) = self.session_log.lock() {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

fn format_timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    // Convert to calendar components (UTC)
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Days since epoch to Y-M-D (simplified Gregorian)
    let (year, month, day) = days_to_ymd(days);
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}",
        year, month, day, hours, minutes, seconds
    )
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    days += 719468;
    let era = days / 146097;
    let doe = days % 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn now_iso8601() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;
    let (year, month, day) = days_to_ymd(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

fn summarize_arguments(args: &Value) -> String {
    let Some(obj) = args.as_object() else {
        return String::new();
    };
    let mut parts = Vec::new();
    for (k, v) in obj {
        match v {
            Value::String(s) => {
                // Truncate long values
                if s.len() > 80 {
                    parts.push(format!("{}=\"{}...\"", k, &s[..77]));
                } else {
                    parts.push(format!("{}={:?}", k, s));
                }
            }
            Value::Array(arr) => {
                let items: Vec<String> = arr
                    .iter()
                    .map(|v| {
                        if let Some(s) = v.as_str() {
                            format!("{:?}", s)
                        } else {
                            v.to_string()
                        }
                    })
                    .collect();
                parts.push(format!("{}=[{}]", k, items.join(", ")));
            }
            _ => {
                parts.push(format!("{}={}", k, v));
            }
        }
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_timestamp() {
        let ts = format_timestamp();
        // Should match YYYYMMDDTHHMMSS pattern
        assert_eq!(ts.len(), 15);
        assert_eq!(&ts[8..9], "T");
    }

    #[test]
    fn test_summarize_arguments() {
        let args = serde_json::json!({"command": "ls", "args": ["-la"]});
        let s = summarize_arguments(&args);
        assert!(s.contains("command="));
        assert!(s.contains("ls"));
        assert!(s.contains("args="));
    }

    #[test]
    fn test_audit_logger_creates_files() {
        let dir = tempfile::tempdir().unwrap();
        let logger = AuditLogger::new(dir.path().to_path_buf()).unwrap();

        // Session log should exist
        let session_files: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with("_session.log"))
            .collect();
        assert_eq!(session_files.len(), 1);

        // Log a tool call and result
        logger.log_tool_call("terminal_execute", &serde_json::json!({"command": "ls"}));
        let result = ExecuteResult {
            exit_code: 0,
            stdout: "file1\nfile2\n".to_string(),
            stderr: String::new(),
            timed_out: false,
            stdout_truncated: false,
            stderr_truncated: false,
        };
        let filename = logger.log_command("ls", Some("/tmp"), &result);
        logger.log_tool_result(&format!("exit_code=0 timed_out=false -> {}", filename));

        // Command log should exist
        let cmd_files: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with("_001.log"))
            .collect();
        assert_eq!(cmd_files.len(), 1);

        // Check session log content
        let session_content = fs::read_to_string(session_files[0].path()).unwrap();
        assert!(session_content.contains("TOOL terminal_execute"));
        assert!(session_content.contains("RESULT"));
        assert!(session_content.contains("001.log"));

        // Check command log content
        let cmd_content = fs::read_to_string(cmd_files[0].path()).unwrap();
        assert!(cmd_content.contains("$ ls"));
        assert!(cmd_content.contains("(cwd: /tmp)"));
        assert!(cmd_content.contains("file1"));
        assert!(cmd_content.contains("--- exit code: 0 ---"));
    }
}
