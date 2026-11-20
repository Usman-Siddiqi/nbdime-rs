//! `pprint.pformat` for JSON values: the default width of 80, indent 1 and sorted dict keys.

use crate::py;
use crate::value::Value;

use super::pyrepr::{repr_str, safe_repr};

const WIDTH: i64 = 80;
const INDENT_PER_LEVEL: i64 = 1;

/// `pprint.pformat(value)`.
pub fn pformat(value: &Value) -> String {
    let mut out = String::new();
    emit(value, &mut out, 0, 0, 0);
    out
}

/// `len(s)` as an `i64`, for width arithmetic that may go negative.
fn width_of(s: &str) -> i64 {
    py::py_len(s) as i64
}

fn emit(value: &Value, out: &mut String, indent: i64, allowance: i64, level: usize) {
    let rep = safe_repr(value);
    let max_width = WIDTH - indent - allowance;
    if width_of(&rep) > max_width {
        match value {
            Value::Object(_) => return pprint_dict(value, out, indent, allowance, level + 1),
            Value::List(_) => return pprint_list(value, out, indent, allowance, level + 1),
            Value::Str(s) => return pprint_str(s, out, indent, allowance, level + 1),
            _ => {}
        }
    }
    out.push_str(&rep);
}

fn pprint_dict(value: &Value, out: &mut String, indent: i64, allowance: i64, level: usize) {
    let Value::Object(map) = value else {
        return;
    };
    out.push('{');
    if !map.is_empty() {
        let mut items: Vec<_> = map.iter().collect();
        items.sort_by(|a, b| a.0.cmp(b.0));
        let indent = indent + INDENT_PER_LEVEL;
        let delimiter = format!(",\n{}", " ".repeat(indent as usize));
        let last_index = items.len() - 1;
        for (i, (key, entry)) in items.into_iter().enumerate() {
            let last = i == last_index;
            let rep = repr_str(key);
            out.push_str(&rep);
            out.push_str(": ");
            emit(
                entry,
                out,
                indent + width_of(&rep) + 2,
                if last { allowance + 1 } else { 1 },
                level,
            );
            if !last {
                out.push_str(&delimiter);
            }
        }
    }
    out.push('}');
}

fn pprint_list(value: &Value, out: &mut String, indent: i64, allowance: i64, level: usize) {
    let Value::List(items) = value else {
        return;
    };
    out.push('[');
    let indent = indent + INDENT_PER_LEVEL;
    let delimiter = format!(",\n{}", " ".repeat(indent as usize));
    let last_index = items.len().saturating_sub(1);
    for (i, entry) in items.iter().enumerate() {
        let last = i == last_index;
        if i > 0 {
            out.push_str(&delimiter);
        }
        emit(
            entry,
            out,
            indent,
            if last { allowance + 1 } else { 1 },
            level,
        );
    }
    out.push(']');
}

/// Splits a line into `\S*\s*` runs (`re.findall(r'\S*\s*', line)` without its final empty match).
fn word_parts(line: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_spaces = false;
    for (i, c) in line.char_indices() {
        let space = py::is_space(c);
        if in_spaces && !space {
            parts.push(&line[start..i]);
            start = i;
        }
        in_spaces = space;
    }
    if start < line.len() {
        parts.push(&line[start..]);
    }
    parts
}

