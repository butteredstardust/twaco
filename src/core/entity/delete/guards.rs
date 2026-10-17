/// Conditions a caller explicitly accepts before deleting an entity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Acknowledged {
    pub repository_defined: bool,
    pub outside_dependents: bool,
    pub file_repository_data_loss: bool,
}

/// A stable reason why an entity delete was refused.
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardCode {
    RepositoryDefined,
    OutsideDependents,
    FileRepositoryDataLoss,
    NoDeleteMethod,
}

impl GuardCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RepositoryDefined => "repository_defined",
            Self::OutsideDependents => "outside_dependents",
            Self::FileRepositoryDataLoss => "file_repository_data_loss",
            Self::NoDeleteMethod => "no_delete_method",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Refusal {
    pub(super) code: GuardCode,
    pub(super) message: String,
}

impl Acknowledged {
    /// Fold in the legacy `--force` alias, returning whether it was used. `force` never
    /// acknowledges FileRepository data loss.
    pub fn with_force(self, force: bool) -> (Self, bool) {
        (
            Self {
                repository_defined: force || self.repository_defined,
                outside_dependents: force || self.outside_dependents,
                file_repository_data_loss: self.file_repository_data_loss,
            },
            force,
        )
    }
}
