//! Rendering text diffs and three-way merges, with `git` or `diff`/`diff3` where they exist and
//! built-in renderers otherwise, in the order upstream's `prettyprint` tries them.
//!
//! The tools run in a private temporary directory (mode 0700, unpredictable name, files created
//! exclusively with mode 0600; removed on every path) and inherit the caller's environment, so
//! the user's git configuration applies, as it does upstream.

use std::collections::hash_map::RandomState;
use std::env;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::difflib::unified_diff;
use crate::py;

use super::PrettyPrintConfig;

/// Why a diff or merge could not be rendered: the tool could not be run, its output is not
/// text, or it is empty where upstream indexes into it (`IndexError`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RenderError(pub String);

/// The text of `a` and `b` as the terminal shows a change from one to the other: the lines of a
/// unified diff (git's `--color`/`--color-words` form, plain `diff`, or difflib's), without the
/// header that names the compared files.
///
/// Uses `git diff --no-index` when `config.use_git` and `git` is on `PATH`, else `diff` when
/// `config.use_diff` and `diff` is, else difflib.
pub fn diff_render(a: &str, b: &str, config: &PrettyPrintConfig) -> Result<String, RenderError> {
    if config.use_git && which("git") {
        diff_render_with_git(a, b, config)
    } else if config.use_diff && which("diff") {
        diff_render_with_diff(a, b)
    } else {
        Ok(diff_render_with_difflib(a, b, config))
    }
}

/// `git diff --no-index` of `a` against `b`, minus git's four header lines.
pub fn diff_render_with_git(
    a: &str,
    b: &str,
    config: &PrettyPrintConfig,
) -> Result<String, RenderError> {
    let mut args = vec!["diff", "--no-index"];
    if config.use_color {
        args.push(if config.color_words {
            "--color-words"
        } else {
            "--color"
        });
    }
    args.extend(["before", "after"]);
    let diff = external_diff_render("git", &args, a, b)?;
    Ok(py::splitlines_keepends(&diff).into_iter().skip(4).collect())
}

/// The output of `diff before after`.
pub fn diff_render_with_diff(a: &str, b: &str) -> Result<String, RenderError> {
    external_diff_render("diff", &["before", "after"], a, b)
}

/// difflib's unified diff, coloured like git's, minus its two header lines.
pub fn diff_render_with_difflib(a: &str, b: &str, config: &PrettyPrintConfig) -> String {
    py::splitlines_keepends(&builtin_diff_render(a, b, config))
        .into_iter()
        .skip(2)
        .collect()
}

fn builtin_diff_render(a: &str, b: &str, config: &PrettyPrintConfig) -> String {
    let (a_lines, b_lines) = (py::splitlines(a), py::splitlines(b));
    let rendered: Vec<String> = unified_diff(&a_lines, &b_lines, 3, "")
        .into_iter()
        .map(|line| {
            let mut chars = line.chars();
            let first = chars.next();
            let rest = chars.as_str();
            match first {
                Some('+') => format!("{}{rest}{}", config.add(), config.reset()),
                Some('-') => format!("{}{rest}{}", config.remove(), config.reset()),
                Some('@') => line,
                // Cannot happen: every line of a unified diff starts with one of the above or a space.
                _ => format!("{}{rest}{}", config.keep(), config.reset()),
            }
        })
        .collect();
    rendered.join("\n")
}

