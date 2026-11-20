//! Reading notebooks for the command line tools (`nbdime.utils`).

use std::fs;

use anyhow::{anyhow, bail, Context, Result};
use nbdime_core::nbformat::{self, NbformatError};
use nbdime_core::{Map, Value};

/// The name of the null file, which stands for an added or removed notebook.
pub const EXPLICIT_MISSING_FILE: &str = "/dev/null";

/// A notebook given on the command line or found in a git revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Path(String),
    /// A file-like object: its name and its text.
    Stream {
        name: String,
        text: String,
    },
    /// Python's `None` (no base revision and no files).
    Nothing,
}

impl Source {
    /// `fn == EXPLICIT_MISSING_FILE`.
    pub fn is_missing_file(&self) -> bool {
        matches!(self, Source::Path(p) if p == EXPLICIT_MISSING_FILE)
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Source::Path(p) => Some(p),
            Source::Stream { name, .. } => Some(name),
            Source::Nothing => None,
        }
    }
}

/// What to return for the null file or an empty file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// An empty dict.
    Empty,
    /// A minimal valid notebook.
    Minimal,
}

impl Fallback {
    fn notebook(self) -> Value {
        match self {
            Fallback::Empty => Value::Object(Map::new()),
            Fallback::Minimal => nbformat::new_notebook(),
        }
    }
}

/// `read_notebook(f, on_null, on_empty)`.
pub fn read_notebook(
    source: &Source,
    on_null: Fallback,
    on_empty: Option<Fallback>,
) -> Result<Value> {
    let text = match source {
        Source::Nothing => bail!("cannot read a notebook from None"),
        Source::Path(path) if path == EXPLICIT_MISSING_FILE => return Ok(on_null.notebook()),
        Source::Path(path) => {
            let bytes =
                fs::read(crate::osstr::to_path(path)).with_context(|| format!("reading {path}"))?;
            String::from_utf8(bytes).map_err(|e| anyhow!("{path}: {e}"))?
        }
        Source::Stream { text, .. } => text.clone(),
    };
    match nbformat::reads(&text) {
        Ok(notebook) => Ok(notebook),
        Err(NbformatError::Json(error)) => match on_empty {
            // Only a file that is completely empty is replaced; anything else is not JSON.
            Some(fallback) if text.is_empty() || !matches!(source, Source::Path(_)) => {
                Ok(fallback.notebook())
            }
            _ => Err(anyhow!("Notebook does not appear to be JSON: {error}")),
        },
        Err(other) => Err(other.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, content: &str) -> String {
        let path =
            std::env::temp_dir().join(format!("nbdime-rs-utils-{}-{name}", std::process::id()));
        fs::write(&path, content).unwrap();
        path.to_str().unwrap().to_owned()
    }

    #[test]
    fn null_file_gives_the_fallback() {
        let null = Source::Path(EXPLICIT_MISSING_FILE.into());
        assert_eq!(
            read_notebook(&null, Fallback::Empty, None).unwrap(),
            Value::Object(Map::new())
        );
        let minimal = read_notebook(&null, Fallback::Minimal, None).unwrap();
        assert_eq!(minimal.get("nbformat"), Some(&Value::from(4)));
    }

    #[test]
    fn empty_file_needs_on_empty() {
        let path = temp_file("empty", "");
        let source = Source::Path(path.clone());
        assert!(read_notebook(&source, Fallback::Empty, None).is_err());
        assert!(read_notebook(&source, Fallback::Empty, Some(Fallback::Minimal)).is_ok());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn garbage_is_an_error_even_with_on_empty() {
        let path = temp_file("garbage", "{not json");
        let source = Source::Path(path.clone());
        assert!(read_notebook(&source, Fallback::Empty, Some(Fallback::Minimal)).is_err());
        let blank = temp_file("blank", " ");
        assert!(read_notebook(
            &Source::Path(blank.clone()),
            Fallback::Empty,
            Some(Fallback::Empty)
        )
        .is_err());
        fs::remove_file(path).unwrap();
        fs::remove_file(blank).unwrap();
    }

    #[test]
    fn streams_are_read_from_their_text() {
        let text = r#"{"nbformat": 4, "nbformat_minor": 5, "metadata": {}, "cells": []}"#;
        let source = Source::Stream {
            name: "a.ipynb (HEAD)".into(),
            text: text.into(),
        };
        assert!(read_notebook(&source, Fallback::Empty, None).is_ok());
        assert_eq!(source.name(), Some("a.ipynb (HEAD)"));
        assert!(read_notebook(&Source::Nothing, Fallback::Empty, None).is_err());
    }
}
