//! Command line text and operating system strings that are not UTF-8 (see
//! [`nbdime_core::fsname`] for the encoding).

use std::ffi::OsStr;
use std::io::{self, Write};

pub use nbdime_core::fsname::{for_output, from_bytes, to_bytes, to_os, to_path};

/// An operating system string (an argument, an environment variable) as text.
pub fn from_os(os: &OsStr) -> String {
    match os.to_str() {
        Some(text) => text.to_owned(),
        None => from_bytes(&os_bytes(os)),
    }
}

#[cfg(unix)]
fn os_bytes(os: &OsStr) -> std::borrow::Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt;
    std::borrow::Cow::Borrowed(os.as_bytes())
}

#[cfg(not(unix))]
fn os_bytes(os: &OsStr) -> std::borrow::Cow<'_, [u8]> {
    std::borrow::Cow::Owned(os.to_string_lossy().into_owned().into_bytes())
}

/// `repr(text)`, with escaped bytes shown as the lone surrogates Python has for them.
pub fn repr(text: &str) -> String {
    let repr = nbdime_core::prettyprint::repr_str(text);
    if !repr.contains("\\U0010f7") {
        return repr;
    }
    let mut repr = repr;
    for byte in 0x80..=0xFFu32 {
        repr = repr.replace(&format!("\\U0010f7{byte:02x}"), &format!("\\udc{byte:02x}"));
    }
    repr
}

/// Writes text to stderr like Python's `backslashreplace` handler; failures are ignored.
pub fn eprint(text: &str) {
    let _ = io::stderr().lock().write_all(for_output(text).as_bytes());
}

/// Stdout as nbdime sets it up (`setup_std_streams`): names with undecodable bytes appear as
/// `\udcXX`.
///
/// Every `write` call must carry whole characters, which `write_all` of a `str` does.
pub struct StdoutEscaped<W: Write>(pub W);

impl<W: Write> Write for StdoutEscaped<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // U+10F780.. start with 0xF4 in UTF-8; nothing else needs a look.
        if buf.contains(&0xF4) {
            if let Ok(text) = std::str::from_utf8(buf) {
                self.0.write_all(for_output(text).as_bytes())?;
                return Ok(buf.len());
            }
        }
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdout_escapes_the_bytes() {
        let mut out = StdoutEscaped(Vec::new());
        out.write_all(from_bytes(b"x\xfey\n").as_bytes()).unwrap();
        out.write_all("\u{e9}\n".as_bytes()).unwrap();
        assert_eq!(out.0, "x\\udcfey\n\u{e9}\n".as_bytes());
    }

    #[cfg(unix)]
    #[test]
    fn os_strings_are_decoded() {
        use std::os::unix::ffi::OsStrExt;
        let os = OsStr::from_bytes(b"m\xfe.ipynb");
        let text = from_os(os);
        assert_eq!(to_os(&text), os);
        assert_eq!(from_os(OsStr::new("ok")), "ok");
    }
}
