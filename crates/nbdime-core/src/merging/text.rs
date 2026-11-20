//! Rendering a three-way text merge with conflict markers.

use std::collections::hash_map::RandomState;
use std::env;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::py;

use super::MergeError;

/// The merge of `local` and `remote` against `base` as text with `<<<<<<<`, `=======` and
/// `>>>>>>>` markers around the conflicting parts, and the status that the renderer reports
/// (`git merge-file`: the number of conflicts; zero when the merge is clean).
///
/// Picks the renderer like upstream's `prettyprint.merge_render`: `git merge-file` when `git` is
/// on `PATH`, else `diff3 -m`, else a built-in renderer that treats any difference between the
/// two sides as one conflict. The external tools run in a private temporary directory and see
/// the caller's environment, so the user's git configuration applies.
///
/// `strategy` is one of `use-base`, `use-local`, `use-remote` or `union` (the last only with
/// git); anything else is ignored, as upstream does.
pub fn merge_render(
    base: &str,
    local: &str,
    remote: &str,
    strategy: Option<&str>,
) -> Result<(String, i32), MergeError> {
    if strategy == Some("use-base") {
        return Ok((base.to_owned(), 0));
    }
    if which("git") {
        merge_render_with_git(base, local, remote, strategy)
    } else if which("diff3") {
        merge_render_with_diff3(base, local, remote, strategy)
    } else {
        Ok(builtin_merge_render(local, remote, strategy))
    }
}

fn merge_render_with_git(
    base: &str,
    local: &str,
    remote: &str,
    strategy: Option<&str>,
) -> Result<(String, i32), MergeError> {
    let mut args = vec!["merge-file", "-p", "local", "base", "remote"];
    match strategy {
        Some("use-local") => args.push("--ours"),
        Some("use-remote") => args.push("--theirs"),
        Some("union") => args.push("--union"),
        _ => {}
    }
    let (merged, status, stderr) = external_merge_render("git", &args, base, local, remote)?;
    // Without a last line there is nothing to inspect (upstream raises `IndexError`).
    let Some(last) = py::splitlines_keepends(&merged).last().copied() else {
        let detail = stderr.trim();
        return Err(MergeError::Render(if detail.is_empty() {
            "git produced no output".into()
        } else {
            format!("git produced no output: {detail}")
        }));
    };
    // Drop the trailing newline if ">>>>>>> remote" is the last line.
    if last.contains('\n') && last.contains(">>>>>>>") {
        return Ok((py::rstrip(&merged).to_owned(), status));
    }
    Ok((merged, status))
}

fn merge_render_with_diff3(
    base: &str,
    local: &str,
    remote: &str,
    strategy: Option<&str>,
) -> Result<(String, i32), MergeError> {
    match strategy {
        Some("use-local") => Ok((local.to_owned(), 0)),
        Some("use-remote") => Ok((remote.to_owned(), 0)),
        _ => {
            let args = ["-m", "local", "base", "remote"];
            let (merged, status, _) = external_merge_render("diff3", &args, base, local, remote)?;
            Ok((merged, status))
        }
    }
}

/// The renderer of last resort: any difference between the sides is one conflict.
fn builtin_merge_render(local: &str, remote: &str, strategy: Option<&str>) -> (String, i32) {
    if local == remote {
        return (local.to_owned(), 0);
    }
    match strategy {
        Some("use-local") => return (local.to_owned(), 0),
        Some("use-remote") => return (remote.to_owned(), 0),
        _ => {}
    }
    let lines = format_merge_render_lines(
        lines_of(local),
        lines_of(remote),
        "local",
        "remote",
        7, // git's default marker size
    );
    (lines.concat(), 1)
}