/// The merge of `local` and `remote` against `base` as text with `<<<<<<<`, `=======` and
/// `>>>>>>>` markers around the conflicting parts, and the status that the renderer reports
/// (`git merge-file`: the number of conflicts; zero when the merge is clean).
///
/// Picks the renderer like upstream's `merge_render`: `git merge-file` when `config.use_git` and
/// `git` is on `PATH`, else `diff3 -m` when `config.use_diff` and `diff3` is, else a built-in
/// renderer that treats any difference between the two sides as one conflict.
///
/// `strategy` is one of `use-base`, `use-local`, `use-remote` or `union` (the last only with
/// git); anything else is ignored, as upstream does (minus its warning).
pub fn merge_render(
    base: &str,
    local: &str,
    remote: &str,
    strategy: Option<&str>,
    config: &PrettyPrintConfig,
) -> Result<(String, i32), RenderError> {
    if strategy == Some("use-base") {
        return Ok((base.to_owned(), 0));
    }
    if config.use_git && which("git") {
        merge_render_with_git(base, local, remote, strategy)
    } else if config.use_diff && which("diff3") {
        merge_render_with_diff3(base, local, remote, strategy)
    } else {
        Ok(builtin_merge_render(local, remote, strategy))
    }
}

pub fn merge_render_with_git(
    base: &str,
    local: &str,
    remote: &str,
    strategy: Option<&str>,
) -> Result<(String, i32), RenderError> {
    let mut args = vec!["merge-file", "-p", "local", "base", "remote"];
    match strategy {
        Some("use-local") => args.push("--ours"),
        Some("use-remote") => args.push("--theirs"),
        Some("union") => args.push("--union"),
        _ => {}
    }
    let (merged, status) = external_merge_render("git", &args, base, local, remote)?;
    // Without a last line there is nothing to inspect (upstream raises `IndexError`).
    let Some(last) = py::splitlines_keepends(&merged).last().copied() else {
        return Err(RenderError("git produced no output".into()));
    };
    // Drop the trailing newline if ">>>>>>> remote" is the last line.
    if last.contains('\n') && last.contains(">>>>>>>") {
        return Ok((py::rstrip(&merged).to_owned(), status));
    }
    Ok((merged, status))
}

pub fn merge_render_with_diff3(
    base: &str,
    local: &str,
    remote: &str,
    strategy: Option<&str>,
) -> Result<(String, i32), RenderError> {
    match strategy {
        Some("use-local") => Ok((local.to_owned(), 0)),
        Some("use-remote") => Ok((remote.to_owned(), 0)),
        _ => {
            let args = ["-m", "local", "base", "remote"];
            let (merged, status) = external_merge_render("diff3", &args, base, local, remote)?;
            Ok((merged, status))
        }
    }
}

/// The renderer of last resort: any difference between the sides is one conflict.
pub fn builtin_merge_render(local: &str, remote: &str, strategy: Option<&str>) -> (String, i32) {
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

/// Runs `program` with `args` over the files `before` and `after` in a private temporary
/// directory and returns its standard output, minus the `\ No newline at end of file` lines.
fn external_diff_render(
    program: &str,
    args: &[&str],
    a: &str,
    b: &str,
) -> Result<String, RenderError> {
    let dir = TempDir::new("nbdime-diff-")?;
    for (name, text) in [("before", a), ("after", b)] {
        write_named(&dir.path().join(name), text)?;
    }
    let output = Command::new(program)
        .args(args)
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| RenderError(format!("cannot run {program}: {e}")))?;
    let text = String::from_utf8(output.stdout)
        .map_err(|_| RenderError(format!("{program} output is not UTF-8")))?;
    remove_no_newline_markers(&text)
}

/// `re.subn(r"^\\ No newline at end of file\n?", "", text, flags=re.M)`, which upstream asserts
/// to remove at most two lines.
fn remove_no_newline_markers(text: &str) -> Result<String, RenderError> {
    const MARKER: &str = "\\ No newline at end of file";
    let mut out = String::with_capacity(text.len());
    let mut removed = 0;
    for line in text.split_inclusive('\n') {
        match line.strip_prefix(MARKER) {
            Some(rest) => {
                removed += 1;
                out.push_str(rest.strip_prefix('\n').unwrap_or(rest));
            }
            None => out.push_str(line),
        }
    }
    if removed > 2 {
        return Err(RenderError(
            "unexpected output from external diff renderer".into(),
        ));
    }
    Ok(out)
}

