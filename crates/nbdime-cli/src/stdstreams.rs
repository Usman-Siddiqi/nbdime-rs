//! What Python's standard streams do to nbdime's output on Windows.
//!
//! Python's `sys.stdout` and `sys.stderr` are text streams that write every `"\n"` as `"\r\n"`.
//! `setup_std_streams()` (called first thing by `nbdiff`, `nbshow`, `nbpatch`, `nbmerge` and
//! `git-nbdiffdriver`, however they are reached) replaces a stream whose error handler is
//! `strict` or `surrogateescape` with an encoder over its raw buffer, so `"\n"` stays `"\n"`.
//! That is stdout, unless `PYTHONIOENCODING` is set; stderr's handler is `backslashreplace`, so
//! it keeps its line endings. Both streams are then wrapped in colorama, which strips ANSI
//! escape sequences from a stream that is not a terminal (and renders them on one). Elsewhere
//! nothing changes.

use std::borrow::Cow;
#[cfg(windows)]
use std::io::IsTerminal;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

static SET_UP: AtomicBool = AtomicBool::new(false);
static RAW_STDOUT: AtomicBool = AtomicBool::new(false);

/// `nbdime.utils.setup_std_streams()`.
pub fn setup_std_streams() {
    if !SET_UP.swap(true, Ordering::Relaxed) {
        RAW_STDOUT.store(
            std::env::var_os("PYTHONIOENCODING").is_none_or(|v| v.is_empty()),
            Ordering::Relaxed,
        );
        #[cfg(windows)]
        enable_ansi_on_consoles();
    }
}

#[derive(Clone, Copy)]
enum Stream {
    Stdout,
    Stderr,
}

/// The bytes Python would emit for `text` written to the stream.
#[cfg(windows)]
fn render(text: &[u8], stream: Stream) -> Cow<'_, [u8]> {
    if !SET_UP.load(Ordering::Relaxed) {
        return crlf(text);
    }
    let (terminal, raw) = match stream {
        Stream::Stdout => (
            io::stdout().is_terminal(),
            RAW_STDOUT.load(Ordering::Relaxed),
        ),
        Stream::Stderr => (io::stderr().is_terminal(), false),
    };
    let text = if terminal {
        Cow::Borrowed(text)
    } else {
        strip_ansi(text)
    };
    match (raw, text) {
        (true, text) => text,
        (false, Cow::Borrowed(text)) => crlf(text),
        (false, Cow::Owned(text)) => Cow::Owned(crlf(&text).into_owned()),
    }
}

#[cfg(not(windows))]
fn render(text: &[u8], _stream: Stream) -> Cow<'_, [u8]> {
    Cow::Borrowed(text)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn crlf(text: &[u8]) -> Cow<'_, [u8]> {
    if !text.contains(&b'\n') {
        return Cow::Borrowed(text);
    }
    let mut out = Vec::with_capacity(text.len() + text.len() / 16);
    for &b in text {
        if b == b'\n' {
            out.push(b'\r');
        }
        out.push(b);
    }
    Cow::Owned(out)
}

/// colorama's `AnsiToWin32` stripping: CSI sequences `\x1b[` digits and `;` then a letter, and
/// OSC sequences `\x1b]` up to a BEL, each optionally wrapped in `\x01`...`\x02`.
#[cfg_attr(not(windows), allow(dead_code))]
fn strip_ansi(text: &[u8]) -> Cow<'_, [u8]> {
    if !text.contains(&0x1b) {
        return Cow::Borrowed(text);
    }
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        match sequence_len(&text[i..]) {
            Some(n) => i += n,
            None => {
                out.push(text[i]);
                i += 1;
            }
        }
    }
    Cow::Owned(out)
}

/// The length of the escape sequence at the start of `s`, if one starts there.
fn sequence_len(s: &[u8]) -> Option<usize> {
    let lead = usize::from(s.first() == Some(&0x01));
    let rest = &s[lead..];
    let body = match rest {
        [0x1b, b'[', tail @ ..] => {
            let params = tail
                .iter()
                .take_while(|b| b.is_ascii_digit() || **b == b';')
                .count();
            match tail.get(params) {
                Some(b) if b.is_ascii_alphabetic() => 2 + params + 1,
                _ => return None,
            }
        }
        [0x1b, b']', tail @ ..] => 2 + tail.iter().position(|&b| b == 0x07)? + 1,
        _ => return None,
    };
    let trail = usize::from(rest.get(body) == Some(&0x02));
    Some(lead + body + trail)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn enable_ansi_on_consoles() {
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    for id in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: plain Win32 calls on this process's standard handles; `mode` outlives them and
        // a handle that is not a console makes GetConsoleMode fail, which leaves it untouched.
        unsafe {
            let handle = GetStdHandle(id);
            let mut mode = 0;
            if GetConsoleMode(handle, &mut mode) != 0 {
                SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }
    }
}

/// Standard output as the Python process would write it.
pub struct Stdout<W>(pub W);

impl<W: Write> Write for Stdout<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write_all(&render(buf, Stream::Stdout))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// Writes `text` to standard error as the Python process would; failures are ignored.
pub fn eprint(text: &str) {
    let _ = io::stderr()
        .lock()
        .write_all(&render(text.as_bytes(), Stream::Stderr));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_is_python_text_mode() {
        assert_eq!(&*crlf(b"a\nb\r\n"), b"a\r\nb\r\r\n");
        assert!(matches!(crlf(b"none"), Cow::Borrowed(_)));
    }

    #[test]
    fn strips_what_colorama_strips() {
        assert_eq!(
            &*strip_ansi(b"\x1b[34m\x1b[1m## x:\x1b[0m\n\x1b[38;5;18;01mc\x1b[39;00m"),
            b"## x:\nc"
        );
        assert_eq!(&*strip_ansi(b"\x01\x1b[31m\x02r\x1b]0;title\x07!"), b"r!");
        // Not a complete sequence: kept.
        assert_eq!(&*strip_ansi(b"\x1b[3;\x1b[\x1b"), b"\x1b[3;\x1b[\x1b");
        assert_eq!(&*strip_ansi(b"\x1b[?25h"), b"\x1b[?25h");
        assert!(matches!(strip_ansi(b"plain"), Cow::Borrowed(_)));
    }
}
