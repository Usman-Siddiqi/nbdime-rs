//! A JSON serializer that produces exactly what Python's `json.dumps` produces.

use std::fmt::Write as _;

use super::scan::plain_prefix_len;
use crate::value::Value;

/// The subset of `json.dumps` keyword arguments nbdime uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpOptions {
    /// `indent=n`; `None` writes everything on one line.
    pub indent: Option<usize>,
    /// `separators=(item, key)`.
    pub item_separator: String,
    pub key_separator: String,
    pub ensure_ascii: bool,
    pub sort_keys: bool,
}

impl DumpOptions {
    /// `json.dumps(v)` defaults.
    pub fn python_default() -> Self {
        DumpOptions {
            indent: None,
            item_separator: ", ".into(),
            key_separator: ": ".into(),
            ensure_ascii: true,
            sort_keys: false,
        }
    }

    /// `json.dumps(v, separators=(",", ":"))`.
    pub fn compact() -> Self {
        DumpOptions {
            item_separator: ",".into(),
            key_separator: ":".into(),
            ..Self::python_default()
        }
    }

    /// `json.dumps(nb, indent=1, sort_keys=True, separators=(",", ": "), ensure_ascii=False)`,
    /// as `nbformat` writes notebooks.
    pub fn nbformat_write() -> Self {
        DumpOptions {
            indent: Some(1),
            item_separator: ",".into(),
            ensure_ascii: false,
            sort_keys: true,
            ..Self::python_default()
        }
    }

    /// `json.dump(d, f, indent=2, separators=(",", ": "))`, as `nbdiff --out` writes diffs.
    pub fn nbdime_diff_file() -> Self {
        DumpOptions {
            indent: Some(2),
            item_separator: ",".into(),
            ..Self::python_default()
        }
    }
}

pub fn dumps(value: &Value, options: &DumpOptions) -> String {
    let mut out = String::new();
    write_value(&mut out, value, options);
    out
}

pub fn write_value(out: &mut String, value: &Value, options: &DumpOptions) {
    enum Task<'a> {
        Value(&'a Value, usize),
        Close(char, usize),
        Separator(usize),
        Key(&'a str),
    }

    // Iterative to cope with deep values on small stacks.
    let mut tasks = vec![Task::Value(value, 0)];
    while let Some(task) = tasks.pop() {
        match task {
            Task::Key(k) => {
                write_string(out, k, options.ensure_ascii);
                out.push_str(&options.key_separator);
            }
            Task::Separator(level) => {
                out.push_str(&options.item_separator);
                newline(out, options, level);
            }
            Task::Close(c, level) => {
                newline(out, options, level);
                out.push(c);
            }
            Task::Value(v, level) => match v {
                Value::Null => out.push_str("null"),
                Value::Bool(true) => out.push_str("true"),
                Value::Bool(false) => out.push_str("false"),
                Value::Int(i) => {
                    let _ = write!(out, "{i}");
                }
                Value::Float(f) => write_float(out, *f),
                Value::Str(s) => write_string(out, s, options.ensure_ascii),
                Value::List(items) => {
                    if items.is_empty() {
                        out.push_str("[]");
                        continue;
                    }
                    out.push('[');
                    newline(out, options, level + 1);
                    tasks.push(Task::Close(']', level));
                    for (i, item) in items.iter().enumerate().rev() {
                        tasks.push(Task::Value(item, level + 1));
                        if i > 0 {
                            tasks.push(Task::Separator(level + 1));
                        }
                    }
                }
                Value::Object(map) => {
                    if map.is_empty() {
                        out.push_str("{}");
                        continue;
                    }
                    out.push('{');
                    newline(out, options, level + 1);
                    tasks.push(Task::Close('}', level));
                    let mut entries: Vec<(&String, &Value)> = map.iter().collect();
                    if options.sort_keys {
                        entries.sort_by(|a, b| a.0.cmp(b.0));
                    }
                    for (i, (k, item)) in entries.into_iter().enumerate().rev() {
                        tasks.push(Task::Value(item, level + 1));
                        tasks.push(Task::Key(k));
                        if i > 0 {
                            tasks.push(Task::Separator(level + 1));
                        }
                    }
                }
            },
        }
    }
}

fn newline(out: &mut String, options: &DumpOptions, level: usize) {
    if let Some(indent) = options.indent {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', indent * level));
    }
}

/// `json.encoder.(py_)encode_basestring(_ascii)`.
pub fn write_string(out: &mut String, s: &str, ensure_ascii: bool) {
    out.reserve(s.len() + 2);
    out.push('"');
    let bytes = s.as_bytes();
    // `s[start..at]` is pending output that needs no escaping. Every escaped character starts
    // with a byte this scan stops at, so `at` is always on a character boundary when it stops.
    let (mut start, mut at) = (0, 0);
    loop {
        // With ensure_ascii Python escapes everything outside ' '..='~', including DEL.
        at += plain_prefix_len(&bytes[at..], ensure_ascii);
        if at == bytes.len() {
            break;
        }
        out.push_str(&s[start..at]);
        let c = s[at..].chars().next().expect("a character starts here");
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c => {
                let mut units = [0u16; 2];
                for &unit in c.encode_utf16(&mut units).iter() {
                    push_unicode_escape(out, unit);
                }
            }
        }
        at += c.len_utf8();
        start = at;
    }
    out.push_str(&s[start..]);
    out.push('"');
}

/// `\uXXXX` with lowercase hex digits.
fn push_unicode_escape(out: &mut String, unit: u16) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push_str("\\u");
    for shift in [12, 8, 4, 0] {
        out.push(char::from(HEX[usize::from((unit >> shift) & 0xf)]));
    }
}

