//! Diff configuration as data: which predicates align sequence items and which differ handles
//! the items at a path.

use std::collections::HashMap;

use crate::diff_format::{Diff, DiffEntry, Key};
use crate::value::Value;

use super::approximate::compare_values_approximate;
use super::generic::{diff_sequence_multilevel, diff_values};
use super::notebooks::{diff_attachments, diff_single_outputs};
use super::predicates::{self, Item};
use super::strings::{diff_string_lines, diff_strings_by_char};
use super::{DiffError, MAX_DIFF_DEPTH};

/// Decides whether two sequence items are "the same" for alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Predicate {
    /// Python `==`.
    Eq,
    /// `compare_strings_approximate` with its default threshold, on two strings or two lists.
    /// Other operands are an error unless they differ in truthiness.
    StringsApproximate,
    /// `compare_cell_approximate`: same cell type and similar source.
    CellApproximate,
    /// `compare_cell_moderate`: additionally, outputs that align without additions or removals.
    CellModerate,
    /// `compare_cell_strict`: nearly equal source and strictly equal outputs.
    CellStrict,
    /// `compare_cell_by_ids`: both cells have an `id` and the ids are equal.
    CellById,
    /// `compare_output_approximate`.
    OutputApproximate,
    /// `compare_output_strict`.
    OutputStrict,
}

impl Predicate {
    /// Whether `x` and `y` count as the same item.
    pub(super) fn apply<'a>(
        self,
        x: &Item<'a>,
        y: &Item<'a>,
        config: &DiffConfig,
    ) -> Result<bool, DiffError> {
        match self {
            Predicate::Eq => Ok(x.value().py_eq(y.value())),
            Predicate::StringsApproximate => compare_values_approximate(x.value(), y.value(), None),
            Predicate::CellApproximate => predicates::compare_cell_approximate(x, y),
            Predicate::CellModerate => predicates::compare_cell_moderate(x, y, config),
            Predicate::CellStrict => predicates::compare_cell_strict(x, y),
            Predicate::CellById => predicates::compare_cell_by_ids(x, y),
            Predicate::OutputApproximate => predicates::compare_output_approximate(x, y),
            Predicate::OutputStrict => predicates::compare_output_strict(x, y),
        }
    }

    /// [`Predicate::apply`] for two plain values.
    ///
    /// Values nested deeper than [`MAX_DIFF_DEPTH`] are an error, as for the diff entry points:
    /// outputs that embed cells make the predicates recurse through the values.
    pub fn compare(self, x: &Value, y: &Value, config: &DiffConfig) -> Result<bool, DiffError> {
        if x.nesting_exceeds(MAX_DIFF_DEPTH) || y.nesting_exceeds(MAX_DIFF_DEPTH) {
            return Err(DiffError::TooDeep(MAX_DIFF_DEPTH));
        }
        if self == Predicate::Eq {
            return Ok(x.py_eq(y));
        }
        self.apply(&Item::new(x), &Item::new(y), config)
    }
}

/// Computes the diff of two items found at a path.
#[derive(Clone, Debug, PartialEq)]
pub enum Differ {
    /// `nbdime.diff`: dispatches on the type of the items.
    Generic,
    /// `diff_strings_by_char`.
    StringsByChar,
    /// `diff_sequence_multilevel`: lists aligned with the predicates configured for the path.
    SequenceMultilevel,
    /// `diff_string_lines`: a cell source.
    StringLines,
    /// `diff_single_outputs`.
    SingleOutputs,
    /// `diff_attachments`.
    Attachments,
    /// `diff_ignore`: no changes, whatever the items.
    Ignore,
    /// `diff_ignore_keys`: `inner`, minus the top-level entries whose key is in `keys`.
    IgnoreKeys {
        inner: Box<Differ>,
        keys: Vec<Value>,
    },
}

impl Differ {
    pub(super) fn apply(
        &self,
        a: &Value,
        b: &Value,
        path: &str,
        config: &DiffConfig,
    ) -> Result<Diff, DiffError> {
        match self {
            Differ::Generic => diff_values(a, b, path, config),
            Differ::StringsByChar => match (a, b) {
                (Value::Str(a), Value::Str(b)) => Ok(diff_strings_by_char(a, b)),
                _ => Err(DiffError::NotStrings),
            },
            Differ::SequenceMultilevel => match (a, b) {
                (Value::List(a), Value::List(b)) => diff_sequence_multilevel(a, b, path, config),
                _ => Err(DiffError::NotLists),
            },
            Differ::StringLines => diff_string_lines(a, b),
            Differ::SingleOutputs => diff_single_outputs(a, b, path, config),
            Differ::Attachments => diff_attachments(a, b, path),
            Differ::Ignore => Ok(Vec::new()),
            Differ::IgnoreKeys { inner, keys } => {
                let mut diff = inner.apply(a, b, path, config)?;
                retain_unlisted_keys(&mut diff, keys);
                Ok(diff)
            }
        }
    }
}

/// Drops the entries whose key is in `keys` (Python `in`, so `1`, `1.0` and `True` are one key).
pub(super) fn retain_unlisted_keys(diff: &mut Vec<DiffEntry>, keys: &[Value]) {
    diff.retain(|e| {
        let key = match &e.key {
            Key::Index(i) => Value::from(*i),
            Key::Name(name) => Value::from(name.as_str()),
        };
        !keys.iter().any(|k| key.py_eq_item(k))
    });
}

/// Whether the sequence index `index` is in `keys`, as [`retain_unlisted_keys`] decides it.
pub(super) fn index_listed(index: usize, keys: &[Value]) -> bool {
    let key = Value::from(index);
    keys.iter().any(|k| key.py_eq_item(k))
}

