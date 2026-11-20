//! The terminal pretty-printer for diffs and merges (`nbdime.prettyprint`).
//!
//! [`Printer`] writes what upstream writes to `config.out`, byte for byte, including the colorama
//! escape sequences. Modified multi-line strings are rendered by [`render::diff_render`], which
//! spawns `git diff --no-index` or `diff` like upstream does.
//!
//! Syntax highlighting of notebook sources beyond Markdown cells (`pretty_print_notebook`,
//! `nbshow`) is not part of this module.

use std::borrow::Cow;
use std::fs;
use std::time::UNIX_EPOCH;

use chrono::{Datelike, Local, TimeZone, Timelike};
use md5::{Digest, Md5};

use crate::patch::PatchError;
use crate::path::star_path_str;
use crate::py;
use crate::value::Value;

mod highlight;
mod pformat;
mod printer;
mod py_tables;
mod pyrepr;
pub mod render;

pub use pformat::pformat;
pub use printer::Printer;
pub use pyrepr::{is_printable, py_str, repr, repr_float, repr_str};
pub use render::RenderError;

/// Indentation offset in pretty-print.
pub const IND: &str = "  ";

/// Max line width used in some places in pretty-print.
pub const MAXWIDTH: usize = 78;

/// What printing can fail on: values of a kind upstream cannot print (it raises there), diffs
/// that do not apply, or the external renderers.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PrettyPrintError {
    /// A value, path or diff of the wrong kind (`NBDiffFormatError`, `TypeError`, `KeyError`,
    /// `AttributeError`, `ValueError` and `AssertionError` upstream).
    #[error("{0}")]
    Invalid(String),
    /// `use_color` is not a key of upstream's colour table (`KeyError` or `TypeError`).
    #[error("invalid use_color value")]
    InvalidColor,
    /// A value nested more deeply than upstream's recursion limit allows.
    #[error("value is nested more than {MAX_DEPTH} levels deep")]
    TooDeep,
    /// Something upstream does that this port does not (yet) do.
    #[error("{0}")]
    Unsupported(String),
    #[error(transparent)]
    Patch(#[from] PatchError),
    #[error(transparent)]
    Render(#[from] RenderError),
}

/// Nesting beyond which values are rejected instead of risking the stack (cf. findings F-009).
pub const MAX_DEPTH: usize = 500;

pub(crate) fn invalid<T>(msg: impl Into<String>) -> Result<T, PrettyPrintError> {
    Err(PrettyPrintError::Invalid(msg.into()))
}

/// What to print and how: the `diff_ignorables` include flags, and the options of the text
/// renderers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrettyPrintConfig {
    pub sources: bool,
    pub outputs: bool,
    pub attachments: bool,
    pub metadata: bool,
    pub id: bool,
    pub details: bool,
    pub color_words: bool,
    pub use_git: bool,
    pub use_diff: bool,
    /// Truthiness of `use_color`.
    pub use_color: bool,
    /// `use_color` is something other than a boolean (or 0 or 1), so that upstream's `col_const`
    /// lookup raises `KeyError` the first time a colour is needed: the colour accessors then
    /// return [`COLOR_ERROR`].
    pub invalid_color: bool,
}

/// What the colour accessors return when `use_color` is invalid; [`Printer`] stops at the write
/// that contains it.
pub(crate) const COLOR_ERROR: &str = "\u{10FFFF}";

impl Default for PrettyPrintConfig {
    fn default() -> Self {
        PrettyPrintConfig {
            sources: true,
            outputs: true,
            attachments: true,
            metadata: true,
            id: true,
            details: true,
            color_words: false,
            use_git: true,
            use_diff: true,
            use_color: true,
            invalid_color: false,
        }
    }
}

impl PrettyPrintConfig {
    /// Whether `path` (`/cells/3/source`, ...) belongs to a part of the notebook that is not
    /// shown.
    pub fn should_ignore_path(&self, path: &str) -> bool {
        let starred = star_path_str(path);
        if starred.starts_with("/cells/*/source") {
            return !self.sources;
        }
        if starred.starts_with("/cells/*/attachments") {
            return !self.attachments;
        }
        if starred.starts_with("/cells/*/metadata") || starred.starts_with("/metadata") {
            return !self.metadata;
        }
        if starred.starts_with("/cells/*/id") {
            return !self.id;
        }
        if starred.starts_with("/cells/*/outputs") {
            return !self.outputs
                || (starred == "/cells/*/outputs/*/execution_count" && !self.details);
        }
        // Everything under /cells/ that is not handled above (all other sub-keys are known).
        if starred.starts_with("/cells/*/") || starred.starts_with("/nbformat") {
            return !self.details;
        }
        false
    }

