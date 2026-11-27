//! Python's text-mode newline translation on output: with the default `newline=None`, every
//! `"\n"` written to a file opened with `"w"` becomes `os.linesep`. Only Windows translates; a
//! `"\r\n"` already in the text becomes `"\r\r\n"` there, as in Python. The standard streams
//! are not translated (upstream's stdout is plain `"\n"` on Windows too).

use std::borrow::Cow;

/// Whether output newlines become `"\r\n"` (`os.linesep` is not `"\n"`).
pub const TRANSLATES: bool = cfg!(windows);

/// `text` as Python's text mode writes it on this platform.
pub fn native(text: &str) -> Cow<'_, str> {
    translate(text, TRANSLATES)
}

fn translate(text: &str, crlf: bool) -> Cow<'_, str> {
    if crlf && text.contains('\n') {
        Cow::Owned(text.replace('\n', "\r\n"))
    } else {
        Cow::Borrowed(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translation_matches_python_text_mode() {
        assert_eq!(translate("a\nb\r\nc", true), "a\r\nb\r\r\nc");
        assert_eq!(translate("a\nb", false), "a\nb");
        assert!(matches!(translate("no newline", true), Cow::Borrowed(_)));
    }
}