fn pprint_str(s: &str, out: &mut String, mut indent: i64, mut allowance: i64, level: usize) {
    if s.is_empty() {
        out.push_str(&repr_str(s));
        return;
    }
    let lines = py::splitlines_keepends(s);
    if level == 1 {
        indent += 1;
        allowance += 1;
    }
    let max_width = WIDTH - indent;
    let mut max_width1 = max_width;
    let mut chunks: Vec<String> = Vec::new();
    let mut rep = String::new();
    for (i, line) in lines.iter().enumerate() {
        let is_last_line = i == lines.len() - 1;
        rep = repr_str(line);
        if is_last_line {
            max_width1 -= allowance;
        }
        if width_of(&rep) <= max_width1 {
            chunks.push(rep.clone());
            continue;
        }
        let parts = word_parts(line);
        let mut max_width2 = max_width;
        let mut current = String::new();
        for (j, part) in parts.iter().enumerate() {
            if j == parts.len() - 1 && is_last_line {
                max_width2 -= allowance;
            }
            let candidate = format!("{current}{part}");
            if width_of(&repr_str(&candidate)) > max_width2 {
                if !current.is_empty() {
                    chunks.push(repr_str(&current));
                }
                current = (*part).to_owned();
            } else {
                current = candidate;
            }
        }
        if !current.is_empty() {
            chunks.push(repr_str(&current));
        }
    }
    if chunks.len() == 1 {
        out.push_str(&rep);
        return;
    }
    if level == 1 {
        out.push('(');
    }
    let delimiter = format!("\n{}", " ".repeat(indent.max(0) as usize));
    for (i, chunk) in chunks.iter().enumerate() {
        if i > 0 {
            out.push_str(&delimiter);
        }
        out.push_str(chunk);
    }
    if level == 1 {
        out.push(')');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    fn pf(text: &str) -> String {
        pformat(&parse(text).unwrap())
    }

    #[test]
    fn short_values_stay_on_one_line() {
        assert_eq!(pf("[1, 2, 3]"), "[1, 2, 3]");
        assert_eq!(
            pf(r#"{"b": 1, "a": [true, null]}"#),
            "{'a': [True, None], 'b': 1}"
        );
        assert_eq!(pf("[]"), "[]");
        assert_eq!(pf("2.5"), "2.5");
        assert_eq!(pf("null"), "None");
    }

    #[test]
    fn long_lists_wrap_one_item_per_line() {
        let text = format!(
            "[{}]",
            (0..30)
                .map(|i| (i * 1000).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let out = pf(&text);
        assert!(out.starts_with("[0,\n 1000,\n 2000,"));
        assert!(out.ends_with(",\n 29000]"));
    }

    #[test]
    fn long_dict_values_wrap_under_the_key() {
        let long = "word ".repeat(30);
        assert_eq!(
            pf(&format!(r#"{{"key": "{long}", "z": 1}}"#)),
            "{'key': 'word word word word word word word word word word word word word word '\n        \
             'word word word word word word word word word word word word word word '\n        \
             'word word ',\n 'z': 1}"
        );
    }

    #[test]
    fn a_top_level_string_is_parenthesised() {
        assert_eq!(
            pformat(&Value::from("ab ".repeat(40))),
            "('ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab '\n \
             'ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ')"
        );
    }

    #[test]
    fn strings_in_containers_are_split_into_implicit_concatenations() {
        assert_eq!(
            pformat(&Value::List(vec![Value::from("ab ".repeat(40))])),
            "['ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab '\n \
             'ab ab ab ab ab ab ab ab ab ab ab ab ab ab ab ']"
        );
    }

    #[test]
    fn multi_line_strings_split_at_line_ends() {
        let out = pformat(&Value::List(vec![Value::from("a\n".repeat(50))]));
        assert!(out.starts_with("['a\\n'\n 'a\\n'\n 'a\\n'"));
        assert_eq!(out.lines().count(), 50);
    }

    #[test]
    fn unbreakable_words_stay_whole() {
        let out = pformat(&Value::List(vec![Value::from("x".repeat(100))]));
        assert_eq!(out, format!("['{}']", "x".repeat(100)));
    }

    #[test]
    fn word_parts_alternate_words_and_spaces() {
        assert_eq!(word_parts("ab  cd e"), ["ab  ", "cd ", "e"]);
        assert_eq!(word_parts("  ab"), ["  ", "ab"]);
        assert_eq!(word_parts("ab\n"), ["ab\n"]);
        assert!(word_parts("").is_empty());
    }
}
