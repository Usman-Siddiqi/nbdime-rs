//! Document paths as nbdime writes them: `/cells/0/source`, and the starred form `/cells/*/source`
//! used to look up per-path configuration.

use crate::diff_format::Key;

/// First code point of every run of ten decimal digits (Unicode category Nd), which is what
/// Python's `\d` and `int()` accept in a `str`.
const DECIMAL_ZEROS: [u32; 68] = [
    0x30, 0x660, 0x6F0, 0x7C0, 0x966, 0x9E6, 0xA66, 0xAE6, 0xB66, 0xBE6, 0xC66, 0xCE6, 0xD66,
    0xDE6, 0xE50, 0xED0, 0xF20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90,
    0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10,
    0x104A0, 0x10D30, 0x11066, 0x110F0, 0x11136, 0x111D0, 0x112F0, 0x11450, 0x114D0, 0x11650,
    0x116C0, 0x11730, 0x118E0, 0x11950, 0x11C50, 0x11D50, 0x11DA0, 0x11F50, 0x16A60, 0x16AC0,
    0x16B50, 0x1D7CE, 0x1D7D8, 0x1D7E2, 0x1D7EC, 0x1D7F6, 0x1E140, 0x1E2F0, 0x1E4F0, 0x1E950,
    0x1FBF0,
];

/// The value of a Unicode decimal digit.
pub fn decimal_digit(c: char) -> Option<u32> {
    let c = u32::from(c);
    let run = DECIMAL_ZEROS
        .partition_point(|&zero| zero <= c)
        .checked_sub(1)?;
    let digit = c - DECIMAL_ZEROS[run];
    (digit < 10).then_some(digit)
}

/// The sign and decimal digits of `s` if it matches `^[-+]?\d+$`.
///
/// Python's `$` also matches before one final newline, which `int()` then ignores.
pub fn parse_int_string(s: &str) -> Option<(bool, String)> {
    let s = s.strip_suffix('\n').unwrap_or(s);
    let (negative, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty() {
        return None;
    }
    let digits = digits
        .chars()
        .map(|c| decimal_digit(c).and_then(|d| char::from_digit(d, 10)))
        .collect::<Option<String>>()?;
    Some((negative, digits))
}

/// `r_is_int.match(s)`, with `r_is_int = ^[-+]?\d+$`.
pub fn is_int_string(s: &str) -> bool {
    parse_int_string(s).is_some()
}

/// `split_path`: `/foo/bar` into `foo` and `bar`; empty parts are dropped.
pub fn split_path(path: &str) -> Vec<&str> {
    path.split('/').filter(|part| !part.is_empty()).collect()
}

/// `"/".join(parts)` with a leading slash. Parts that are `""` or `"/"` are dropped, which makes
/// distinct paths collide (`["a/b"]` and `["a", "b"]` give one string).
pub fn join_parts<S: AsRef<str>>(parts: &[S]) -> String {
    let kept: Vec<&str> = parts
        .iter()
        .map(AsRef::as_ref)
        .filter(|part| !part.is_empty() && *part != "/")
        .collect();
    let joined = kept.join("/");
    if joined.starts_with('/') {
        joined
    } else {
        format!("/{joined}")
    }
}

/// `join_path` for a path of keys.
pub fn join_path(path: &[Key]) -> String {
    let parts: Vec<String> = path.iter().map(Key::to_string).collect();
    join_parts(&parts)
}

/// `star_path`: indices, and keys that read as integers, become `*`.
pub fn star_path(path: &[Key]) -> String {
    let parts: Vec<&str> = path
        .iter()
        .map(|key| match key {
            Key::Name(name) if !is_int_string(name) => name.as_str(),
            _ => "*",
        })
        .collect();
    join_parts(&parts)
}

/// `star_path` of a path given as a string (what `Strategies.get` does to its argument).
pub fn star_path_str(path: &str) -> String {
    let parts: Vec<&str> = split_path(path)
        .into_iter()
        .map(|part| if is_int_string(part) { "*" } else { part })
        .collect();
    join_parts(&parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(items: &[&str]) -> Vec<Key> {
        items
            .iter()
            .map(|s| {
                s.parse::<usize>()
                    .map_or_else(|_| Key::from(*s), Key::Index)
            })
            .collect()
    }

    #[test]
    fn splits_and_joins() {
        assert_eq!(split_path("/cells//0/source/"), ["cells", "0", "source"]);
        assert_eq!(split_path("/"), Vec::<&str>::new());
        assert_eq!(join_path(&keys(&["cells", "3"])), "/cells/3");
        assert_eq!(join_path(&[]), "/");
        assert_eq!(
            join_path(&[Key::from(""), Key::from("/"), Key::from("a")]),
            "/a"
        );
        assert_eq!(join_path(&[Key::from("/x")]), "/x");
    }

    #[test]
    fn stars_integers_and_integer_strings() {
        assert_eq!(
            star_path(&keys(&["cells", "3", "source"])),
            "/cells/*/source"
        );
        assert_eq!(
            star_path(&[
                Key::from("a"),
                Key::from("12"),
                Key::from("+5"),
                Key::from("-7")
            ]),
            "/a/*/*/*"
        );
        assert_eq!(star_path(&[Key::from("1.5"), Key::from("")]), "/1.5");
        assert_eq!(star_path(&[]), "/");
        assert_eq!(star_path_str("//cells/7/id"), "/cells/*/id");
        assert_eq!(star_path_str("/"), "/");
    }

    #[test]
    fn integer_strings_follow_python() {
        assert!(is_int_string("007"));
        assert!(is_int_string("12\n"));
        assert!(!is_int_string("12\n\n"));
        assert!(!is_int_string("+"));
        assert!(!is_int_string("1_000"));
        assert!(!is_int_string(" 1"));
        // Arabic-Indic and fullwidth digits are decimal digits; superscripts are not.
        assert_eq!(
            parse_int_string("-\u{663}\u{660}"),
            Some((true, "30".into()))
        );
        assert_eq!(
            parse_int_string("\u{ff11}\u{ff12}"),
            Some((false, "12".into()))
        );
        assert!(!is_int_string("\u{b2}"));
        assert_eq!(decimal_digit('9'), Some(9));
        assert_eq!(decimal_digit('a'), None);
        assert_eq!(decimal_digit('\u{1d7ff}'), Some(9));
    }
}
