//! Diffs of strings, by character and by line.

use crate::diff_format::{Diff, SequenceDiffBuilder};
use crate::difflib::{SequenceMatcher, Tag};
use crate::py::splitlines_keepends;
use crate::value::Value;

use super::config::{DiffConfig, Differ, Predicate};
use super::generic::diff_lists;

/// Character diff of two strings, indexed by code point. `addrange` values are strings.
pub fn diff_strings_by_char(a: &str, b: &str) -> Diff {
    if a == b {
        return Vec::new();
    }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut di = SequenceDiffBuilder::new();
    for op in SequenceMatcher::new(&a, &b).opcodes() {
        let inserted = || Value::Str(b[op.b.clone()].iter().collect());
        match op.tag {
            Tag::Equal => {}
            Tag::Replace => {
                di.removerange(op.a.start, op.a.len());
                di.addrange(op.a.start, inserted());
            }
            Tag::Insert => di.addrange(op.a.start, inserted()),
            Tag::Delete => di.removerange(op.a.start, op.a.len()),
        }
    }
    di.validated()
}

/// Line diff of two strings; lines are split like `str.splitlines(True)`, aligned approximately
/// and then diffed by character.
pub fn diff_strings_linewise(a: &str, b: &str) -> Diff {
    if a == b {
        return Vec::new();
    }
    let lines = |s: &str| -> Vec<Value> {
        splitlines_keepends(s)
            .into_iter()
            .map(Value::from)
            .collect()
    };
    let config = DiffConfig::new(
        vec![Predicate::StringsApproximate, Predicate::Eq],
        Differ::StringsByChar,
    );
    diff_lists(&lines(a), &lines(b), "", &config)
        .expect("the line-diff configuration only diffs strings")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_format::{diff_to_value, DiffEntry};
    use crate::json::{dumps, DumpOptions};

    fn json(d: &Diff) -> String {
        dumps(&diff_to_value(d), &DumpOptions::compact())
    }

    #[test]
    fn by_char_uses_code_points() {
        let d = diff_strings_by_char("\u{1F600}ab", "ab\u{1F600}");
        assert_eq!(
            d,
            [
                DiffEntry::removerange(0, 1),
                DiffEntry::addrange(3, Value::from("\u{1F600}")),
            ]
        );
    }

    #[test]
    fn by_char_replace_is_removerange_then_addrange() {
        assert_eq!(
            json(&diff_strings_by_char("abc", "axc")),
            r#"[{"op":"addrange","key":1,"valuelist":"x"},{"op":"removerange","key":1,"length":1}]"#
        );
        assert!(diff_strings_by_char("same", "same").is_empty());
        assert!(diff_strings_by_char("", "").is_empty());
    }

    #[test]
    fn linewise_patches_similar_lines() {
        let d = diff_strings_linewise("x = 1\ny = 2\n", "x = 1\ny = 3\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].key.as_index(), Some(1));
        assert!(matches!(d[0].op, crate::Op::Patch { .. }));
    }

    #[test]
    fn linewise_handles_all_line_boundaries() {
        let d = diff_strings_linewise("a\rb", "a\rb\u{2028}c");
        assert_eq!(
            json(&d),
            r#"[{"op":"addrange","key":1,"valuelist":["b\u2028","c"]},{"op":"removerange","key":1,"length":1}]"#
        );
    }

    #[test]
    fn linewise_from_and_to_empty() {
        assert_eq!(
            json(&diff_strings_linewise("", "a\n")),
            r#"[{"op":"addrange","key":0,"valuelist":["a\n"]}]"#
        );
        assert_eq!(
            json(&diff_strings_linewise("a\n", "")),
            r#"[{"op":"removerange","key":0,"length":1}]"#
        );
    }
}
