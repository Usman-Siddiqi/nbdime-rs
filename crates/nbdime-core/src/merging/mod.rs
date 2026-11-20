//! Three-way merge of JSON documents and notebooks.
//!
//! A merge is a list of [`MergeDecision`]s: for every place where the local and remote sides
//! changed the base, which side to take, whether that is a conflict, and what to do about it.
//! The decisions are computed from the two diffs against the base ([`decide_merge`],
//! [`decide_notebook_merge`]) and then applied to the base ([`apply_decisions`]).

pub mod autoresolve;
pub mod chunks;
pub mod decisions;
mod generic;
pub mod notebooks;
pub mod strategies;
pub mod text;

use crate::diff_format::DiffFormatError;
use crate::diffing::DiffError;
use crate::patch::PatchError;

pub use decisions::{apply_decisions, Action, MergeDecision, MergeDecisionBuilder};
pub use generic::{decide_merge, decide_merge_with_diff};
pub use notebooks::{decide_notebook_merge, merge_notebooks, notebook_merge_strategies, MergeArgs};
pub use strategies::Strategies;

/// Why a merge could not be computed or applied.
///
/// The variants follow the exceptions upstream raises, which the engine reproduces wherever the
/// input is something the merge cannot handle (inconsistent decisions, diffs that do not fit the
/// base, a conflict under the `fail` strategy).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MergeError {
    /// An internal consistency check failed (`AssertionError`).
    #[error("{0}")]
    Assertion(String),
    /// A conflict that the strategy `fail` forbids, or a strategy that cannot apply
    /// (`RuntimeError`).
    #[error("{0}")]
    Runtime(String),
    /// A value of the wrong kind, or a key that does not resolve (`ValueError`, `TypeError`,
    /// `KeyError` and `IndexError`).
    #[error("{0}")]
    Invalid(String),
    /// A decision action that is not defined (`NotImplementedError`).
    #[error("The action \"{0}\" is not defined")]
    NotImplemented(String),
    /// Rendering an inline source conflict with `git merge-file` failed.
    #[error("merge rendering failed: {0}")]
    Render(String),
    #[error(transparent)]
    Diff(#[from] DiffError),
    #[error(transparent)]
    Patch(#[from] PatchError),
    #[error(transparent)]
    Format(#[from] DiffFormatError),
}

pub(crate) fn assertion<T>(msg: impl Into<String>) -> Result<T, MergeError> {
    Err(MergeError::Assertion(msg.into()))
}

pub(crate) fn invalid<T>(msg: impl Into<String>) -> Result<T, MergeError> {
    Err(MergeError::Invalid(msg.into()))
}