fn lines_of(text: &str) -> Vec<String> {
    py::splitlines_keepends(text)
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// The lines of a conflict between `local` and `remote` (each as `splitlines(True)` lines): the
/// common leading lines, the marked conflict, and the common last line.
///
/// Quirks of upstream kept as they are: the final line of each side gets a second newline when
/// it already ends in one, and a common last line is repeated after the conflict while it also
/// stays inside both sides (upstream's loop for the common tail walks forward from the last
/// index, so it matches at most that one line and trims nothing).
fn format_merge_render_lines(
    mut local: Vec<String>,
    mut remote: Vec<String>,
    local_title: &str,
    remote_title: &str,
    marker_size: usize,
) -> Vec<String> {
    for side in [&mut local, &mut remote] {
        if let Some(last) = side.last_mut().filter(|l| l.ends_with('\n')) {
            last.push('\n');
        }
    }

    let common = local
        .iter()
        .zip(&remote)
        .take_while(|(l, r)| l == r)
        .count();
    let mut lines: Vec<String> = local.drain(..common).collect();
    remote.drain(..common);

    let postline = match (local.last(), remote.last()) {
        (Some(l), Some(r)) if l == r => Some(l.clone()),
        _ => None,
    };

    lines.push(format!("{} {local_title}\n", "<".repeat(marker_size)));
    lines.append(&mut local);
    lines.push(format!("{}\n", "=".repeat(marker_size)));
    lines.append(&mut remote);
    lines.push(format!("{} {remote_title}\n", ">".repeat(marker_size)));
    lines.extend(postline);

    for line in &mut lines {
        if !line.ends_with('\n') {
            line.push('\n');
        }
    }
    if let Some(last) = lines.last_mut() {
        let kept = last.trim_end_matches(['\r', '\n']).len();
        last.truncate(kept);
    }
    lines
}

/// Runs `program` with `args` over the files `local`, `base` and `remote` in a private temporary
/// directory and returns its standard output with `\r\n` turned into `\n`, its exit status and
/// its standard error.
fn external_merge_render(
    program: &str,
    args: &[&str],
    base: &str,
    local: &str,
    remote: &str,
) -> Result<(String, i32, String), MergeError> {
    let dir = TempDir::new().map_err(render_error)?;
    for (name, text) in [("local", local), ("base", base), ("remote", remote)] {
        write_private(&dir.path().join(name), text).map_err(render_error)?;
    }
    let output = Command::new(program)
        .args(args)
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .output()
        .map_err(|e| MergeError::Render(format!("cannot run {program}: {e}")))?;
    let Ok(text) = String::from_utf8(output.stdout) else {
        return Err(MergeError::Render(format!("{program} output is not UTF-8")));
    };
    let status = exit_status(&output.status);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok((text.replace("\r\n", "\n"), status, stderr))
}

/// The exit code, or the negated signal number for a killed process, as Python reports it.
fn exit_status(status: &std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .or_else(|| status.signal().map(|s| -s))
            .unwrap_or(-1)
    }
    #[cfg(not(unix))]
    {
        status.code().unwrap_or(-1)
    }
}

fn render_error(e: io::Error) -> MergeError {
    MergeError::Render(e.to_string())
}

