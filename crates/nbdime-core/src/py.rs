//! Python string semantics that nbdime relies on.

/// `str.splitlines(True)`: splits after every line boundary Python recognises, keeping it.
///
/// Boundaries are `\n`, `\r`, `\r\n`, `\v`, `\f`, `\x1c`, `\x1d`, `\x1e`, `\x85`, ` ` and
/// ` `. An empty string gives no lines.
pub fn splitlines_keepends(s: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let end = match c {
            '\r' => match chars.peek() {
                Some(&(j, '\n')) => {
                    chars.next();
                    j + 1
                }
                _ => i + 1,
            },
            c if is_line_boundary(c) => i + c.len_utf8(),
            _ => continue,
        };
        lines.push(&s[start..end]);
        start = end;
    }
    if start < s.len() {
        lines.push(&s[start..]);
    }
    lines
}

/// `str.isspace()` for one character: Unicode White_Space plus the separators `\x1c` to `\x1f`.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// `str.rstrip()`.
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

/// `str.splitlines()` (without line ends).
pub fn splitlines(s: &str) -> Vec<&str> {
    splitlines_keepends(s)
        .into_iter()
        .map(|line| {
            line.strip_suffix("\r\n")
                .or_else(|| {
                    line.chars()
                        .next_back()
                        .filter(|c| *c == '\r' || is_line_boundary(*c))
                        .map(|c| &line[..line.len() - c.len_utf8()])
                })
                .unwrap_or(line)
        })
        .collect()
}

/// Line boundaries other than `\r` (which may pair with a following `\n`).
fn is_line_boundary(c: char) -> bool {
    matches!(
        c,
        '\n' | '\u{b}'
            | '\u{c}'
            | '\u{1c}'
            | '\u{1d}'
            | '\u{1e}'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// `len(s)` in Python: the number of code points.
pub fn py_len(s: &str) -> usize {
    s.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rstrip_strips_python_whitespace() {
        assert_eq!(rstrip("a \t\n\x0b\x0c\r\x1c\x1f\u{85}\u{a0}\u{2003}"), "a");
        assert_eq!(rstrip(" a b"), " a b");
        assert_eq!(rstrip("\u{200b}"), "\u{200b}");
        assert_eq!(rstrip(""), "");
    }

    #[test]
    fn splitlines_matches_python() {
        // "a\rb\x0bc\x85d e\r\nf".splitlines(True)
        assert_eq!(
            splitlines_keepends("a\rb\u{b}c\u{85}d\u{2028}e\r\nf"),
            ["a\r", "b\u{b}", "c\u{85}", "d\u{2028}", "e\r\n", "f"]
        );
        assert!(splitlines_keepends("").is_empty());
        assert_eq!(splitlines_keepends("\n"), ["\n"]);
        assert_eq!(splitlines_keepends("\r\r\n\n"), ["\r", "\r\n", "\n"]);
        assert_eq!(splitlines_keepends("x\u{1e}"), ["x\u{1e}"]);
        assert_eq!(
            splitlines_keepends("tab\tnot\u{a0}boundary"),
            ["tab\tnot\u{a0}boundary"]
        );
    }

    #[test]
    fn splitlines_without_ends() {
        assert_eq!(splitlines("a\r\nb\rc\u{2029}d"), ["a", "b", "c", "d"]);
        assert_eq!(splitlines("a\n\n"), ["a", ""]);
    }
}
