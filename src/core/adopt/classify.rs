//! The small, value-agnostic part of adopt's three-way comparison.

/// How an exported value relates to the repository and the collaborator's base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Same,
    Stale,
    Theirs,
    Conflict,
    Added,
    WeRemoved,
    Unknown,
}

/// The owner-shaped grouping used to narrow an adopt report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ui,
    Backend,
}

/// Classify two unequal values for which both have a base value.
pub(crate) fn three_way(theirs_is_base: bool, ours_is_base: bool) -> Change {
    match (theirs_is_base, ours_is_base) {
        (true, false) => Change::Stale,
        (false, true) => Change::Theirs,
        (false, false) => Change::Conflict,
        // The caller has already handled `theirs == ours`; this still has a useful,
        // conservative answer if it is ever called with two independently-normalised values.
        (true, true) => Change::Same,
    }
}
