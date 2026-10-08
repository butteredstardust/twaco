//! twaco: a portable ThingWorx sidecar toolchain.
//!
//! The library is the whole tool. `src/main.rs` and, later, the MCP server are thin adapters
//! over it, which is what keeps the two surfaces from drifting apart.

pub mod core;

#[cfg(test)]
mod xml_oracle;

/// The version, and the commit a build came from when it was built in a git checkout:
/// `0.1.0 (a1b2c3d4e 2026-10-06)`. A build with uncommitted changes says `+dirty` after the commit.
pub fn version() -> String {
    match option_env!("TWACO_BUILD") {
        Some(build) => format!("{} ({build})", env!("CARGO_PKG_VERSION")),
        None => env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// The Model Context Protocol server, the other adapter beside the CLI.
pub mod mcp;
