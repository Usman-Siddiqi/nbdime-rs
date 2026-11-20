//! Applying diffs to JSON values.

use std::collections::HashSet;

use crate::diff_format::{DiffEntry, DiffFormatError, Key, Op};
use crate::diff_utils::{
    flatten_list_of_string_diff, join_into, join_value, type_name, StrOrLines,
};
use crate::value::{Map, Value};

/// A diff that cannot be applied to the object it was given for.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatchError {
    /// The object is not a dict, list or string.
    #[error("Invalid object type to patch: {0}")]
    InvalidType(&'static str),
    /// The diff does not fit the object or is internally inconsistent.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Format(#[from] DiffFormatError),
}

fn invalid<T>(msg: impl Into<String>) -> Result<T, PatchError> {
    Err(PatchError::Invalid(msg.into()))
}

/// Applies a hierarchical diff to `obj`, which may be a dict, a list or a string.
///
/// A string is patched with a line-based diff, whatever produced it.
pub fn patch(obj: &Value, diff: &[DiffEntry]) -> Result<Value, PatchError> {
    match obj {
        Value::Object(map) => patch_dict(map, diff),
        Value::List(items) => patch_list(items, diff).map(Value::List),
        Value::Str(s) => patch_string(s, diff).map(Value::Str),
        other => Err(PatchError::InvalidType(type_name(other))),
    }
}

/// `patch` for a notebook: the same operation, since notebooks are plain dicts here.
pub fn patch_notebook(nb: &Value, diff: &[DiffEntry]) -> Result<Value, PatchError> {
    patch(nb, diff)
}

pub fn patch_list(obj: &[Value], diff: &[DiffEntry]) -> Result<Vec<Value>, PatchError> {
    let mut out = Vec::with_capacity(obj.len());
    let mut take = 0;
    for e in diff {
        let index = list_key(e)?;
        out.extend_from_slice(slice(obj, take, index));
        let skip = match &e.op {
            Op::AddRange { valuelist } => {
                extend_from_iterable(&mut out, valuelist)?;
                0
            }
            Op::RemoveRange { length } => *length,
            Op::Patch { diff } => {
                let Some(item) = obj.get(index) else {
                    return invalid(format!("list index {index} out of range"));
                };
                out.push(patch(item, diff)?);
                1
            }
            Op::Add { value } => {
                out.push(value.clone());
                0
            }
            Op::Remove => 1,
            Op::Replace { value } => {
                out.push(value.clone());
                1
            }
            Op::ParentDeleted => return invalid("Invalid op parent_deleted"),
        };
        // take never decreases: repeated +/- at one index can pass it.
        take = take.max(index.saturating_add(skip));
    }
    out.extend_from_slice(slice(obj, take, obj.len()));
    Ok(out)
}

/// Patches a multiline string with a line-based diff.
pub fn patch_string(obj: &str, diff: &[DiffEntry]) -> Result<String, PatchError> {
    let flat = flatten_list_of_string_diff(StrOrLines::Str(obj), diff)?;
    patch_singleline_string(obj, &flat)
}

/// Patches a string with a character-based diff (keys are code point indices).
pub fn patch_singleline_string(obj: &str, diff: &[DiffEntry]) -> Result<String, PatchError> {
    let chars = CharIndex::new(obj);
    let mut out = String::with_capacity(obj.len());
    let mut take = 0;
    for e in diff {
        let index = list_key(e)?;
        out.push_str(chars.slice(take, index));
        let skip = match &e.op {
            Op::AddRange { valuelist } => {
                join_into(&mut out, valuelist)?;
                0
            }
            Op::RemoveRange { length } => *length,
            Op::Patch { diff } => {
                if index >= chars.len() {
                    return invalid(format!("string index {index} out of range"));
                }
                out.push_str(&patch_string(chars.slice(index, index + 1), diff)?);
                1
            }
            Op::Add { value } => {
                join_value(&mut out, value)?;
                0
            }
            Op::Remove => 1,
            Op::Replace { value } => {
                join_value(&mut out, value)?;
                1
            }
            Op::ParentDeleted => return invalid("Invalid op parent_deleted"),
        };
        take = take.max(index.saturating_add(skip));
    }
    out.push_str(chars.slice(take, chars.len()));
    Ok(out)
}

