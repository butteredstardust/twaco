//! Run one SQL command or query through a short-lived Thing on the built-in Database template.
//! The repository identifies the connection, but credentials and connection values come from
//! the live Thing and the selected profile. The temporary Thing is removed on every exit path.

use super::codes::{Coded, ErrorCode};

mod connection;
mod execute;
mod model;
mod remote;
mod resolve;
mod sweep;

// A string newtype that mixes caller mistakes, server failures and local failures: no variant
// says which, so the honest category is `unclassified` until the type gains variants.
impl Coded for DbError {
    fn code(&self) -> ErrorCode {
        ErrorCode::Unclassified
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use connection::without_password;
pub(crate) use execute::execute;
pub(crate) use model::{DbError, Report};
pub use model::{Mode, Options};
pub(crate) use remote::Remote;
#[cfg(test)]
pub(crate) use resolve::resolve_thing;
#[cfg(test)]
pub(crate) use sweep::is_temporary_name;
pub use sweep::SweepStatus;
pub(crate) use sweep::{sweep, Sweeper, Swept};
