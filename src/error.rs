#![deny(warnings)]

// Error types for the terminal-mcp crate

use thiserror::Error;

/// Main error type for the terminal-mcp application
#[derive(Error, Debug)]
pub enum TerminalMcpError {
    /// Shell execution errors
    #[error("Shell error: {0}")]
    Shell(#[from] ShellError),

    /// JSON serialization/deserialization errors
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// MCP protocol errors
    #[error("MCP protocol error: {0}")]
    Mcp(#[from] McpError),

    /// Transport layer errors
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),

    /// Script errors
    #[error("Script error: {0}")]
    Script(#[from] ScriptError),

    /// IO errors
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Shell execution errors
#[derive(Error, Debug)]
pub enum ShellError {
    /// Command not found
    #[error("Command not found: {0}")]
    CommandNotFound(String),

    /// Command execution failed
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    /// Command timed out
    #[error("Command timed out after {0} seconds")]
    Timeout(u64),

    /// Invalid command
    #[error("Invalid command: {0}")]
    InvalidCommand(String),
}

/// Script storage errors
#[derive(Error, Debug)]
pub enum ScriptError {
    /// Script not found
    #[error("Script not found: {0}")]
    NotFound(String),

    /// Invalid script name
    #[error("Invalid script name: {0}")]
    InvalidName(String),
}

/// MCP protocol errors
#[derive(Error, Debug)]
pub enum McpError {
    /// Invalid protocol version
    #[error("Unsupported protocol version: {0}")]
    InvalidProtocolVersion(String),

    /// Invalid JSON-RPC message
    #[error("Invalid JSON-RPC message: {0}")]
    InvalidJsonRpc(String),

    /// Tool not found
    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    /// Invalid tool parameters
    #[error("Invalid tool parameters: {0}")]
    InvalidToolParameters(String),
}

/// Transport layer errors
#[derive(Error, Debug)]
pub enum TransportError {
    /// WebSocket connection error
    #[error("WebSocket connection error: {0}")]
    WebSocket(String),

    /// Invalid message format
    #[error("Invalid message format: {0}")]
    InvalidMessage(String),

    /// Connection closed
    #[error("Connection closed")]
    ConnectionClosed,

    /// IO error in transport
    #[error("Transport IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type alias for convenience
pub type Result<T> = std::result::Result<T, TerminalMcpError>;
