//! Helpers shared by diff consumers.

use std::mem::discriminant;

use crate::diff_format::{Diff, DiffEntry, DiffFormatError, Key, Op};
use crate::patch::{invalid, join_into, PatchError};
use crate::py::{py_len, splitlines_keepends};
use crate::value::Value;

/// How many items a single sequence diff entry consumes from the base and the remote sequence.
///
/// For character diffs `valuelist` is a string and its length is counted in code points.
pub fn count_consumed_symbols(e: &DiffEntry) -> Result<(usize, usize), DiffFormatError> {
    match &e.op {
        Op::AddRange { valuelist } => Ok((
            0,
            match valuelist {
                Value::List(items) => items.len(),
                Value::Str(s) => s.chars().count(),
                _ => {
                    return Err(DiffFormatError(
                        "addrange valuelist is not a sequence".into(),
                    ))
                }
            },
        )),
        Op::RemoveRange { length } => Ok((*length, 0)),
        Op::Patch { .. } => Ok((1, 1)),
        op => Err(DiffFormatError(format!("Invalid op '{}'", op.name()))),
    }
}

/// Copies a sequence diff entry with `n` added to its key.
pub fn offset_op(e: &DiffEntry, n: isize) -> Result<DiffEntry, PatchError> {
    let shifted = match &e.key {
        Key::Index(i) => i.checked_add_signed(n),
        Key::Name(_) => None,
    };
    match shifted {
        Some(key) => Ok(DiffEntry {
            key: Key::Index(key),
            op: e.op.clone(),
        }),
        None => invalid(format!("cannot offset diff key '{}' by {n}", e.key)),
    }
}

/// A source as one string; a list of lines is joined with `\n` after stripping each line's own.
pub fn source_as_string(source: &Value) -> Result<String, PatchError> {
    match source {
        Value::Str(s) => Ok(s.clone()),
        Value::List(lines) => {
            let mut parts = Vec::with_capacity(lines.len());
            for line in lines {
                match line {
                    Value::Str(s) => parts.push(s.trim_matches('\n')),
                    _ => return invalid("source line is not a string"),
                }
            }
            Ok(parts.join("\n"))
        }
        other => invalid(format!(
            "Invalid argument type. Should be string or sequence of strings. Got {other:?}"
        )),
    }
}

/// The text a line diff is relative to: a string (split into lines here) or its lines.
#[derive(Debug, Clone, Copy)]
pub enum StrOrLines<'a> {
    Str(&'a str),
    Lines(&'a [&'a str]),
}

/// Translates a diff of lines (as split by `str.splitlines(True)`) into a diff of the joined
/// string, with keys in code points.
pub fn flatten_list_of_string_diff(
    a: &StrOrLines,
    linebased_diff: &[DiffEntry],
) -> Result<Diff, PatchError> {
    let lines: Vec<&str> = match a {
        StrOrLines::Str(s) => splitlines_keepends(s),
        StrOrLines::Lines(lines) => lines.to_vec(),
    };
    let mut line_to_char = Vec::with_capacity(lines.len() + 1);
    line_to_char.push(0);
    let mut total = 0;
    for line in &lines {
        total += py_len(line);
        line_to_char.push(total);
    }
    let at = |i: usize| {
        line_to_char
            .get(i)
            .copied()
            .ok_or_else(|| PatchError::Invalid("list index out of range".into()))
    };

    let mut charbased = Vec::with_capacity(linebased_diff.len());
    for e in linebased_diff {
        let Key::Index(key) = e.key else {
            return invalid("line diff key must be an integer");
        };
        let line_offset = at(key)?;
        match &e.op {
            Op::Patch { diff } => {
                for p in diff {
                    charbased.push(offset_op(p, line_offset as isize)?);
                }
            }
            Op::AddRange { valuelist } => {
                let mut joined = String::new();
                join_into(&mut joined, valuelist)?;
                charbased.push(DiffEntry::addrange(line_offset, Value::Str(joined)));
            }
            Op::RemoveRange { length } => {
                let end = key
                    .checked_add(*length)
                    .ok_or_else(|| PatchError::Invalid("list index out of range".into()))?;
                charbased.push(DiffEntry::removerange(line_offset, at(end)? - line_offset));
            }
            op => charbased.push(DiffEntry {
                key: Key::Index(line_offset),
                op: op.clone(),
            }),
        }
    }

    let mut combined: Diff = Vec::with_capacity(charbased.len());
    for d in charbased {
        match combined.last_mut() {
            Some(last) if overlaps(last, &d)? => combine_ops(last, d)?,
            _ => combined.push(d),
        }
    }
    // Python sorts by key only; the sort is stable.
    combined.sort_by_key(|e| e.key.as_index());
    Ok(combined)
}

