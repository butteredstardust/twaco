//! twaco: a portable ThingWorx sidecar toolchain.
//!
//! The library is the whole tool. `src/main.rs` and, later, the MCP server are thin adapters
//! over it, which is what keeps the two surfaces from drifting apart.

pub mod core;

/// The Model Context Protocol server, the other adapter beside the CLI.
pub mod mcp;
