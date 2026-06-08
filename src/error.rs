#![deny(warnings)]

// Domain error types for terminal-mcp.
//
// JSON-RPC / protocol / transport errors are owned by mcp-core; this module
// covers only the server's own domain (shell execution, stored scripts, and
// tool-parameter validation). These map onto `mcp_core::CallError` at the
// service boundary (see `src/service.rs`).

use thiserror::Error;

/// Top-level domain error used across the crate.
#[derive(Error, Debug)]
pub enum TerminalMcpError {
    /// Shell execution errors.
    #[error("Shell error: {0}")]
    Shell(#[from] ShellError),

    /// MCP tool-dispatch errors (unknown tool, bad parameters).
    #[error("MCP protocol error: {0}")]
    Mcp(#[from] McpError),

    /// JSON serialization failure while building a tool reply.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Script storage and invocation errors.
    #[error("Script error: {0}")]
    Script(#[from] ScriptError),

    /// Underlying I/O errors.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Errors originating from process execution.
#[derive(Error, Debug)]
pub enum ShellError {
    /// Command binary could not be found on PATH.
    #[error("Command not found: {0}")]
    CommandNotFound(String),

    /// Command setup or execution failed before a clean result was produced.
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    /// Command exceeded the configured timeout.
    #[error("Command timed out after {0} seconds")]
    Timeout(u64),

    /// Command input was invalid.
    #[error("Invalid command: {0}")]
    InvalidCommand(String),
}

/// Errors related to stored scripts.
#[derive(Error, Debug)]
pub enum ScriptError {
    /// Requested script was not found.
    #[error("Script not found: {0}")]
    NotFound(String),

    /// Script name failed validation.
    #[error("Invalid script name: {0}")]
    InvalidName(String),
}

/// Errors related to MCP tool dispatch semantics.
#[derive(Error, Debug)]
pub enum McpError {
    /// Requested tool name does not exist.
    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    /// Tool parameters were missing or invalid.
    #[error("Invalid tool parameters: {0}")]
    InvalidToolParameters(String),
}

/// Convenience result alias for crate APIs.
pub type Result<T> = std::result::Result<T, TerminalMcpError>;