/// `shutil.which(program)`: an executable regular file in a directory of `PATH`.
fn which(program: &str) -> bool {
    let path = env::var_os("PATH").unwrap_or_else(|| "/bin:/usr/bin".into());
    env::split_paths(&path).any(|dir| is_executable(&dir.join(program)))
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// A scratch directory, readable by its owner only, that is removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    /// Creates a directory with an unpredictable name (`mkdtemp`): `create_dir` fails on any
    /// existing path, so a name that someone else has already taken is retried, never reused.
    fn new() -> io::Result<TempDir> {
        let parent = env::temp_dir();
        for _ in 0..100 {
            let mut hasher = RandomState::new().build_hasher();
            hasher.write_u32(std::process::id());
            let path = parent.join(format!("nbdime-merge-{:016x}", hasher.finish()));
            match create_private_dir(&path) {
                Ok(()) => return Ok(TempDir(path)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "cannot find an unused temporary directory name",
        ))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

fn write_private(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests of an external renderer only mean something where the tool exists.
    fn have(tool: &str) -> bool {
        let found = which(tool);
        if !found {
            eprintln!("{tool} is not on PATH: test not run");
        }
        found
    }

    fn builtin(local: &str, remote: &str) -> (String, i32) {
        builtin_merge_render(local, remote, None)
    }

    #[test]
    fn builtin_equal_sides_are_clean() {
        assert_eq!(builtin("same", "same"), ("same".into(), 0));
    }

    #[test]
    fn builtin_marks_one_conflict_and_doubles_final_newlines() {
        assert_eq!(
            builtin("a\n", "b\n"),
            ("<<<<<<< local\na\n\n=======\nb\n\n>>>>>>> remote".into(), 1)
        );
        assert_eq!(
            builtin("b", "c"),
            ("<<<<<<< local\nb\n=======\nc\n>>>>>>> remote".into(), 1)
        );
    }

    #[test]
    fn builtin_keeps_the_common_prefix_outside_the_conflict() {
        assert_eq!(
            builtin("x\ny", "x\nz"),
            ("x\n<<<<<<< local\ny\n=======\nz\n>>>>>>> remote".into(), 1)
        );
    }

    #[test]
    fn builtin_repeats_a_common_last_line_after_the_conflict() {
        assert_eq!(
            builtin("a\nq\nc", "a\nr\nc"),
            (
                "a\n<<<<<<< local\nq\nc\n=======\nr\nc\n>>>>>>> remote\nc".into(),
                1
            )
        );
    }

    #[test]
    fn builtin_empty_side_and_crlf() {
        assert_eq!(
            builtin("", "x"),
            ("<<<<<<< local\n=======\nx\n>>>>>>> remote".into(), 1)
        );
        assert_eq!(
            builtin("a\r\nb\r\n", "a\r\nc\r\n"),
            (
                "a\r\n<<<<<<< local\nb\r\n\n=======\nc\r\n\n>>>>>>> remote".into(),
                1
            )
        );
    }

    #[test]
    fn builtin_strategies() {
        assert_eq!(
            builtin_merge_render("l\n", "r\n", Some("use-local")),
            ("l\n".into(), 0)
        );
        assert_eq!(
            builtin_merge_render("l\n", "r\n", Some("use-remote")),
            ("r\n".into(), 0)
        );
        // Other strategies are ignored.
        assert_eq!(
            builtin_merge_render("l", "r", Some("union")),
            builtin("l", "r")
        );
    }

    #[test]
    fn use_base_needs_no_renderer() {
        assert_eq!(
            merge_render("base", "l", "r", Some("use-base")).unwrap(),
            ("base".into(), 0)
        );
    }

    #[test]
    fn git_clean_merge_has_status_zero() {
        if !have("git") {
            return;
        }
        let r = merge_render_with_git("a\nb\nc\n", "A\nb\nc\n", "a\nb\nC\n", None).unwrap();
        assert_eq!(r, ("A\nb\nC\n".into(), 0));
    }

    #[test]
    fn git_conflict_is_marked_and_loses_its_final_newline() {
        if !have("git") {
            return;
        }
        let r = merge_render_with_git("a\n", "b\n", "c\n", None).unwrap();
        assert_eq!(
            r,
            ("<<<<<<< local\nb\n=======\nc\n>>>>>>> remote".into(), 1)
        );
    }

    #[test]
    fn git_strategies() {
        if !have("git") {
            return;
        }
        let ours = merge_render_with_git("a\n", "b\n", "c\n", Some("use-local")).unwrap();
        assert_eq!(ours.0, "b\n");
        let theirs = merge_render_with_git("a\n", "b\n", "c\n", Some("use-remote")).unwrap();
        assert_eq!(theirs.0, "c\n");
        let union = merge_render_with_git("a\n", "b\n", "c\n", Some("union")).unwrap();
        assert_eq!(union.0, "b\nc\n");
    }

    #[test]
    fn git_empty_result_is_an_error() {
        if !have("git") {
            return;
        }
        assert!(merge_render_with_git("", "", "", None).is_err());
    }

    #[test]
    fn git_carriage_returns_are_normalized() {
        if !have("git") {
            return;
        }
        let (merged, _) = merge_render_with_git("a\r\n", "a\r\nb\r\n", "a\r\n", None).unwrap();
        assert_eq!(merged, "a\nb\n");
    }

    #[test]
    fn diff3_clean_and_conflicting_merges() {
        if !have("diff3") {
            return;
        }
        let clean = merge_render_with_diff3("a\nb\nc\n", "A\nb\nc\n", "a\nb\nC\n", None).unwrap();
        assert_eq!(clean, ("A\nb\nC\n".into(), 0));
        let conflict = merge_render_with_diff3("a\n", "b\n", "c\n", None).unwrap();
        assert_eq!(
            conflict,
            (
                "<<<<<<< local\nb\n||||||| base\na\n=======\nc\n>>>>>>> remote\n".into(),
                1
            )
        );
        // Unlike git's, the result is neither trimmed nor an error when empty.
        assert_eq!(
            merge_render_with_diff3("", "", "", None).unwrap(),
            (String::new(), 0)
        );
        let crlf = merge_render_with_diff3("a\r\n", "a\r\nb\r\n", "a\r\n", None).unwrap();
        assert_eq!(crlf, ("a\nb\n".into(), 0));
    }

    #[test]
    fn diff3_strategies_return_a_side() {
        assert_eq!(
            merge_render_with_diff3("b", "l", "r", Some("use-local")).unwrap(),
            ("l".into(), 0)
        );
        assert_eq!(
            merge_render_with_diff3("b", "l", "r", Some("use-remote")).unwrap(),
            ("r".into(), 0)
        );
    }

    #[test]
    fn merge_render_uses_whatever_renderer_is_available() {
        let (merged, status) = merge_render("a\n", "b\n", "c\n", None).unwrap();
        assert_eq!(status, 1);
        assert!(merged.starts_with("<<<<<<< local\n"));
    }

    #[test]
    fn which_finds_executables_on_path_only() {
        assert!(!which("nbdime-rs-no-such-tool"));
        assert!(!which("/"));
    }

    #[test]
    fn temp_dirs_are_private_unique_and_removed() {
        let (a, b) = (TempDir::new().unwrap(), TempDir::new().unwrap());
        assert_ne!(a.path(), b.path());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(a.path()), 0o700);
            write_private(&a.path().join("f"), "x").unwrap();
            assert_eq!(mode(&a.path().join("f")), 0o600);
        }
        let (pa, pb) = (a.path().to_owned(), b.path().to_owned());
        drop(a);
        drop(b);
        assert!(!pa.exists() && !pb.exists());
    }

    #[test]
    fn existing_files_are_never_overwritten() {
        let dir = TempDir::new().unwrap();
        write_private(&dir.path().join("f"), "x").unwrap();
        assert!(write_private(&dir.path().join("f"), "y").is_err());
    }
}
