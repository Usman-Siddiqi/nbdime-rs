//! File names and command line text that are not valid UTF-8.
//!
//! Python decodes them with `surrogateescape`: every undecodable byte `0xXX` becomes the lone
//! surrogate `U+DCXX`. Rust strings cannot hold surrogates, so the same bytes map to the code
//! points `U+10F780..=U+10F7FF` (plane 16 private use) instead. The text flows through the
//! argument parser and the printers like any other, and the original bytes come back when a name
//! reaches the file system ([`to_path`]); output shows `\udcXX`, as the `backslashreplace` error
//! handler of nbdime's standard streams does. A genuine character in that private-use range is
//! indistinguishable from an escaped byte.

use std::borrow::Cow;
use std::ffi::OsString;
use std::path::PathBuf;

const BASE: u32 = 0x10F700;
const FIRST: u32 = BASE + 0x80;
const LAST: u32 = BASE + 0xFF;

fn escaped(c: char) -> Option<u8> {
    let c = c as u32;
    (FIRST..=LAST).contains(&c).then(|| (c - BASE) as u8)
}

fn escape(byte: u8) -> char {
    char::from_u32(BASE + u32::from(byte)).expect("a private-use code point")
}

/// `bytes.decode("utf-8", "surrogateescape")`.
pub fn from_bytes(mut bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len());
    loop {
        match std::str::from_utf8(bytes) {
            Ok(rest) => {
                text.push_str(rest);
                return text;
            }
            Err(error) => {
                let (valid, rest) = bytes.split_at(error.valid_up_to());
                text.push_str(std::str::from_utf8(valid).expect("validated prefix"));
                let bad = error.error_len().unwrap_or(rest.len());
                text.extend(rest[..bad].iter().map(|b| escape(*b)));
                bytes = &rest[bad..];
            }
        }
    }
}

/// `os.fsencode(text)`: the bytes of a name, with escaped bytes restored.
pub fn to_bytes(text: &str) -> Cow<'_, [u8]> {
    if !text.chars().any(|c| escaped(c).is_some()) {
        return Cow::Borrowed(text.as_bytes());
    }
    let mut bytes = Vec::with_capacity(text.len());
    let mut buf = [0u8; 4];
    for c in text.chars() {
        match escaped(c) {
            Some(byte) => bytes.push(byte),
            None => bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes()),
        }
    }
    Cow::Owned(bytes)
}

/// The operating system string for a name.
pub fn to_os(text: &str) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(to_bytes(text).into_owned())
    }
    #[cfg(not(unix))]
    {
        OsString::from(text)
    }
}

pub fn to_path(text: &str) -> PathBuf {
    PathBuf::from(to_os(text))
}

/// What the `backslashreplace` handler makes of text on a standard stream.
pub fn for_output(text: &str) -> Cow<'_, str> {
    if !text.chars().any(|c| escaped(c).is_some()) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.chars()
            .map(|c| match escaped(c) {
                Some(byte) => format!("\\udc{byte:02x}"),
                None => c.to_string(),
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_bytes_round_trip() {
        let bytes = b"a\xff.ipynb\xc3\xa9\xe2\x82";
        let text = from_bytes(bytes);
        assert_eq!(text.chars().count(), 1 + 1 + 6 + 1 + 2);
        assert_eq!(to_bytes(&text).as_ref(), bytes);
        assert_eq!(for_output(&text), "a\\udcff.ipynb\u{e9}\\udce2\\udc82");
        assert_eq!(from_bytes(b"plain"), "plain");
    }
}
