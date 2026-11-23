//! Offline page compilation. Applying artifacts and baseline ownership belong to the build layer.

mod align;
mod baseline;
pub(crate) mod build;
pub(crate) mod compile;
pub mod components;
mod derive;
mod elements;
pub mod extract;
pub(crate) mod ids;
pub(crate) mod merge;
mod native;
mod preserve;
mod schema;

#[derive(Debug, thiserror::Error)]
#[error("{}:{line}:{column}: {message}", file.as_deref().unwrap_or("page.yaml"))]
pub(crate) struct Error {
    pub(crate) line: u64,
    pub(crate) column: u64,
    pub(crate) message: String,
    /// The spec file, once a build knows which one it read.
    pub(crate) file: Option<String>,
}

impl Error {
    pub(crate) fn at(location: serde_saphyr::Location, message: impl ToString) -> Self {
        Self {
            line: location.line().max(1),
            column: location.column().max(1),
            message: message.to_string(),
            file: None,
        }
    }

    pub(crate) fn in_file(mut self, file: impl ToString) -> Self {
        self.file = Some(file.to_string());
        self
    }
}

/// A spec error message as the JSON object `--json` prints: `file`, `line`, `column` and
/// `message` when it is `file:line:column: message`, else only `message`.
pub fn error_object(text: &str) -> serde_json::Value {
    let located = || {
        let mut at = text.len();
        // The first `:<line>:<column>: ` from the right that is followed by the message.
        while let Some(found) = text[..at].rfind(": ") {
            let head = &text[..found];
            let mut parts = head.rsplitn(3, ':');
            let (column, line, file) = (parts.next()?, parts.next()?, parts.next()?);
            if let (Ok(column), Ok(line)) = (column.parse::<u64>(), line.parse::<u64>()) {
                return Some(
                    serde_json::json!({"file": file, "line": line, "column": column, "message": &text[found + 2..]}),
                );
            }
            at = found;
        }
        None
    };
    serde_json::json!({"error": located().unwrap_or_else(|| serde_json::json!({"message": text}))})
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