    pub fn keep(&self) -> &'static str {
        if self.invalid_color {
            COLOR_ERROR
        } else {
            "   "
        }
    }

    pub fn remove(&self) -> &'static str {
        if self.invalid_color {
            COLOR_ERROR
        } else if self.use_color {
            "\x1b[31m-  "
        } else {
            "-  "
        }
    }

    pub fn add(&self) -> &'static str {
        if self.invalid_color {
            COLOR_ERROR
        } else if self.use_color {
            "\x1b[32m+  "
        } else {
            "+  "
        }
    }

    pub fn info(&self) -> &'static str {
        if self.invalid_color {
            COLOR_ERROR
        } else if self.use_color {
            "\x1b[34m\x1b[1m## "
        } else {
            "## "
        }
    }

    pub fn reset(&self) -> &'static str {
        if self.invalid_color {
            COLOR_ERROR
        } else if self.use_color {
            "\x1b[0m"
        } else {
            ""
        }
    }
}

/// The modification time of `filename` as `datetime.fromtimestamp(mtime).isoformat(" ")`, or
/// `(no timestamp)` if there is no such file.
pub fn file_timestamp(filename: &str) -> Result<String, PrettyPrintError> {
    let Ok(modified) = fs::metadata(crate::fsname::to_path(filename)).and_then(|m| m.modified())
    else {
        return Ok("(no timestamp)".into());
    };
    // `os.path.getmtime` is the float `sec + 1e-9 * nsec`, with a fraction that counts forward.
    let (sec, nsec) = match modified.duration_since(UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64, d.subsec_nanos()),
        Err(e) => {
            let d = e.duration();
            match d.subsec_nanos() {
                0 => (-(d.as_secs() as i64), 0),
                n => (-(d.as_secs() as i64) - 1, 1_000_000_000 - n),
            }
        }
    };
    let mtime = sec as f64 + 1e-9 * f64::from(nsec);
    // `datetime.fromtimestamp` rounds the fraction to whole microseconds, ties to even.
    let whole = mtime.trunc();
    let mut micros = ((mtime - whole) * 1e6).round_ties_even();
    let mut whole = whole;
    if micros >= 1e6 {
        micros -= 1e6;
        whole += 1.0;
    } else if micros < 0.0 {
        micros += 1e6;
        whole -= 1.0;
    }
    let out_of_range = || PrettyPrintError::Invalid(format!("timestamp out of range: {mtime}"));
    let dt = Local
        .timestamp_opt(whole as i64, 0)
        .single()
        .ok_or_else(out_of_range)?;
    if !(1..=9999).contains(&dt.year()) {
        return Err(out_of_range());
    }
    let micros = micros as u32;
    let mut text = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second()
    );
    if micros != 0 {
        text.push_str(&format!(".{micros:06}"));
    }
    Ok(text)
}

