//! Offline page compilation. Applying artifacts and baseline ownership belong to the build layer.
#![allow(dead_code)] // The build facade is added in batch 4.

pub(crate) mod compile;
pub(crate) mod ids;
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
