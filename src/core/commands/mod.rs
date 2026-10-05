//! Command contracts shared by the command-line and MCP adapters.

pub mod push;

/// Whether a command describes a change or carries it out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Plan,
    Apply,
}

/// The level of access an outcome used or may have used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    None,
    Read,
    Write,
}

/// The workspace and server access associated with a command outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effects {
    pub workspace: Access,
    pub server: Access,
}

impl Effects {
    pub const fn new(workspace: Access, server: Access) -> Self {
        Self { workspace, server }
    }
}
