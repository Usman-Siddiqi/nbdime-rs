//! The generic diff engine: sequence alignment, string diffs and the recursive JSON diff.

mod approximate;
pub mod config;
pub mod generic;
pub mod sequence;
pub mod snakes;
pub mod strings;

use crate::diff_format::DiffFormatError;

pub use approximate::compare_strings_approximate;
pub use config::{DiffConfig, Differ, Predicate};
pub use generic::{diff, diff_with_config};
pub use strings::{diff_strings_by_char, diff_strings_linewise};

/// Deepest nesting of lists and dicts that [`diff`] accepts. The recursion over nested values
/// uses native stack, so it is bounded rather than left to the input. Upstream hits Python's
/// recursion limit somewhere between 400 and 500 levels, so this accepts slightly more.
pub const MAX_DIFF_DEPTH: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiffError {
    #[error("Cannot diff values nested more than {0} levels deep.")]
    TooDeep(usize),
    #[error("Can currently only diff list, dict, or str objects.")]
    UnsupportedTypes,
    #[error("Approximate comparison needs two strings or two lists.")]
    ApproximateOperands,
    #[error("Cannot compare lists approximately: unhashable list or dict item.")]
    UnhashableItem,
    #[error("Arguments need to be string types.")]
    NotStrings,
    #[error("invalid diff: {0}")]
    InvalidDiff(#[from] DiffFormatError),
}