fn is_add(op: &Op) -> bool {
    matches!(op, Op::Add { .. } | Op::AddRange { .. })
}

/// Whether `new` can be merged into the last op so far: same op at the same key, an add and an
/// addrange at the same key, or removeranges that touch.
fn overlaps(existing: &DiffEntry, new: &DiffEntry) -> Result<bool, PatchError> {
    let (Key::Index(ek), Key::Index(nk)) = (&existing.key, &new.key) else {
        return invalid("sequence diff entry without integer key");
    };
    if discriminant(&existing.op) == discriminant(&new.op) {
        if ek == nk {
            return Ok(true);
        }
        if let Op::RemoveRange { length } = existing.op {
            if ek + length >= *nk {
                if ek + length != *nk {
                    return invalid("Unexpected diff keys/lengths");
                }
                return Ok(true);
            }
        }
        Ok(false)
    } else {
        Ok(is_add(&existing.op) && is_add(&new.op) && ek == nk)
    }
}

/// Merges `new` into `existing`, which `overlaps` accepted.
fn combine_ops(existing: &mut DiffEntry, new: DiffEntry) -> Result<(), PatchError> {
    if let Op::Add { value } = &mut existing.op {
        let value = std::mem::take(value);
        existing.op = Op::AddRange {
            valuelist: Value::List(vec![value]),
        };
    }
    match (&mut existing.op, new.op) {
        (Op::AddRange { valuelist }, Op::AddRange { valuelist: more }) => {
            extend_valuelist(valuelist, more)
        }
        (Op::AddRange { valuelist }, Op::Add { value }) => match valuelist {
            Value::Str(s) => match value {
                Value::Str(more) => {
                    s.push_str(&more);
                    Ok(())
                }
                _ => invalid("can only concatenate str to str"),
            },
            Value::List(items) => {
                items.push(value);
                Ok(())
            }
            _ => invalid("valuelist has no append"),
        },
        (Op::RemoveRange { length }, Op::RemoveRange { length: more }) => {
            *length += more;
            Ok(())
        }
        // Upstream's _combine_ops returns None for any other equal pair (two patches or two
        // replaces at one key), which fails with AttributeError on the next use.
        _ => invalid("cannot combine diff ops at the same key"),
    }
}

