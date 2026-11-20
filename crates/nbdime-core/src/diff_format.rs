//! nbdime's diff format (`diff_format.schema.json`): entries, builders and validation.

use std::fmt;

use indexmap::IndexMap;

use crate::value::{Int, Map, Value};

/// A diff is an ordered list of entries applying to one list, dict or string.
pub type Diff = Vec<DiffEntry>;

/// The key of a diff entry: an index for sequences, a name for mappings.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Index(usize),
    Name(String),
}

impl Key {
    pub fn as_index(&self) -> Option<usize> {
        match self {
            Key::Index(i) => Some(*i),
            Key::Name(_) => None,
        }
    }

    pub fn as_name(&self) -> Option<&str> {
        match self {
            Key::Name(s) => Some(s),
            Key::Index(_) => None,
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Index(i) => write!(f, "{i}"),
            Key::Name(s) => f.write_str(s),
        }
    }
}

impl From<usize> for Key {
    fn from(i: usize) -> Self {
        Key::Index(i)
    }
}

impl From<&str> for Key {
    fn from(s: &str) -> Self {
        Key::Name(s.to_owned())
    }
}

impl From<String> for Key {
    fn from(s: String) -> Self {
        Key::Name(s)
    }
}

/// A diff operation and its payload.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Insert `value` at a mapping key.
    Add { value: Value },
    /// Remove the value at a mapping key.
    Remove,
    /// Replace the value at a mapping key.
    Replace { value: Value },
    /// Insert values before a sequence index. For character diffs of strings `valuelist` is a
    /// [`Value::Str`]; otherwise it is a [`Value::List`].
    AddRange { valuelist: Value },
    /// Remove `length` items starting at a sequence index.
    RemoveRange { length: usize },
    /// Apply a nested diff to the value at the key.
    Patch { diff: Diff },
    /// Marks a key whose parent was deleted on the other side. Internal to the merge engine:
    /// diffing never produces it and it is neither valid in a diff nor serialized as a diff op
    /// by anything but a decision that kept it.
    ParentDeleted,
}