/// Runs `program` with `args` over the files `local`, `base` and `remote` in a private temporary
/// directory and returns its standard output with `\r\n` turned into `\n` and its exit status.
/// Its standard error is the caller's, as upstream's `Popen` leaves it.
fn external_merge_render(
    program: &str,
    args: &[&str],
    base: &str,
    local: &str,
    remote: &str,
) -> Result<(String, i32), RenderError> {
    let dir = TempDir::new("nbdime-merge-")?;
    for (name, text) in [("local", local), ("base", base), ("remote", remote)] {
        write_named(&dir.path().join(name), text)?;
    }
    let output = Command::new(program)
        .args(args)
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| RenderError(format!("cannot run {program}: {e}")))?;
    let Ok(text) = String::from_utf8(output.stdout) else {
        return Err(RenderError(format!("{program} output is not UTF-8")));
    };
    let status = exit_status(&output.status);
    Ok((text.replace("\r\n", "\n"), status))
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

/// `shutil.which(program)`: a file that is not a directory and that the current user may
/// execute, in a directory of `PATH` (an empty `PATH` finds nothing, an empty entry is the
/// current directory).
fn which(program: &str) -> bool {
    which_in(program, env::var_os("PATH"))
}

/// [`which`] with the value of `PATH` given (`None` when it is not set, which leaves the
/// platform's default).
fn which_in(program: &str, path: Option<std::ffi::OsString>) -> bool {
    let path = path.unwrap_or_else(|| default_path().into());
    !path.is_empty()
        && env::split_paths(&path)
            .any(|dir| executable_names(program).any(|name| is_executable(&dir.join(name))))
}

#[cfg(unix)]
fn default_path() -> &'static str {
    "/bin:/usr/bin"
}

#[cfg(not(unix))]
fn default_path() -> &'static str {
    ""
}

/// The names `program` may have on disk: itself, and on Windows also with each `PATHEXT`
/// extension (`shutil.which`).
fn executable_names(program: &str) -> impl Iterator<Item = String> + '_ {
    let extensions: Vec<String> = if cfg!(windows) {
        env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned())
            .split(';')
            .filter(|ext| !ext.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        Vec::new()
    };
    std::iter::once(program.to_owned()).chain(
        extensions
            .into_iter()
            .map(move |ext| program.to_owned() + &ext),
    )
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| !meta.is_dir()) && may_execute(path)
}

/// `os.access(path, os.X_OK)`, which asks the kernel about the current user's rights (the mode
/// bits alone say nothing about root, ACLs or read-only mounts).
#[cfg(unix)]
#[allow(unsafe_code)]
fn may_execute(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `path` is a NUL-terminated string that outlives the call, and `access` only reads it.
    unsafe { libc::access(path.as_ptr(), libc::X_OK) == 0 }
}

#[cfg(not(unix))]
fn may_execute(_path: &Path) -> bool {
    true
}

