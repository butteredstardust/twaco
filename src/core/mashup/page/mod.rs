//! Offline page compilation. Applying artifacts and baseline ownership belong to the build layer.

mod baseline;
pub(crate) mod build;
pub(crate) mod compile;
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

#[cfg(test)]
mod tests;