impl Op {
    pub fn name(&self) -> &'static str {
        match self {
            Op::Add { .. } => "add",
            Op::Remove => "remove",
            Op::Replace { .. } => "replace",
            Op::AddRange { .. } => "addrange",
            Op::RemoveRange { .. } => "removerange",
            Op::Patch { .. } => "patch",
            Op::ParentDeleted => "parent_deleted",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiffEntry {
    pub key: Key,
    pub op: Op,
}

impl DiffEntry {
    pub fn add(key: impl Into<Key>, value: Value) -> Self {
        DiffEntry {
            key: key.into(),
            op: Op::Add { value },
        }
    }

    pub fn remove(key: impl Into<Key>) -> Self {
        DiffEntry {
            key: key.into(),
            op: Op::Remove,
        }
    }

    pub fn replace(key: impl Into<Key>, value: Value) -> Self {
        DiffEntry {
            key: key.into(),
            op: Op::Replace { value },
        }
    }

    pub fn addrange(key: usize, valuelist: Value) -> Self {
        DiffEntry {
            key: Key::Index(key),
            op: Op::AddRange { valuelist },
        }
    }

    pub fn removerange(key: usize, length: usize) -> Self {
        DiffEntry {
            key: Key::Index(key),
            op: Op::RemoveRange { length },
        }
    }

    pub fn patch(key: impl Into<Key>, diff: Diff) -> Self {
        DiffEntry {
            key: key.into(),
            op: Op::Patch { diff },
        }
    }

    pub fn parent_deleted(key: impl Into<Key>) -> Self {
        DiffEntry {
            key: key.into(),
            op: Op::ParentDeleted,
        }
    }

    pub fn is_addrange(&self) -> bool {
        matches!(self.op, Op::AddRange { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DiffFormatError(pub String);

fn err<T>(msg: impl Into<String>) -> Result<T, DiffFormatError> {
    Err(DiffFormatError(msg.into()))
}

/// Builds a sequence diff, keeping entries sorted by key.
///
/// At equal keys, `addrange` entries come before all other entries, and entries of the same
/// class keep their insertion order.
#[derive(Debug, Default, Clone)]
pub struct SequenceDiffBuilder {
    diff: Diff,
}

impl SequenceDiffBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts `entry` at its sorted position. `entry` must be `addrange`, `removerange` or
    /// `patch` with an index key.
    pub fn append(&mut self, entry: DiffEntry) {
        let key = entry
            .key
            .as_index()
            .expect("sequence diff entries have index keys");
        debug_assert!(matches!(
            entry.op,
            Op::AddRange { .. } | Op::RemoveRange { .. } | Op::Patch { .. }
        ));
        let before = |e: &DiffEntry| {
            let k = e
                .key
                .as_index()
                .expect("sequence diff entries have index keys");
            if entry.is_addrange() {
                k >= key
            } else {
                k > key
            }
        };
        let mut pos = self.diff.len();
        while pos > 0 && before(&self.diff[pos - 1]) {
            pos -= 1;
        }
        self.diff.insert(pos, entry);
    }

    /// Adds a `patch` entry unless `diff` is empty.
    pub fn patch(&mut self, key: usize, diff: Diff) {
        if !diff.is_empty() {
            self.append(DiffEntry::patch(key, diff));
        }
    }

    /// Adds an `addrange` entry unless `valuelist` is empty.
    pub fn addrange(&mut self, key: usize, valuelist: Value) {
        let empty = match &valuelist {
            Value::List(l) => l.is_empty(),
            Value::Str(s) => s.is_empty(),
            _ => panic!("addrange valuelist must be a list or a string"),
        };
        if !empty {
            self.append(DiffEntry::addrange(key, valuelist));
        }
    }

    /// Adds a `removerange` entry unless `length` is zero.
    pub fn removerange(&mut self, key: usize, length: usize) {
        if length > 0 {
            self.append(DiffEntry::removerange(key, length));
        }
    }

    pub fn validated(self) -> Diff {
        self.diff
    }
}

/// Builds a mapping diff; [`MappingDiffBuilder::validated`] sorts entries by key.
#[derive(Debug, Default, Clone)]
pub struct MappingDiffBuilder {
    diff: IndexMap<String, DiffEntry>,
}

impl MappingDiffBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an entry. Panics if the key already has an entry: callers derive keys from a dict, so
    /// a duplicate is a logic error.
    pub fn append(&mut self, entry: DiffEntry) {
        let key = entry
            .key
            .as_name()
            .expect("mapping diff entries have name keys")
            .to_owned();
        let previous = self.diff.insert(key, entry);
        assert!(previous.is_none(), "duplicate key in mapping diff");
    }

    pub fn add(&mut self, key: &str, value: Value) {
        self.append(DiffEntry::add(key, value));
    }

    pub fn remove(&mut self, key: &str) {
        self.append(DiffEntry::remove(key));
    }

    pub fn replace(&mut self, key: &str, value: Value) {
        self.append(DiffEntry::replace(key, value));
    }

    /// Adds a `patch` entry unless `diff` is empty.
    pub fn patch(&mut self, key: &str, diff: Diff) {
        if !diff.is_empty() {
            self.append(DiffEntry::patch(key, diff));
        }
    }

    pub fn validated(self) -> Diff {
        let mut entries: Vec<(String, DiffEntry)> = self.diff.into_iter().collect();
        // Byte order of UTF-8 equals code point order, which is how Python sorts str.
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries.into_iter().map(|(_, e)| e).collect()
    }
}

/// Checks that `diff` is well formed, as `nbdime.diff_format.validate_diff` does.
///
/// With `deep`, nested patch diffs are checked too.
pub fn validate_diff(diff: &Diff, deep: bool) -> Result<(), DiffFormatError> {
    let mut stack: Vec<&Diff> = vec![diff];
    while let Some(diff) = stack.pop() {
        for e in diff {
            match (&e.key, &e.op) {
                (Key::Index(_), Op::AddRange { valuelist }) => {
                    if !matches!(valuelist, Value::List(_) | Value::Str(_)) {
                        return err(format!(
                            "addrange expects a sequence of values to insert, not '{valuelist:?}'."
                        ));
                    }
                }
                (Key::Index(_), Op::RemoveRange { .. }) => {}
                (Key::Name(_), Op::Add { .. } | Op::Remove | Op::Replace { .. }) => {}
                (_, Op::Patch { diff }) => {
                    if deep {
                        stack.push(diff);
                    }
                }
                (key, op) => {
                    return err(format!(
                        "Invalid diff entry key '{key}' for op '{}'. Expecting int for sequences \
                         or unicode/str for mappings.",
                        op.name()
                    ))
                }
            }
        }
    }
    Ok(())
}

/// Converts a diff to its JSON form, with fields in nbdime's order (`op`, `key`, payload).
pub fn diff_to_value(diff: &[DiffEntry]) -> Value {
    Value::List(diff.iter().map(entry_to_value).collect())
}

pub fn entry_to_value(e: &DiffEntry) -> Value {
    let mut m = Map::with_capacity(3);
    let op = Value::from(e.op.name());
    let key = match &e.key {
        Key::Index(i) => Value::from(*i),
        Key::Name(s) => Value::from(s.as_str()),
    };
    if matches!(e.op, Op::ParentDeleted) {
        // Upstream builds this entry as `DiffEntry(key=..., op=...)`.
        m.insert("key".into(), key);
        m.insert("op".into(), op);
    } else {
        m.insert("op".into(), op);
        m.insert("key".into(), key);
    }
    match &e.op {
        Op::Add { value } | Op::Replace { value } => {
            m.insert("value".into(), value.clone());
        }
        Op::Remove | Op::ParentDeleted => {}
        Op::AddRange { valuelist } => {
            m.insert("valuelist".into(), valuelist.clone());
        }
        Op::RemoveRange { length } => {
            m.insert("length".into(), Value::from(*length));
        }
        Op::Patch { diff } => {
            m.insert("diff".into(), diff_to_value(diff));
        }
    }
    Value::Object(m)
}

/// Parses the JSON form of a diff.
pub fn diff_from_value(v: &Value) -> Result<Diff, DiffFormatError> {
    let Value::List(items) = v else {
        return err("DiffOp must be a list.");
    };
    items.iter().map(entry_from_value).collect()
}

pub fn entry_from_value(v: &Value) -> Result<DiffEntry, DiffFormatError> {
    let Value::Object(m) = v else {
        return err(format!("DiffOp entry '{v:?}' is not a diff type."));
    };
    let field = |name: &str| {
        m.get(name)
            .ok_or_else(|| DiffFormatError(format!("diff entry is missing '{name}'")))
    };
    let key = match field("key")? {
        Value::Int(Int::Small(i)) if *i >= 0 => Key::Index(*i as usize),
        Value::Str(s) => Key::Name(s.clone()),
        other => return err(format!("Invalid diff entry key '{other:?}'.")),
    };
    let index = |v: &Value| match v {
        Value::Int(Int::Small(i)) if *i >= 0 => Ok(*i as usize),
        other => err(format!("expected a non-negative integer, got '{other:?}'")),
    };
    let op = match field("op")?.as_str() {
        Some("add") => Op::Add {
            value: field("value")?.clone(),
        },
        Some("remove") => Op::Remove,
        Some("replace") => Op::Replace {
            value: field("value")?.clone(),
        },
        Some("addrange") => Op::AddRange {
            valuelist: field("valuelist")?.clone(),
        },
        Some("removerange") => Op::RemoveRange {
            length: index(field("length")?)?,
        },
        Some("patch") => Op::Patch {
            diff: diff_from_value(field("diff")?)?,
        },
        _ => return err(format!("Unknown diff op '{:?}'.", m.get("op"))),
    };
    Ok(DiffEntry { key, op })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{dumps, parse, DumpOptions};

    #[test]
    fn sequence_builder_orders_addrange_first() {
        let mut b = SequenceDiffBuilder::new();
        b.removerange(2, 1);
        b.patch(2, vec![DiffEntry::remove("x")]);
        b.addrange(2, Value::List(vec![Value::from(1)]));
        b.addrange(0, Value::from("ab"));
        b.addrange(2, Value::List(vec![Value::from(2)]));
        b.removerange(1, 0);
        let keys: Vec<(usize, &str)> = b
            .validated()
            .iter()
            .map(|e| (e.key.as_index().unwrap(), e.op.name()))
            .collect::<Vec<_>>()
            .into_iter()
            .collect();
        assert_eq!(
            keys,
            [
                (0, "addrange"),
                (2, "addrange"),
                (2, "addrange"),
                (2, "removerange"),
                (2, "patch")
            ]
        );
    }

    #[test]
    fn mapping_builder_sorts_by_code_point() {
        let mut b = MappingDiffBuilder::new();
        b.add("é", Value::Null);
        b.remove("b");
        b.replace("Z", Value::from(1));
        b.patch("a", vec![]);
        let keys: Vec<String> = b.validated().iter().map(|e| e.key.to_string()).collect();
        assert_eq!(keys, ["Z", "b", "é"]);
    }

    #[test]
    fn json_round_trip_keeps_field_order() {
        let text = r#"[{"op":"patch","key":"cells","diff":[{"op":"addrange","key":0,"valuelist":"ab"},{"op":"removerange","key":0,"length":2}]},{"op":"replace","key":"x","value":1.0}]"#;
        let diff = diff_from_value(&parse(text).unwrap()).unwrap();
        validate_diff(&diff, true).unwrap();
        assert_eq!(dumps(&diff_to_value(&diff), &DumpOptions::compact()), text);
    }

    #[test]
    fn validation_rejects_mismatched_keys() {
        let bad = vec![DiffEntry {
            key: Key::Name("a".into()),
            op: Op::RemoveRange { length: 1 },
        }];
        assert!(validate_diff(&bad, false).is_err());
        let nested_bad = vec![DiffEntry::patch("x", bad)];
        assert!(validate_diff(&nested_bad, false).is_ok());
        assert!(validate_diff(&nested_bad, true).is_err());
    }
}
