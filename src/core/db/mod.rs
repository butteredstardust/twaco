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

pub use connection::without_password;
pub use execute::execute;
pub use model::{DbError, Mode, Options, Report};
pub use remote::Remote;
pub use resolve::resolve_thing;
pub use sweep::{is_temporary_name, sweep, SweepStatus, Sweeper, Swept};
