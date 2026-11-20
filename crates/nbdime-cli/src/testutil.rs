//! Helpers for the unit tests of the commands.

use std::fs;
use std::path::PathBuf;

/// A directory removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("nbdime-rs-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    pub fn path(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_owned()
    }

    /// Writes a file and returns its path.
    pub fn write(&self, name: &str, content: &str) -> String {
        let path = self.path(name);
        fs::write(&path, content).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A minimal notebook with one code cell per source.
pub fn notebook(sources: &[&str]) -> String {
    let cells: Vec<String> = sources
        .iter()
        .enumerate()
        .map(|(i, source)| {
            format!(
                r#"{{"cell_type": "code", "execution_count": null, "id": "cell{i:04}", "metadata": {{}}, "outputs": [], "source": ["{source}"]}}"#
            )
        })
        .collect();
    format!(
        r#"{{"cells": [{}], "metadata": {{}}, "nbformat": 4, "nbformat_minor": 5}}"#,
        cells.join(", ")
    )
}

type App = fn(&str, &[String], &mut dyn std::io::Write) -> anyhow::Result<i32>;

/// Runs a command and returns its exit status and what it wrote to stdout.
pub fn run_app(app: App, prog: &str, args: &[&str]) -> (i32, String) {
    let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    let mut out = Vec::new();
    let code = app(prog, &args, &mut out).unwrap();
    (code, String::from_utf8(out).unwrap())
}
