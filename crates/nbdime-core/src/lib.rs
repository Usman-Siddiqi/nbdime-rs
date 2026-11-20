//! Content-aware diff and merge for Jupyter notebooks, compatible with
//! [nbdime](https://github.com/jupyter/nbdime).
//!
//! Values, diffs and merge decisions use nbdime's JSON formats, and the algorithms reproduce
//! nbdime's results. See `docs/porting-guide.md` in the repository for the semantics.

pub mod diff_format;
pub mod diff_utils;
pub mod diffing;
pub mod difflib;
pub mod json;
pub mod log;
pub mod merging;
pub mod nbformat;
pub mod patch;
pub mod path;
pub mod prettyprint;
pub mod py;
pub mod value;

pub use diff_format::{Diff, DiffEntry, Key, Op};
pub use value::{Int, Map, PyType, Value};