/// `hash_string`: the hex MD5 digest of the UTF-8 encoding of `s`.
pub fn hash_string(s: &str) -> String {
    Md5::digest(s.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Whether `text` matches `^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$`
/// (with no newline in it, `$` can only match at the end).
fn is_base64(text: &str) -> bool {
    let (body, remainder) = if let Some(body) = text.strip_suffix("==") {
        (body, 2)
    } else if let Some(body) = text.strip_suffix('=') {
        (body, 3)
    } else {
        (text, 0)
    };
    body.len() % 4 == remainder
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

/// `_trim_base64`: long base64 data (possibly wrapped at newlines) is replaced by its first
/// eight characters and a digest.
pub fn trim_base64(s: &str) -> Cow<'_, str> {
    // Only `\n` is removed before matching: any other line break keeps a string from matching.
    if py::py_len(s) > 64 && is_base64(&s.replace('\n', "")) {
        let digest = hash_string(s);
        let head: String = s.chars().take(8).collect();
        Cow::Owned(format!("{head}...<snip base64, md5={}...>", &digest[..16]))
    } else {
        Cow::Borrowed(s)
    }
}

/// `format_value`: a simple value for printing. Strings are snipped if they are base64 data and
/// otherwise kept as they are; everything else is formatted by `pprint`.
pub fn format_value(value: &Value) -> Result<String, PrettyPrintError> {
    match value {
        Value::Str(s) => Ok(trim_base64(s).into_owned()),
        other => {
            if other.nesting_exceeds(MAX_DEPTH) {
                return Err(PrettyPrintError::TooDeep);
            }
            Ok(pformat(other))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_colors() {
        let colored = PrettyPrintConfig::default();
        assert_eq!(colored.remove(), "\x1b[31m-  ");
        assert_eq!(colored.add(), "\x1b[32m+  ");
        assert_eq!(colored.info().replace("##", "===="), "\x1b[34m\x1b[1m==== ");
        assert_eq!(colored.reset(), "\x1b[0m");
        let plain = PrettyPrintConfig {
            use_color: false,
            invalid_color: false,
            ..colored
        };
        assert_eq!(
            [
                plain.keep(),
                plain.remove(),
                plain.add(),
                plain.info(),
                plain.reset()
            ],
            ["   ", "-  ", "+  ", "## ", ""]
        );
    }

    #[test]
    fn ignored_paths() {
        let all = PrettyPrintConfig::default();
        assert!(!all.should_ignore_path("/cells/0/source"));
        let none = PrettyPrintConfig {
            sources: false,
            outputs: false,
            attachments: false,
            metadata: false,
            id: false,
            details: false,
            ..all.clone()
        };
        for path in [
            "/cells/0/source",
            "/cells/3/attachments/x",
            "/cells/0/metadata/tags",
            "/metadata/kernelspec",
            "/cells/1/id",
            "/cells/1/outputs/0/text",
            "/cells/1/execution_count",
            "/nbformat",
            "/nbformat_minor",
        ] {
            assert!(none.should_ignore_path(path), "{path}");
        }
        for path in ["/cells", "/cells/0", "/other", "//"] {
            assert!(!none.should_ignore_path(path), "{path}");
        }
        let no_details = PrettyPrintConfig {
            details: false,
            ..all
        };
        assert!(no_details.should_ignore_path("/cells/0/outputs/2/execution_count"));
        assert!(!no_details.should_ignore_path("/cells/0/outputs/2/text"));
    }

    #[test]
    fn digest_of_utf8() {
        assert_eq!(hash_string(""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hash_string("abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn base64_is_snipped_when_longer_than_64() {
        let data = "QUJD".repeat(17);
        assert_eq!(data.len(), 68);
        let out = trim_base64(&data);
        assert!(out.starts_with("QUJDQUJD...<snip base64, md5="));
        assert!(out.ends_with("...>"));
        assert_eq!(trim_base64(&"QUJD".repeat(16)), "QUJD".repeat(16));
        assert_eq!(
            trim_base64("not base64 ".repeat(10).as_str()),
            "not base64 ".repeat(10)
        );
    }

    #[test]
    fn base64_may_be_wrapped_at_newlines_only() {
        let wrapped = format!("{}\n{}\n", "QUJD".repeat(12), "QUJD".repeat(12));
        assert!(trim_base64(&wrapped).contains("snip"));
        let crlf = format!("{}\r\n{}\r\n", "QUJD".repeat(12), "QUJD".repeat(12));
        assert!(!trim_base64(&crlf).contains("snip"));
    }

    #[test]
    fn base64_padding() {
        assert!(is_base64("QQ=="));
        assert!(is_base64("QUI="));
        assert!(is_base64("QUJD"));
        assert!(is_base64(""));
        assert!(!is_base64("QUJDQQ"));
        assert!(!is_base64("QUJD=="));
        assert!(!is_base64("QUJ"));
        assert!(!is_base64("QU=D"));
        assert!(!is_base64("QUJ\u{e9}"));
    }

    #[test]
    fn timestamp_of_a_missing_file() {
        assert_eq!(file_timestamp("/no/such/file").unwrap(), "(no timestamp)");
    }

    #[test]
    fn timestamps_show_microseconds_only_when_there_are_some() {
        use std::time::Duration;
        let dir = std::env::temp_dir().join(format!("nbdime-ts-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let stamp = |nanos: u32| {
            let path = dir.join("f");
            let file = fs::File::create(&path).unwrap();
            file.set_modified(UNIX_EPOCH + Duration::new(1_700_000_000, nanos))
                .unwrap();
            file_timestamp(path.to_str().unwrap()).unwrap()
        };
        let whole = stamp(0);
        assert_eq!(whole.len(), 19, "{whole}");
        assert_eq!(&stamp(250_000_000)[19..], ".250000");
        // The float `sec + 1e-9 * nsec` rounds to the nearest microsecond, ties to even.
        assert_eq!(&stamp(1_000)[19..], ".000001");
        assert_eq!(stamp(400), whole);
        assert_eq!(&stamp(999_999_400)[19..], ".999999");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn timestamp_of_a_file_is_iso_with_a_space() {
        let stamp = file_timestamp("Cargo.toml").unwrap();
        let (date, time) = stamp.split_once(' ').unwrap();
        assert_eq!(date.len(), 10);
        assert!(time.len() == 8 || time.len() == 15, "{stamp}");
    }

    #[test]
    fn format_value_formats_non_strings_with_pprint() {
        assert_eq!(format_value(&Value::from("a\nb")).unwrap(), "a\nb");
        assert_eq!(format_value(&Value::Bool(true)).unwrap(), "True");
        assert_eq!(format_value(&Value::List(vec![])).unwrap(), "[]");
    }
}