pub fn patch_dict(obj: &Map, diff: &[DiffEntry]) -> Result<Value, PatchError> {
    let mut out = Map::with_capacity(obj.len());
    let mut deleted: HashSet<&str> = HashSet::new();
    for e in diff {
        let Key::Name(key) = &e.key else {
            return invalid("dict key must be string");
        };
        if out.contains_key(key) {
            return invalid(format!("multiple diff entries target same key: {key:?}"));
        }
        match &e.op {
            Op::Add { value } => {
                if obj.contains_key(key) {
                    return invalid(format!("patch add value not found for key: {key:?}"));
                }
                out.insert(key.clone(), value.clone());
            }
            Op::Remove => {
                deleted.insert(key);
            }
            Op::Replace { value } => {
                if deleted.contains(key.as_str()) {
                    return invalid(format!("cannot replace deleted key: {key:?}"));
                }
                out.insert(key.clone(), value.clone());
            }
            Op::Patch { diff } => {
                if deleted.contains(key.as_str()) {
                    return invalid(format!("cannot patch deleted key: {key:?}"));
                }
                let Some(item) = obj.get(key) else {
                    return invalid(format!("key not found in object: {key:?}"));
                };
                out.insert(key.clone(), patch(item, diff)?);
            }
            op => {
                return Err(DiffFormatError(format!("Invalid op {}.", op.name())).into());
            }
        }
    }
    // Keys named in the diff come first, in diff order, then the untouched keys.
    for (key, value) in obj {
        if !deleted.contains(key.as_str()) && !out.contains_key(key) {
            out.insert(key.clone(), value.clone());
        }
    }
    Ok(Value::Object(out))
}

fn list_key(e: &DiffEntry) -> Result<usize, PatchError> {
    match &e.key {
        Key::Index(i) => Ok(*i),
        Key::Name(_) => invalid("list key must be integer"),
    }
}

/// `items[start:end]` with Python's clamping: out-of-range bounds and `start >= end` give less,
/// down to nothing, and are never errors.
fn slice<T>(items: &[T], start: usize, end: usize) -> &[T] {
    let end = end.min(items.len());
    if start >= end {
        &[]
    } else {
        &items[start..end]
    }
}

/// `out.extend(valuelist)`: a string yields its characters and a dict its keys.
fn extend_from_iterable(out: &mut Vec<Value>, valuelist: &Value) -> Result<(), PatchError> {
    match valuelist {
        Value::List(items) => out.extend(items.iter().cloned()),
        Value::Str(s) => out.extend(s.chars().map(|c| Value::Str(c.to_string()))),
        Value::Object(m) => out.extend(m.keys().map(|k| Value::Str(k.clone()))),
        other => return invalid(format!("'{}' object is not iterable", type_name(other))),
    }
    Ok(())
}

/// Maps code point indices of a string to byte offsets.
struct CharIndex<'a> {
    s: &'a str,
    /// Byte offset of every code point, plus the total length; `None` for ASCII, where the two
    /// coincide.
    offsets: Option<Vec<usize>>,
}

impl<'a> CharIndex<'a> {
    fn new(s: &'a str) -> Self {
        let offsets = (!s.is_ascii()).then(|| {
            let mut v: Vec<usize> = s.char_indices().map(|(i, _)| i).collect();
            v.push(s.len());
            v
        });
        CharIndex { s, offsets }
    }

    fn len(&self) -> usize {
        self.offsets.as_ref().map_or(self.s.len(), |o| o.len() - 1)
    }

    fn byte(&self, i: usize) -> usize {
        let i = i.min(self.len());
        self.offsets.as_ref().map_or(i, |o| o[i])
    }

