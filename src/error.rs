#![deny(warnings)]

// Central error types for terminal-mcp.

use thiserror::Error;

/// Top-level error type used across the crate.
#[derive(Error, Debug)]
pub enum TerminalMcpError {
    /// Shell execution errors.
    #[error("Shell error: {0}")]
    Shell(#[from] ShellError),

    /// JSON serialization/deserialization errors.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// MCP protocol errors.
    #[error("MCP protocol error: {0}")]
    Mcp(#[from] McpError),

    /// Transport-layer errors.
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),

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

/// Errors related to MCP/JSON-RPC semantics.
#[derive(Error, Debug)]
pub enum McpError {
    /// Unsupported MCP protocol version in initialize request.
    #[error("Unsupported protocol version: {0}")]
    InvalidProtocolVersion(String),

    /// JSON-RPC message shape was invalid.
    #[error("Invalid JSON-RPC message: {0}")]
    InvalidJsonRpc(String),

    /// Requested tool name does not exist.
    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    /// Tool parameters were missing or invalid.
    #[error("Invalid tool parameters: {0}")]
    InvalidToolParameters(String),
}

/// Transport-level framing and connection errors.
#[derive(Error, Debug)]
pub enum TransportError {
    /// WebSocket connection error.
    #[error("WebSocket connection error: {0}")]
    WebSocket(String),

    /// Incoming message framing or format was invalid.
    #[error("Invalid message format: {0}")]
    InvalidMessage(String),

    /// Transport stream was closed by peer.
    #[error("Connection closed")]
    ConnectionClosed,

    /// I/O error while reading or writing transport data.
    #[error("Transport IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience result alias for crate APIs.
pub type Result<T> = std::result::Result<T, TerminalMcpError>;