/// A scratch directory, readable by its owner only, that is removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    /// Creates a directory with an unpredictable name (`mkdtemp`) in the directory that
    /// [`temp_dir`] picks: `create_dir` fails on any existing path, so a name that someone else
    /// has already taken is retried, never reused.
    fn new(prefix: &str) -> Result<TempDir, RenderError> {
        let parent = temp_dir(&tempdir_candidates(|name| env::var_os(name)))?;
        for _ in 0..100 {
            let path = parent.join(random_name(prefix));
            match create_private_dir(&path) {
                Ok(()) => return Ok(TempDir(path)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => {
                    return Err(RenderError(format!(
                        "cannot create {}: {e}",
                        path.display()
                    )))
                }
            }
        }
        Err(RenderError(format!(
            "cannot find an unused temporary directory name in {}",
            parent.display()
        )))
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

fn random_name(prefix: &str) -> String {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u32(std::process::id());
    format!("{prefix}{:016x}", hasher.finish())
}

/// The directories `tempfile.gettempdir()` considers, best first: those named by `TMPDIR`,
/// `TEMP` and `TMP`, the platform's, and the working directory.
fn tempdir_candidates(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = ["TMPDIR", "TEMP", "TMP"]
        .into_iter()
        .filter_map(var)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .collect();
    #[cfg(unix)]
    dirs.extend(["/tmp", "/var/tmp", "/usr/tmp"].map(PathBuf::from));
    #[cfg(not(unix))]
    dirs.push(env::temp_dir());
    dirs.extend(env::current_dir());
    dirs
}

/// The first of `candidates` (made absolute) in which a file can be created, which is how
/// `tempfile` picks its directory.
fn temp_dir(candidates: &[PathBuf]) -> Result<PathBuf, RenderError> {
    let cwd = env::current_dir().unwrap_or_default();
    for dir in candidates {
        let dir = cwd.join(dir);
        for _ in 0..100 {
            let probe = dir.join(random_name("nbdime-probe-"));
            match write_private(&probe, "blat") {
                Ok(()) => {
                    let _ = fs::remove_file(&probe);
                    return Ok(dir);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(_) => break,
            }
        }
    }
    let tried: Vec<_> = candidates
        .iter()
        .map(|dir| dir.display().to_string())
        .collect();
    Err(RenderError(format!(
        "no usable temporary directory found in [{}]",
        tried.join(", ")
    )))
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// [`write_private`], with an error that names the file.
/// `io.open(path, "w", encoding="utf8").write(text)`, newline translation included.
fn write_named(path: &Path, text: &str) -> Result<(), RenderError> {
    write_private(path, &crate::textmode::native(text))
        .map_err(|e| RenderError(format!("cannot write {}: {e}", path.display())))
}

/// Writes `text` to a new file that only its owner can read.
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

    /// The tests of an external renderer need the tool: a machine without it fails them instead
    /// of skipping them.
    fn require(tool: &str) {
        assert!(which(tool), "{tool} must be on PATH to run these tests");
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
            merge_render(
                "base",
                "l",
                "r",
                Some("use-base"),
                &PrettyPrintConfig::default()
            )
            .unwrap(),
            ("base".into(), 0)
        );
    }

    #[test]
    fn git_clean_merge_has_status_zero() {
        require("git");
        let r = merge_render_with_git("a\nb\nc\n", "A\nb\nc\n", "a\nb\nC\n", None).unwrap();
        assert_eq!(r, ("A\nb\nC\n".into(), 0));
    }

    #[test]
    fn git_conflict_is_marked_and_loses_its_final_newline() {
        require("git");
        let r = merge_render_with_git("a\n", "b\n", "c\n", None).unwrap();
        assert_eq!(
            r,
            ("<<<<<<< local\nb\n=======\nc\n>>>>>>> remote".into(), 1)
        );
    }

    #[test]
    fn git_strategies() {
        require("git");
        let ours = merge_render_with_git("a\n", "b\n", "c\n", Some("use-local")).unwrap();
        assert_eq!(ours.0, "b\n");
        let theirs = merge_render_with_git("a\n", "b\n", "c\n", Some("use-remote")).unwrap();
        assert_eq!(theirs.0, "c\n");
        let union = merge_render_with_git("a\n", "b\n", "c\n", Some("union")).unwrap();
        assert_eq!(union.0, "b\nc\n");
    }

    #[test]
    fn git_empty_result_is_an_error() {
        require("git");
        assert!(merge_render_with_git("", "", "", None).is_err());
    }

    #[test]
    fn git_carriage_returns_are_normalized() {
        require("git");
        let (merged, _) = merge_render_with_git("a\r\n", "a\r\nb\r\n", "a\r\n", None).unwrap();
        assert_eq!(merged, "a\nb\n");
    }

    #[test]
    fn diff3_clean_and_conflicting_merges() {
        require("diff3");
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
        let config = PrettyPrintConfig::default();
        let (merged, status) = merge_render("a\n", "b\n", "c\n", None, &config).unwrap();
        assert_eq!(status, 1);
        assert!(merged.starts_with("<<<<<<< local\n"));
    }

    #[test]
    fn which_finds_executables_on_path_only() {
        assert!(!which("nbdime-rs-no-such-tool"));
        assert!(!which("/"));
    }

    #[cfg(unix)]
    #[test]
    fn which_asks_for_execute_rights_and_skips_directories_like_shutil() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("nbdime-test-").unwrap();
        for (name, mode) in [("runnable", 0o755), ("plain", 0o644)] {
            let file = dir.path().join(name);
            fs::write(&file, "").unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
        }
        fs::create_dir(dir.path().join("folder")).unwrap();
        let path = Some(dir.path().as_os_str().to_owned());
        assert!(which_in("runnable", path.clone()));
        assert!(!which_in("plain", path.clone()));
        assert!(!which_in("folder", path.clone()));
        assert!(!which_in("missing", path));

        // `PATH=""` finds nothing; `PATH=":"` has two empty entries, which are the current
        // directory (the absolute name below shows that an entry was tried).
        let absolute = dir.path().join("runnable");
        let absolute = absolute.to_str().unwrap();
        assert!(!which_in(absolute, Some("".into())));
        assert!(which_in(absolute, Some(":".into())));
    }

    #[test]
    fn temp_dir_candidates_follow_tempfile_order() {
        let var = |name: &str| match name {
            "TMPDIR" => Some("/first".into()),
            "TEMP" => Some("".into()),
            "TMP" => Some("/third".into()),
            _ => None,
        };
        let candidates = tempdir_candidates(var);
        assert_eq!(
            candidates[..2],
            [PathBuf::from("/first"), PathBuf::from("/third")]
        );
        #[cfg(unix)]
        assert_eq!(
            candidates[2..5],
            ["/tmp", "/var/tmp", "/usr/tmp"].map(PathBuf::from)
        );
        assert_eq!(candidates.last(), env::current_dir().ok().as_ref());
    }

    #[test]
    fn an_unusable_temp_dir_falls_through_to_the_next_candidate() {
        let good = TempDir::new("nbdime-test-").unwrap();
        let missing = good.path().join("missing");
        let picked = temp_dir(&[missing.clone(), good.path().to_owned()]).unwrap();
        assert_eq!(picked, good.path());
        // The probe leaves nothing behind.
        assert_eq!(fs::read_dir(good.path()).unwrap().count(), 0);

        let err = temp_dir(std::slice::from_ref(&missing)).unwrap_err();
        assert!(err.0.contains(missing.to_str().unwrap()), "{err}");

        // `TMPDIR` pointing nowhere does not stop a diff from being rendered.
        let var = |name: &str| (name == "TMPDIR").then(|| "/nonexistent-dir-xyz".into());
        let picked = temp_dir(&tempdir_candidates(var)).unwrap();
        assert_ne!(picked, PathBuf::from("/nonexistent-dir-xyz"));
    }

    #[test]
    fn write_errors_name_the_file() {
        let dir = TempDir::new("nbdime-test-").unwrap();
        let path = dir.path().join("missing").join("file");
        let err = write_named(&path, "x").unwrap_err();
        assert!(err.0.contains(path.to_str().unwrap()), "{err}");
    }

    #[test]
    fn temp_dirs_are_private_unique_and_removed() {
        let (a, b) = (
            TempDir::new("nbdime-test-").unwrap(),
            TempDir::new("nbdime-test-").unwrap(),
        );
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
        let dir = TempDir::new("nbdime-test-").unwrap();
        write_private(&dir.path().join("f"), "x").unwrap();
        assert!(write_private(&dir.path().join("f"), "y").is_err());
    }

    fn config(use_color: bool, color_words: bool) -> PrettyPrintConfig {
        PrettyPrintConfig {
            use_color,
            invalid_color: false,
            color_words,
            ..PrettyPrintConfig::default()
        }
    }

    const BEFORE: &str = "a\nb\nc\n";
    const AFTER: &str = "a\nB\nc\nd";

    #[test]
    fn difflib_renderer_drops_its_header_and_joins_without_a_final_newline() {
        assert_eq!(
            diff_render_with_difflib(BEFORE, AFTER, &config(false, false)),
            "@@ -1,3 +1,4 @@\n   a\n-  b\n+  B\n   c\n+  d"
        );
        assert_eq!(
            diff_render_with_difflib(BEFORE, AFTER, &config(true, false)),
            "@@ -1,3 +1,4 @@\n   a\x1b[0m\n\x1b[31m-  b\x1b[0m\n\x1b[32m+  B\x1b[0m\n   c\x1b[0m\n\
             \x1b[32m+  d\x1b[0m"
        );
        // Lines that differ only by their ending are equal to the renderer.
        assert_eq!(
            diff_render_with_difflib("a\n", "a", &config(true, false)),
            ""
        );
    }

    #[test]
    fn git_diff_without_its_header() {
        require("git");
        assert_eq!(
            diff_render_with_git(BEFORE, AFTER, &config(false, false)).unwrap(),
            "@@ -1,3 +1,4 @@\n a\n-b\n+B\n c\n+d\n"
        );
        assert_eq!(
            diff_render_with_git(BEFORE, AFTER, &config(true, false)).unwrap(),
            "\x1b[36m@@ -1,3 +1,4 @@\x1b[m\n a\x1b[m\n\x1b[31m-b\x1b[m\n\x1b[32m+\x1b[m\x1b[32mB\x1b[m\n c\x1b[m\n\
             \x1b[32m+\x1b[m\x1b[32md\x1b[m\n\x1b[m\n"
        );
        assert_eq!(
            diff_render_with_git(BEFORE, AFTER, &config(true, true)).unwrap(),
            "\x1b[36m@@ -1,3 +1,4 @@\x1b[m\na\x1b[m\n\x1b[31mb\x1b[m\x1b[32mB\x1b[m\nc\x1b[m\n\x1b[32md\x1b[m\n"
        );
    }

    #[test]
    fn plain_diff_output_is_returned_whole() {
        require("diff");
        assert_eq!(
            diff_render_with_diff(BEFORE, AFTER).unwrap(),
            "2c2\n< b\n---\n> B\n3a4\n> d\n"
        );
    }

    #[test]
    fn diff_render_falls_back_in_order() {
        let none = PrettyPrintConfig {
            use_git: false,
            use_diff: false,
            ..config(false, false)
        };
        assert_eq!(
            diff_render(BEFORE, AFTER, &none).unwrap(),
            diff_render_with_difflib(BEFORE, AFTER, &none)
        );
        require("diff");
        let diff_only = PrettyPrintConfig {
            use_diff: true,
            ..none
        };
        assert_eq!(
            diff_render(BEFORE, AFTER, &diff_only).unwrap(),
            diff_render_with_diff(BEFORE, AFTER).unwrap()
        );
    }

    #[test]
    fn no_newline_markers_are_removed_but_at_most_two_are_allowed() {
        assert_eq!(
            remove_no_newline_markers("-x\n\\ No newline at end of file\n+y\n").unwrap(),
            "-x\n+y\n"
        );
        // Only a marker that starts a line counts, and only a directly following newline goes.
        assert_eq!(
            remove_no_newline_markers(
                "\\ No newline at end of file more\nx \\ No newline at end of file\n"
            )
            .unwrap(),
            " more\nx \\ No newline at end of file\n"
        );
        let three = "\\ No newline at end of file\n".repeat(3);
        assert!(remove_no_newline_markers(&three).is_err());
    }

    #[test]
    fn merge_render_follows_the_configured_fallbacks() {
        let none = PrettyPrintConfig {
            use_git: false,
            use_diff: false,
            ..config(false, false)
        };
        assert_eq!(
            merge_render("a\n", "b\n", "c\n", None, &none).unwrap(),
            ("<<<<<<< local\nb\n\n=======\nc\n\n>>>>>>> remote".into(), 1)
        );
    }
}