    /// `s[start:end]` in code points, with Python's clamping.
    fn slice(&self, start: usize, end: usize) -> &'a str {
        if start >= end.min(self.len()) {
            ""
        } else {
            &self.s[self.byte(start)..self.byte(end)]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_format::diff_from_value;
    use crate::json::parse;

    fn v(text: &str) -> Value {
        parse(text).unwrap()
    }

    fn run(obj: &str, diff: &str) -> Result<Value, PatchError> {
        patch(&v(obj), &diff_from_value(&v(diff)).unwrap())
    }

    fn keys(v: &Value) -> Vec<&str> {
        v.as_object().unwrap().keys().map(String::as_str).collect()
    }

    #[test]
    fn list_take_skip_and_clamping() {
        let r = run(
            "[1,2,3,4,5]",
            r#"[{"op":"removerange","key":1,"length":2},
                {"op":"addrange","key":3,"valuelist":["x"]},
                {"op":"addrange","key":99,"valuelist":["end"]}]"#,
        )
        .unwrap();
        assert_eq!(r, v(r#"[1,"x",4,5,"end"]"#));
    }

    #[test]
    fn list_take_never_decreases() {
        // remove at 2 then add at 2: take has passed index, nothing is retaken
        let r = run(
            "[0,1,2,3]",
            r#"[{"op":"remove","key":2},{"op":"add","key":2,"value":"n"},{"op":"replace","key":1,"value":"r"}]"#,
        )
        .unwrap();
        assert_eq!(r, v(r#"[0,1,"n","r",3]"#));
    }

    #[test]
    fn list_patch_entries() {
        let r = run(
            r#"[{"a":1},"abc"]"#,
            r#"[{"op":"patch","key":0,"diff":[{"op":"replace","key":"a","value":2}]},
                {"op":"patch","key":1,"diff":[{"op":"patch","key":0,"diff":[{"op":"removerange","key":1,"length":1}]}]}]"#,
        )
        .unwrap();
        assert_eq!(r, v(r#"[{"a":2},"ac"]"#));
        assert!(run("[1]", r#"[{"op":"patch","key":3,"diff":[]}]"#).is_err());
        assert!(run("[1]", r#"[{"op":"add","key":"k","value":1}]"#).is_err());
    }

    #[test]
    fn list_addrange_iterates_like_extend() {
        let r = run("[]", r#"[{"op":"addrange","key":0,"valuelist":"ab"}]"#).unwrap();
        assert_eq!(r, v(r#"["a","b"]"#));
        assert!(run("[]", r#"[{"op":"addrange","key":0,"valuelist":5}]"#).is_err());
    }

    #[test]
    fn dict_key_order_is_diff_keys_then_untouched() {
        let r = run(
            r#"{"a":1,"b":2,"c":3,"d":4}"#,
            r#"[{"op":"remove","key":"b"},{"op":"replace","key":"d","value":0},
                {"op":"add","key":"z","value":9},{"op":"replace","key":"a","value":8}]"#,
        )
        .unwrap();
        assert_eq!(keys(&r), ["d", "z", "a", "c"]);
    }

    #[test]
    fn dict_assertions_are_errors() {
        let obj = r#"{"a":1}"#;
        for diff in [
            r#"[{"op":"add","key":"a","value":2}]"#,
            r#"[{"op":"replace","key":"a","value":2},{"op":"replace","key":"a","value":3}]"#,
            r#"[{"op":"remove","key":"a"},{"op":"replace","key":"a","value":3}]"#,
            r#"[{"op":"remove","key":"a"},{"op":"patch","key":"a","diff":[]}]"#,
            r#"[{"op":"patch","key":"missing","diff":[]}]"#,
            r#"[{"op":"replace","key":0,"value":1}]"#,
            r#"[{"op":"addrange","key":"a","valuelist":[]}]"#,
        ] {
            assert!(run(obj, diff).is_err(), "{diff}");
        }
        // removing twice, or removing a missing key, is fine
        assert_eq!(
            run(
                obj,
                r#"[{"op":"remove","key":"q"},{"op":"remove","key":"q"}]"#
            )
            .unwrap(),
            v(obj)
        );
    }

    #[test]
    fn strings_use_line_based_diffs() {
        let r = run(
            r#""one\ntwo\nthree\n""#,
            r#"[{"op":"patch","key":1,"diff":[{"op":"addrange","key":0,"valuelist":"T"},{"op":"removerange","key":0,"length":1}]},
                {"op":"addrange","key":3,"valuelist":["four\n"]}]"#,
        )
        .unwrap();
        assert_eq!(r, v(r#""one\nTwo\nthree\nfour\n""#));
    }

    #[test]
    fn strings_count_code_points() {
        let r = run(
            r#""é😀x\n""#,
            r#"[{"op":"patch","key":0,"diff":[{"op":"removerange","key":1,"length":1}]}]"#,
        )
        .unwrap();
        assert_eq!(r, v(r#""éx\n""#));
        assert!(run(
            r#""abc""#,
            r#"[{"op":"addrange","key":5,"valuelist":["x"]}]"#
        )
        .is_err());
    }

    #[test]
    fn singleline_patch_clamps_and_rejects() {
        let diff = diff_from_value(&v(
            r#"[{"op":"removerange","key":2,"length":9},{"op":"addrange","key":50,"valuelist":"!"}]"#,
        ))
        .unwrap();
        assert_eq!(patch_singleline_string("héllo", &diff).unwrap(), "hé!");
        let bad = diff_from_value(&v(r#"[{"op":"add","key":0,"value":1}]"#)).unwrap();
        assert!(patch_singleline_string("x", &bad).is_err());
    }

    #[test]
    fn overflowing_removerange_ends_are_errors() {
        // The merged end 5 + 2 * i64::MAX is beyond u64; upstream sees it is not 6.
        let max = i64::MAX;
        let diff = format!(
            r#"[{{"op":"patch","key":0,"diff":[{{"op":"removerange","key":5,"length":{max}}},
                {{"op":"removerange","key":5,"length":{max}}},{{"op":"removerange","key":6,"length":1}}]}}]"#
        );
        assert!(run(r#""abcdefgh\n""#, &diff).is_err());
    }

    #[test]
    fn other_types_are_rejected() {
        for obj in ["1", "null", "true", "1.5"] {
            assert!(matches!(run(obj, "[]"), Err(PatchError::InvalidType(_))));
        }
    }
}
