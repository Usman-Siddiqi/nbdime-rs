//! Command line text and operating system strings that are not UTF-8 (see
//! [`nbdime_core::fsname`] for the encoding).

use std::ffi::OsStr;
use std::io::{self, Write};

use nbdime_core::fsname::for_output;
pub use nbdime_core::fsname::{from_bytes, to_bytes, to_os, to_path};

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

/// A name as `backslashreplace` shows it: each escaped byte as `\udcXX`.
///
/// Apply it where an operating system string (a file name, an argument) is formatted into
/// output, never to a whole stream: notebook text may contain the code points used for escaped
/// bytes.
pub fn show(name: &str) -> std::borrow::Cow<'_, str> {
    for_output(name)
}

/// `repr(text)`, with escaped bytes shown as the lone surrogates Python has for them.
pub fn repr(text: &str) -> String {
    let repr = nbdime_core::prettyprint::repr_str(text);
    if !repr.contains("\\U0010f7") {
        return repr;
    }
    let mut out = String::with_capacity(repr.len());
    let mut rest = repr.as_str();
    while let Some(at) = rest.find('\\') {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 1..];
        // `\\` is a literal backslash and must not start an escape of its own.
        let escaped = tail
            .strip_prefix("U0010f7")
            .and_then(|hex| hex.get(..2))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .filter(|byte| *byte >= 0x80);
        match escaped {
            Some(byte) => {
                out.push_str(&format!("\\udc{byte:02x}"));
                rest = &tail["U0010f7".len() + 2..];
            }
            None => {
                out.push('\\');
                let mut next = tail.chars();
                if let Some(c) = next.next() {
                    out.push(c);
                }
                rest = next.as_str();
            }
        }
    }
    out.push_str(rest);
    out
}

/// Writes text to stderr; failures are ignored. Python's standard streams write `"\n"` as is,
/// on Windows too; only files opened in text mode translate it.
pub fn eprint(text: &str) {
    let _ = io::stderr().lock().write_all(text.as_bytes());
}

/// `fs::canonicalize` without Windows' `\\?\` prefix, which git and Python's paths do not use.
pub fn canonicalize(path: impl AsRef<std::path::Path>) -> std::io::Result<std::path::PathBuf> {
    let canonical = std::fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        let text = canonical.to_string_lossy();
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            return Ok(format!(r"\\{unc}").into());
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return Ok(rest.into());
        }
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_shown_with_backslashreplace_and_text_is_left_alone() {
        assert_eq!(show(&from_bytes(b"x\xfey")), "x\\udcfey");
        assert_eq!(show("caf\u{e9}"), "caf\u{e9}");
    }

    #[test]
    fn repr_shows_escaped_bytes_but_not_a_literal_backslash_u() {
        assert_eq!(repr(&from_bytes(b"a\xfe")), "'a\\udcfe'");
        assert_eq!(repr("\\U0010f7fe"), "'\\\\U0010f7fe'");
        assert_eq!(repr(&format!("\\{}", from_bytes(b"\xfe"))), "'\\\\\\udcfe'");
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
