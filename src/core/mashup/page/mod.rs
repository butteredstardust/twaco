//! Offline page compilation. Applying artifacts and baseline ownership belong to the build layer.

mod baseline;
pub(crate) mod build;
pub(crate) mod compile;
pub mod components;
mod derive;
mod elements;
pub mod extract;
pub(crate) mod ids;
pub(crate) mod merge;
mod preserve;
mod schema;

#[derive(Debug, thiserror::Error)]
#[error("page.yaml:{line}:{column}: {message}")]
pub(crate) struct Error {
    pub(crate) line: u64,
    pub(crate) column: u64,
    pub(crate) message: String,
}

impl Error {
    pub(crate) fn at(location: serde_saphyr::Location, message: impl ToString) -> Self {
        Self {
            line: location.line().max(1),
            column: location.column().max(1),
            message: message.to_string(),
        }
    }
}

/// Specs nest widgets, and parsing and placing each level recurses with large frames (debug
/// builds especially), so a modest page overflowed the 1 MiB main-thread stack. The work runs on a
/// thread with room for the deepest spec the YAML parser's depth budget allows (deeper input is
/// refused there with a located error, so nesting cannot recurse without bound).
pub(crate) fn deep<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn_scoped(scope, work)
            .expect("spawn page worker")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

#[cfg(test)]
mod tests;