/// `float.__repr__` as used by `json.dumps`, including `NaN`/`Infinity`.
pub fn write_float(out: &mut String, f: f64) {
    if f.is_nan() {
        out.push_str("NaN");
    } else if f.is_infinite() {
        out.push_str(if f > 0.0 { "Infinity" } else { "-Infinity" });
    } else {
        out.push_str(&py_float_repr(f));
    }
}

/// Python's `repr(float)` for finite values: the shortest round-tripping digits, in positional
/// notation when the decimal exponent is in `[-4, 16)` and scientific notation otherwise.
pub fn py_float_repr(f: f64) -> String {
    debug_assert!(f.is_finite());
    let (digits, decpt) = shortest_digits(f.abs());
    let sign = if f.is_sign_negative() { "-" } else { "" };
    let n = digits.len() as i32;
    if (-4 < decpt) && (decpt <= 16) {
        if decpt <= 0 {
            format!("{sign}0.{}{digits}", "0".repeat((-decpt) as usize))
        } else if decpt >= n {
            format!("{sign}{digits}{}.0", "0".repeat((decpt - n) as usize))
        } else {
            let (int_part, frac) = digits.split_at(decpt as usize);
            format!("{sign}{int_part}.{frac}")
        }
    } else {
        let (first, rest) = digits.split_at(1);
        let e = decpt - 1;
        let esign = if e < 0 { '-' } else { '+' };
        if rest.is_empty() {
            format!("{sign}{first}e{esign}{:02}", e.abs())
        } else {
            format!("{sign}{first}.{rest}e{esign}{:02}", e.abs())
        }
    }
}

/// Shortest round-tripping decimal digits of a finite, non-negative `f`, and the position of the
/// decimal point (`f = 0.d1d2... * 10^decpt`), choosing like CPython's `dtoa` mode 0.
fn shortest_digits(f: f64) -> (String, i32) {
    // `{:e}` gives the shortest round-trip digits. When two candidates of that length are
    // equally close to `f`, it rounds the tie up; CPython rounds it to an even last digit.
    let (mut digits, exp) = split_exp(&format!("{f:e}"));
    let n = digits.len();
    // 767 significant digits is enough for any f64, so this expansion is exact.
    let (exact, exact_exp) = split_exp(&format!("{f:.800e}"));
    if exp == exact_exp && exact.len() > n {
        let (head, tail) = exact.split_at(n);
        let tie = tail.as_bytes()[0] == b'5' && tail.bytes().skip(1).all(|b| b == b'0');
        let last = head.as_bytes()[n - 1];
        // A tie below an odd 9 cannot occur: the rounded-up candidate would end in 0 and a
        // shorter representation would exist.
        if tie && last != b'9' {
            let mut chosen = head.as_bytes().to_vec();
            if (last - b'0') % 2 == 1 {
                chosen[n - 1] += 1;
            }
            digits = String::from_utf8(chosen).expect("ASCII digits");
        }
    }
    (digits, exp + 1)
}

/// Splits `d.ddde<exp>` into its digits and exponent.
fn split_exp(sci: &str) -> (String, i32) {
    let (mantissa, exp) = sci
        .split_once('e')
        .expect("LowerExp output has an exponent");
    let digits = mantissa.chars().filter(|c| *c != '.').collect();
    (
        digits,
        exp.parse().expect("LowerExp exponent is an integer"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    #[test]
    fn float_repr_matches_python() {
        let cases = [
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (0.1, "0.1"),
            (1e-4, "0.0001"),
            (1e-5, "1e-05"),
            (1.5e-7, "1.5e-07"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1.2345e16, "1.2345e+16"),
            (123456789012345.6, "123456789012345.6"),
            (1e300, "1e+300"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (0.30000000000000004, "0.30000000000000004"),
            (-2.5, "-2.5"),
        ];
        for (f, expected) in cases {
            assert_eq!(py_float_repr(f), expected, "{f:e}");
        }
        // Exact ties between two shortest candidates round to an even last digit.
        for (text, expected) in [
            ("73887011875394.125", "73887011875394.12"),
            ("-569584069112364.25", "-569584069112364.2"),
        ] {
            assert_eq!(py_float_repr(text.parse().unwrap()), expected);
        }
    }

    #[test]
    fn string_escaping() {
        let mut out = String::new();
        write_string(&mut out, "a\"\\\n\u{7f}é😀\u{1}", true);
        assert_eq!(out, "\"a\\\"\\\\\\n\\u007f\\u00e9\\ud83d\\ude00\\u0001\"");
        out.clear();
        write_string(&mut out, "a\u{7f}é😀\u{1}", false);
        assert_eq!(out, "\"a\u{7f}é😀\\u0001\"");
    }

    #[test]
    fn layouts() {
        let v = parse(
            r#"{"op": "addrange", "key": 0, "valuelist": [], "x": {}, "y": [1, {"z": null}]}"#,
        )
        .unwrap();
        assert_eq!(
            dumps(&v, &DumpOptions::python_default()),
            r#"{"op": "addrange", "key": 0, "valuelist": [], "x": {}, "y": [1, {"z": null}]}"#
        );
        assert_eq!(
            dumps(&v, &DumpOptions::nbdime_diff_file()),
            "{\n  \"op\": \"addrange\",\n  \"key\": 0,\n  \"valuelist\": [],\n  \"x\": {},\n  \"y\": [\n    1,\n    {\n      \"z\": null\n    }\n  ]\n}"
        );
        let sorted = DumpOptions {
            sort_keys: true,
            ..DumpOptions::compact()
        };
        assert_eq!(
            dumps(&parse(r#"{"b":1,"a":[2.5,NaN]}"#).unwrap(), &sorted),
            r#"{"a":[2.5,NaN],"b":1}"#
        );
    }
}
