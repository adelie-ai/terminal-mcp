#![deny(warnings)]

// Library crate for terminal-mcp.
//
// The JSON-RPC protocol, transports, and CLI are owned by mcp-core; this crate
// supplies the domain (shell execution, the dynamic script registry, audit
// logging) and the `mcp_core::McpService` adapter in `service`.

pub mod error;
pub mod operations;
pub mod service;
pub mod tools;