/// `valuelist += more`: a list takes the items of any iterable, a string only a string.
fn extend_valuelist(valuelist: &mut Value, more: Value) -> Result<(), PatchError> {
    match (valuelist, more) {
        (Value::List(items), Value::List(more)) => items.extend(more),
        (Value::List(items), Value::Str(s)) => {
            items.extend(s.chars().map(|c| Value::Str(c.to_string())));
        }
        (Value::List(items), Value::Object(m)) => items.extend(m.into_keys().map(Value::Str)),
        (Value::Str(s), Value::Str(more)) => s.push_str(&more),
        _ => return invalid("cannot concatenate valuelists"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumed_symbols() {
        let add = DiffEntry::addrange(0, Value::from("a\u{1F600}b"));
        assert_eq!(count_consumed_symbols(&add).unwrap(), (0, 3));
        let add = DiffEntry::addrange(0, Value::List(vec![Value::Null; 2]));
        assert_eq!(count_consumed_symbols(&add).unwrap(), (0, 2));
        assert_eq!(
            count_consumed_symbols(&DiffEntry::removerange(4, 7)).unwrap(),
            (7, 0)
        );
        assert_eq!(
            count_consumed_symbols(&DiffEntry::patch(1, vec![])).unwrap(),
            (1, 1)
        );
        assert!(count_consumed_symbols(&DiffEntry::remove("k")).is_err());
    }

    fn flat(a: &str, diff: &str) -> Result<Diff, PatchError> {
        let diff = crate::diff_format::diff_from_value(&crate::json::parse(diff).unwrap()).unwrap();
        flatten_list_of_string_diff(&StrOrLines::Str(a), &diff)
    }

    fn json(diff: &Diff) -> String {
        crate::json::dumps(
            &crate::diff_format::diff_to_value(diff),
            &crate::json::DumpOptions::compact(),
        )
    }

    #[test]
    fn flatten_translates_line_keys_to_code_points() {
        let d = flat(
            "é\nbb\nc",
            r#"[{"op":"patch","key":1,"diff":[{"op":"removerange","key":1,"length":1}]},
                {"op":"removerange","key":2,"length":1},
                {"op":"addrange","key":3,"valuelist":["x\n"]}]"#,
        )
        .unwrap();
        assert_eq!(
            json(&d),
            r#"[{"op":"removerange","key":3,"length":1},{"op":"removerange","key":5,"length":1},{"op":"addrange","key":6,"valuelist":"x\n"}]"#
        );
    }

    #[test]
    fn flatten_combines_only_with_the_last_op() {
        // two removeranges at the same key add up; adjacent ones merge; an add after a
        // different op at the same key does not
        let d = flat(
            "a\nb\nc\n",
            r#"[{"op":"removerange","key":0,"length":1},{"op":"removerange","key":1,"length":1},
                {"op":"addrange","key":2,"valuelist":["x\n"]},{"op":"addrange","key":2,"valuelist":["y\n"]}]"#,
        )
        .unwrap();
        assert_eq!(
            json(&d),
            r#"[{"op":"removerange","key":0,"length":4},{"op":"addrange","key":4,"valuelist":"x\ny\n"}]"#
        );
        let d = flat(
            "a\nb\n",
            r#"[{"op":"addrange","key":1,"valuelist":["x\n"]},{"op":"removerange","key":1,"length":1},{"op":"addrange","key":1,"valuelist":["y\n"]}]"#,
        )
        .unwrap();
        assert_eq!(d.len(), 3);
    }

    #[test]
    fn flatten_errors() {
        // inconsistent removeranges
        assert!(flat(
            "a\nb\nc\n",
            r#"[{"op":"removerange","key":0,"length":2},{"op":"removerange","key":1,"length":1}]"#
        )
        .is_err());
        // two removes at one key cannot be combined
        let patch = r#"{"op":"patch","key":0,"diff":[{"op":"remove","key":0}]}"#;
        assert!(flat("a\n", &format!("[{patch},{patch}]")).is_err());
        // key past the end of the lines
        assert!(flat("a\n", r#"[{"op":"addrange","key":2,"valuelist":["x"]}]"#).is_err());
        assert!(flat("a\n", r#"[{"op":"removerange","key":0,"length":2}]"#).is_err());
    }

    #[test]
    fn flatten_accepts_lines_and_sorts_by_key() {
        let diff = crate::diff_format::diff_from_value(
            &crate::json::parse(
                r#"[{"op":"addrange","key":2,"valuelist":["z"]},{"op":"addrange","key":0,"valuelist":["a"]}]"#,
            )
            .unwrap(),
        )
        .unwrap();
        let d = flatten_list_of_string_diff(&StrOrLines::Lines(&["ab\n", "c\n"]), &diff).unwrap();
        assert_eq!(
            json(&d),
            r#"[{"op":"addrange","key":0,"valuelist":"a"},{"op":"addrange","key":5,"valuelist":"z"}]"#
        );
    }

    #[test]
    fn add_and_addrange_combine_into_an_addrange() {
        let d = flat(
            "a\n",
            r#"[{"op":"add","key":0,"value":"x"},{"op":"addrange","key":0,"valuelist":["y"]}]"#,
        )
        .unwrap();
        assert_eq!(
            json(&d),
            r#"[{"op":"addrange","key":0,"valuelist":["x","y"]}]"#
        );
    }

    #[test]
    fn offset_op_shifts_index_keys() {
        let e = DiffEntry::removerange(2, 3);
        assert_eq!(offset_op(&e, 5).unwrap(), DiffEntry::removerange(7, 3));
        assert_eq!(offset_op(&e, -2).unwrap(), DiffEntry::removerange(0, 3));
        assert!(offset_op(&e, -3).is_err());
        assert!(offset_op(&DiffEntry::remove("k"), 1).is_err());
    }

    #[test]
    fn source_as_string_joins_stripped_lines() {
        assert_eq!(
            source_as_string(&Value::List(vec![
                Value::from("a\n"),
                Value::from("\nb\n\n"),
                Value::from("c")
            ]))
            .unwrap(),
            "a\nb\nc"
        );
        assert_eq!(source_as_string(&Value::from("x\n")).unwrap(), "x\n");
        assert!(source_as_string(&Value::Null).is_err());
        assert!(source_as_string(&Value::List(vec![Value::Null])).is_err());
    }
}
