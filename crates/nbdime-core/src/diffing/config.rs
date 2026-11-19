//! Diff configuration as data: which predicates align sequence items and which differ handles
//! the items at a path.

use std::collections::HashMap;

use crate::diff_format::Diff;
use crate::value::Value;

use super::approximate::compare_values_approximate;
use super::generic::diff_values;
use super::strings::diff_strings_by_char;
use super::DiffError;

/// Decides whether two sequence items are "the same" for alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Predicate {
    /// Python `==`.
    Eq,
    /// `compare_strings_approximate` with its default threshold, on two strings or two lists.
    /// Other operands are an error unless they differ in truthiness.
    StringsApproximate,
}

impl Predicate {
    /// Whether `a` and `b` count as the same item.
    pub(super) fn apply(self, a: &Value, b: &Value) -> Result<bool, DiffError> {
        match self {
            Predicate::Eq => Ok(a.py_eq(b)),
            Predicate::StringsApproximate => compare_values_approximate(a, b),
        }
    }
}

/// Computes the diff of two items found at a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Differ {
    /// `nbdime.diff`: dispatches on the type of the items.
    Generic,
    /// `diff_strings_by_char`.
    StringsByChar,
}

impl Differ {
    pub(super) fn apply(
        self,
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
        }
    }
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

    pub fn differ(&self, path: &str) -> Differ {
        self.differs
            .get(path)
            .copied()
            .unwrap_or(self.default_differ)
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
        assert_eq!(config.differ("/x"), Differ::Generic);
        config.set_predicates("/x", vec![Predicate::StringsApproximate, Predicate::Eq]);
        config.set_differ("/x/*", Differ::StringsByChar);
        assert_eq!(config.predicates("/x").len(), 2);
        assert_eq!(config.predicates("/y"), [Predicate::Eq]);
        assert_eq!(config.differ("/x/*"), Differ::StringsByChar);
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
    fn paths_join_with_slash() {
        assert_eq!(join_path("", "cells"), "/cells");
        assert_eq!(join_path("/cells", "*"), "/cells/*");
    }
}
