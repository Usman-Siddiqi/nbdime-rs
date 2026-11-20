//! Rendering a three-way text merge with conflict markers.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::py;

use super::MergeError;

/// The merge of `local` and `remote` against `base` as text with `<<<<<<<`, `=======` and
/// `>>>>>>>` markers around the conflicting parts, and the number `git merge-file` reports
/// (the count of conflicts; zero when the merge is clean).
///
/// Runs `git merge-file -p local base remote` without any system or global git configuration.
pub fn merge_render(base: &str, local: &str, remote: &str) -> Result<(String, i32), MergeError> {
    let (merged, status) = external_merge_render(base, local, remote)?;
    // Without a last line there is nothing to inspect (upstream raises `IndexError`).
    let Some(last) = py::splitlines_keepends(&merged).last().copied() else {
        return Err(MergeError::Render("git produced no output".into()));
    };
    // Drop the trailing newline if ">>>>>>> remote" is the last line.
    if last.contains('\n') && last.contains(">>>>>>>") {
        return Ok((py::rstrip(&merged).to_owned(), status));
    }
    Ok((merged, status))
}

/// A scratch directory that is removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<TempDir, MergeError> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let dir = std::env::temp_dir().join(format!(
            "nbdime-rs-merge-{}-{}-{nanos}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).map_err(render_error)?;
        Ok(TempDir(dir))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn render_error(e: std::io::Error) -> MergeError {
    MergeError::Render(e.to_string())
}

fn external_merge_render(
    base: &str,
    local: &str,
    remote: &str,
) -> Result<(String, i32), MergeError> {
    let dir = TempDir::new()?;
    for (name, text) in [("local", local), ("base", base), ("remote", remote)] {
        fs::write(dir.0.join(name), text).map_err(render_error)?;
    }
    let output = Command::new("git")
        .args(["merge-file", "-p", "local", "base", "remote"])
        .current_dir(&dir.0)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| MergeError::Render(format!("cannot run git: {e}")))?;
    let text = String::from_utf8(output.stdout)
        .map_err(|e| MergeError::Render(format!("git output is not UTF-8: {e}")))?;
    // Python reports a process killed by a signal as a negative status.
    let status = output.status.code().unwrap_or(-1);
    Ok((text.replace("\r\n", "\n"), status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_merge_has_status_zero() {
        let (merged, status) = merge_render("a\nb\nc\n", "A\nb\nc\n", "a\nb\nC\n").unwrap();
        assert_eq!(merged, "A\nb\nC\n");
        assert_eq!(status, 0);
    }

    #[test]
    fn conflict_is_marked_and_loses_its_final_newline() {
        let (merged, status) = merge_render("a\n", "b\n", "c\n").unwrap();
        assert_eq!(merged, "<<<<<<< local\nb\n=======\nc\n>>>>>>> remote");
        assert_eq!(status, 1);
    }

    #[test]
    fn empty_result_is_an_error() {
        assert!(merge_render("", "", "").is_err());
    }

    #[test]
    fn carriage_returns_are_normalized() {
        let (merged, _) = merge_render("a\r\n", "a\r\nb\r\n", "a\r\n").unwrap();
        assert_eq!(merged, "a\nb\n");
    }
}
