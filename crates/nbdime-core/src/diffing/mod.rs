//! The generic diff engine: sequence alignment, string diffs and the recursive JSON diff.

pub mod config;
pub mod generic;
pub mod sequence;
pub mod snakes;
pub mod strings;

use crate::diff_format::DiffFormatError;

pub use config::{DiffConfig, Differ, Predicate};
pub use generic::{
    compare_strings_approximate, diff, diff_dicts, diff_lists, diff_sequence_multilevel,
    diff_with_config,
};
pub use strings::{diff_strings_by_char, diff_strings_linewise};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiffError {
    #[error("Can currently only diff list, dict, or str objects.")]
    UnsupportedTypes,
    #[error("Arguments need to be string types.")]
    NotStrings,
    #[error("invalid diff: {0}")]
    InvalidDiff(#[from] DiffFormatError),
}
