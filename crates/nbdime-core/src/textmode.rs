//! Python's text-mode newline translation on output: with the default `newline=None`, every
//! `"\n"` written to a text stream or a file opened with `"w"` becomes `os.linesep`. Only
//! Windows translates; a `"\r\n"` already in the text becomes `"\r\r\n"` there, as in Python.

use std::borrow::Cow;
use std::io::{self, Write};

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

/// A writer that applies the text-mode translation to everything written through it.
pub struct TextWriter<W> {
    inner: W,
    crlf: bool,
}

impl<W: Write> TextWriter<W> {
    pub fn new(inner: W) -> Self {
        Self::with_translation(inner, TRANSLATES)
    }

    fn with_translation(inner: W, crlf: bool) -> Self {
        TextWriter { inner, crlf }
    }

    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for TextWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !self.crlf {
            return self.inner.write(buf);
        }
        let mut lines = buf.split(|&b| b == b'\n');
        if let Some(first) = lines.next() {
            self.inner.write_all(first)?;
        }
        for line in lines {
            self.inner.write_all(b"\r\n")?;
            self.inner.write_all(line)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
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

    #[test]
    fn writer_translates_across_writes() {
        let mut w = TextWriter::with_translation(Vec::new(), true);
        w.write_all(b"one\ntwo").unwrap();
        w.write_all(b"\n\nthree\r\n").unwrap();
        assert_eq!(w.into_inner(), b"one\r\ntwo\r\n\r\nthree\r\r\n");

        let mut w = TextWriter::with_translation(Vec::new(), false);
        w.write_all(b"one\ntwo\r\n").unwrap();
        assert_eq!(w.into_inner(), b"one\ntwo\r\n");
    }
}