/// Predicates, differs and atomic-path overrides keyed by path (`/cells/*/source`).
///
/// Paths without an entry use the defaults. Paths are built like upstream builds them: children
/// of a dict join the key with `/`, list items use `/*`.
#[derive(Clone, Debug)]
pub struct DiffConfig {
    default_predicates: Vec<Predicate>,
    default_differ: Differ,
    predicates: HashMap<String, Vec<Predicate>>,
    differs: HashMap<String, Differ>,
    atomic_paths: HashMap<String, bool>,
}

impl Default for DiffConfig {
    /// Compare list items with `==` and diff everything with the generic differ.
    fn default() -> Self {
        DiffConfig::new(vec![Predicate::Eq], Differ::Generic)
    }
}

impl DiffConfig {
    /// A configuration whose every path uses `predicates` (strictest last) and `differ`.
    ///
    /// # Panics
    /// If `predicates` is empty.
    pub fn new(predicates: Vec<Predicate>, differ: Differ) -> Self {
        assert!(!predicates.is_empty(), "at least one predicate is required");
        DiffConfig {
            default_predicates: predicates,
            default_differ: differ,
            predicates: HashMap::new(),
            differs: HashMap::new(),
            atomic_paths: HashMap::new(),
        }
    }

    pub fn predicates(&self, path: &str) -> &[Predicate] {
        self.predicates
            .get(path)
            .unwrap_or(&self.default_predicates)
    }

    pub fn differ(&self, path: &str) -> &Differ {
        self.differs.get(path).unwrap_or(&self.default_differ)
    }

    /// Sets the predicates of `path`, ordered from the loosest to the strictest.
    ///
    /// # Panics
    /// If `predicates` is empty.
    pub fn set_predicates(&mut self, path: &str, predicates: Vec<Predicate>) {
        assert!(!predicates.is_empty(), "at least one predicate is required");
        self.predicates.insert(path.to_owned(), predicates);
    }

    pub fn set_differ(&mut self, path: &str, differ: Differ) {
        self.differs.insert(path.to_owned(), differ);
    }

    /// Forces values at `path` to be treated as atomic (or not), whatever their type.
    pub fn set_atomic(&mut self, path: &str, atomic: bool) {
        self.atomic_paths.insert(path.to_owned(), atomic);
    }

    /// Whether `x` at `path` is replaced as a whole instead of being diffed.
    pub fn is_atomic(&self, x: &Value, path: &str) -> bool {
        self.atomic_paths.get(path).copied().unwrap_or(!matches!(
            x,
            Value::Str(_) | Value::List(_) | Value::Object(_)
        ))
    }
}

/// `"/".join((path, key))`.
pub fn join_path(path: &str, key: &str) -> String {
    format!("{path}/{key}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        let mut config = DiffConfig::default();
        assert_eq!(config.predicates("/x"), [Predicate::Eq]);
        assert_eq!(config.differ("/x"), &Differ::Generic);
        config.set_predicates("/x", vec![Predicate::StringsApproximate, Predicate::Eq]);
        config.set_differ("/x/*", Differ::StringsByChar);
        assert_eq!(config.predicates("/x").len(), 2);
        assert_eq!(config.predicates("/y"), [Predicate::Eq]);
        assert_eq!(config.differ("/x/*"), &Differ::StringsByChar);
    }

    #[test]
    fn atomic_by_type_or_override() {
        let mut config = DiffConfig::default();
        assert!(config.is_atomic(&Value::Null, "/a"));
        assert!(config.is_atomic(&Value::from(1), "/a"));
        assert!(!config.is_atomic(&Value::from("s"), "/a"));
        assert!(!config.is_atomic(&Value::List(vec![]), "/a"));
        config.set_atomic("/a", true);
        assert!(config.is_atomic(&Value::List(vec![]), "/a"));
        config.set_atomic("/b", false);
        assert!(!config.is_atomic(&Value::Null, "/b"));
    }

    #[test]
    fn ignore_keys_filters_top_level_entries_with_python_membership() {
        let entries = || {
            vec![
                DiffEntry::remove("execution_count"),
                DiffEntry::remove("source"),
            ]
        };
        let differ = |keys: Vec<Value>| Differ::IgnoreKeys {
            inner: Box::new(Differ::Ignore),
            keys,
        };
        let mut diff = entries();
        retain_unlisted_keys(&mut diff, &[Value::from("source"), Value::Null]);
        assert_eq!(diff, [DiffEntry::remove("execution_count")]);
        // Index keys are looked up as integers, and `1 == True == 1.0`.
        let mut diff = vec![DiffEntry::removerange(1, 2), DiffEntry::removerange(2, 1)];
        retain_unlisted_keys(&mut diff, &[Value::Bool(true), Value::from("2")]);
        assert_eq!(diff, [DiffEntry::removerange(2, 1)]);
        // The wrapped differ's own result is what gets filtered.
        let config = DiffConfig::default();
        let (a, b) = (Value::from("x"), Value::from("y"));
        let ignore_nothing = Differ::IgnoreKeys {
            inner: Box::new(Differ::Generic),
            keys: vec![],
        };
        assert_eq!(
            ignore_nothing.apply(&a, &b, "", &config),
            Differ::Generic.apply(&a, &b, "", &config)
        );
        assert_eq!(differ(vec![]).apply(&a, &b, "", &config), Ok(Vec::new()));
    }

    #[test]
    fn paths_join_with_slash() {
        assert_eq!(join_path("", "cells"), "/cells");
        assert_eq!(join_path("/cells", "*"), "/cells/*");
    }
}
