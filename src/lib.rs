//! twaco: a portable ThingWorx sidecar toolchain.
//!
//! The library is the whole tool. `src/main.rs` and, later, the MCP server are thin adapters
//! over it, which is what keeps the two surfaces from drifting apart.

pub mod core;

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

/// A name part that no other test in this process gets: the time and a counter. Tests run in
/// parallel, and the clock alone repeats: on macOS it counts whole microseconds.
#[cfg(test)]
pub(crate) fn test_nonce() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{nanos}-{next}")
}
