//! `repr` and `str` of JSON values as Python prints them.

use std::borrow::Cow;
use std::fmt::Write;

use crate::json::py_float_repr;
use crate::value::Value;

use super::py_tables::NON_PRINTABLE;

/// Whether `c` lies in one of the inclusive `start, end` pairs of `table`.
pub(super) fn in_ranges(table: &[u32], c: char) -> bool {
    let c = u32::from(c);
    let pairs = table.len() / 2;
    let (mut lo, mut hi) = (0, pairs);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if table[2 * mid + 1] < c {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo < pairs && table[2 * lo] <= c
}

/// `str.isprintable()` for one character.
pub fn is_printable(c: char) -> bool {
    !in_ranges(&NON_PRINTABLE, c)
}

/// `repr(s)`.
pub fn repr_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    push_repr_str(&mut out, s);
    out
}

fn push_repr_str(out: &mut String, s: &str) {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            ' '..='~' => out.push(c),
            c if is_printable(c) && !c.is_ascii() => out.push(c),
            c => {
                let cp = u32::from(c);
                let _ = if cp <= 0xff {
                    write!(out, "\\x{cp:02x}")
                } else if cp <= 0xffff {
                    write!(out, "\\u{cp:04x}")
                } else {
                    write!(out, "\\U{cp:08x}")
                };
            }
        }
    }
    out.push(quote);
}

/// `repr(f)` for any float.
pub fn repr_float(f: f64) -> String {
    if f.is_nan() {
        "nan".into()
    } else if f.is_infinite() {
        if f > 0.0 { "inf" } else { "-inf" }.into()
    } else {
        py_float_repr(f)
    }
}

/// `repr(value)`: dicts keep their insertion order.
pub fn repr(value: &Value) -> String {
    let mut out = String::new();
    push_repr(&mut out, value, false);
    out
}

/// `repr` of a value as `pprint` builds it with `sort_dicts=True`: dict keys in code point order.
pub(super) fn safe_repr(value: &Value) -> String {
    let mut out = String::new();
    push_repr(&mut out, value, true);
    out
}

fn push_repr(out: &mut String, value: &Value, sort_keys: bool) {
    match value {
        Value::Null => out.push_str("None"),
        Value::Bool(true) => out.push_str("True"),
        Value::Bool(false) => out.push_str("False"),
        Value::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Value::Float(f) => out.push_str(&repr_float(*f)),
        Value::Str(s) => push_repr_str(out, s),
        Value::List(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                push_repr(out, item, sort_keys);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            let mut entries: Vec<_> = map.iter().collect();
            if sort_keys {
                entries.sort_by(|a, b| a.0.cmp(b.0));
            }
            for (i, (key, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                push_repr_str(out, key);
                out.push_str(": ");
                push_repr(out, item, sort_keys);
            }
            out.push('}');
        }
    }
}

/// `str(value)`: strings as they are, everything else as `repr`.
pub fn py_str(value: &Value) -> Cow<'_, str> {
    match value {
        Value::Str(s) => Cow::Borrowed(s),
        other => Cow::Owned(repr(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    fn r(text: &str) -> String {
        repr(&parse(text).unwrap())
    }

    #[test]
    fn string_quoting() {
        assert_eq!(repr_str("abc"), "'abc'");
        assert_eq!(repr_str("it's"), "\"it's\"");
        assert_eq!(repr_str("say \"hi\""), "'say \"hi\"'");
        assert_eq!(repr_str("both ' and \""), "'both \\' and \"'");
        assert_eq!(repr_str("back\\slash"), "'back\\\\slash'");
    }

    #[test]
    fn string_escapes() {
        assert_eq!(repr_str("a\tb\nc\rd"), "'a\\tb\\nc\\rd'");
        assert_eq!(repr_str("\x00\x1f\x7f"), "'\\x00\\x1f\\x7f'");
        assert_eq!(repr_str("\u{85}\u{a0}\u{ad}"), "'\\x85\\xa0\\xad'");
        assert_eq!(
            repr_str("\u{2028}\u{200b}\u{feff}"),
            "'\\u2028\\u200b\\ufeff'"
        );
        assert_eq!(
            repr_str("\u{e9}\u{4e2d}\u{1f600}"),
            "'\u{e9}\u{4e2d}\u{1f600}'"
        );
        assert_eq!(repr_str("\u{e0000}\u{10ffff}"), "'\\U000e0000\\U0010ffff'");
        // Unassigned in Unicode 15.1, so not printable.
        assert_eq!(repr_str("\u{378}"), "'\\u0378'");
    }

    #[test]
    fn printable_follows_the_general_category() {
        assert!(is_printable(' '));
        assert!(is_printable('a'));
        assert!(!is_printable('\u{a0}'));
        assert!(!is_printable('\u{50000}'));
        assert!(is_printable('\u{3134a}'));
        assert!(!is_printable('\u{3134b}'));
    }

    #[test]
    fn tables_are_sorted_ranges_and_space_agrees_with_the_string_helpers() {
        use super::super::py_tables::{SPACE, WORD};
        for table in [&NON_PRINTABLE[..], &SPACE[..], &WORD[..]] {
            assert!(table.chunks_exact(2).all(|p| p[0] <= p[1]));
            assert!(table.windows(3).step_by(2).all(|w| w[1] < w[2]));
        }
        for c in (0..=0x10ffff).filter_map(char::from_u32) {
            assert_eq!(in_ranges(&SPACE, c), crate::py::is_space(c), "{c:?}");
        }
        assert!(in_ranges(&WORD, '_') && in_ranges(&WORD, '\u{b2}') && !in_ranges(&WORD, '-'));
    }

    #[test]
    fn floats() {
        assert_eq!(repr_float(f64::NAN), "nan");
        assert_eq!(repr_float(-f64::NAN), "nan");
        assert_eq!(repr_float(f64::INFINITY), "inf");
        assert_eq!(repr_float(f64::NEG_INFINITY), "-inf");
        assert_eq!(repr_float(-0.0), "-0.0");
        assert_eq!(repr_float(1e16), "1e+16");
        assert_eq!(repr_float(1e-5), "1e-05");
    }

    #[test]
    fn containers_keep_insertion_order() {
        assert_eq!(
            r(r#"{"b": 1, "a": [true, null, 2.5, "x"]}"#),
            "{'b': 1, 'a': [True, None, 2.5, 'x']}"
        );
        assert_eq!(r("[]"), "[]");
        assert_eq!(r("{}"), "{}");
        assert_eq!(
            r("123456789012345678901234567890"),
            "123456789012345678901234567890"
        );
    }

    #[test]
    fn safe_repr_sorts_dict_keys() {
        let v = parse(r#"{"b": {"z": 1, "y": 2}, "a": 0}"#).unwrap();
        assert_eq!(safe_repr(&v), "{'a': 0, 'b': {'y': 2, 'z': 1}}");
    }

    #[test]
    fn str_of_values() {
        assert_eq!(py_str(&Value::from("a")), "a");
        assert_eq!(py_str(&Value::Null), "None");
        assert_eq!(py_str(&Value::from(3)), "3");
        assert_eq!(py_str(&parse("[\"a\"]").unwrap()), "['a']");
    }
}
