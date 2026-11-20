//! Notebooks in git revisions (`nbdime.gitfiles`, `nbdime.vcs.git.filter_integration`).
//!
//! Upstream drives git through GitPython; here the same questions go to `git` subprocesses:
//! `rev-parse` for revisions and the `diff` and `diff-tree` raw formats GitPython parses, and
//! `check-attr` and `config` for clean filters.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};

use crate::utils::{Source, EXPLICIT_MISSING_FILE};

/// A git repository found from a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// `None` for a bare repository.
    pub working_tree: Option<PathBuf>,
    /// Directories between the working tree and the directory the search started in.
    pub popped: Vec<String>,
}

fn is_git_dir(path: &Path) -> bool {
    path.join("objects").is_dir() && path.join("refs").is_dir() && path.join("HEAD").exists()
}

/// Whether `dir` is a repository's working tree: `.git` is a git directory, or a file naming one.
fn is_working_tree(dir: &Path) -> bool {
    let dotgit = dir.join(".git");
    if dotgit.is_dir() {
        return is_git_dir(&dotgit);
    }
    fs::read_to_string(&dotgit)
        .ok()
        .and_then(|text| text.strip_prefix("gitdir: ").map(|p| p.trim().to_owned()))
        .is_some_and(|target| is_git_dir(&dir.join(target)))
}

/// `get_repo(path)`: the repository `path` is in (or is the top of), or `None`.
pub fn get_repo(path: &Path) -> Option<Repo> {
    let absolute = std::path::absolute(path).ok()?;
    let mut current = fs::canonicalize(&absolute).unwrap_or(absolute);
    let mut popped = Vec::new();
    loop {
        if is_working_tree(&current) {
            return Some(Repo {
                working_tree: Some(current),
                popped,
            });
        }
        if is_git_dir(&current) {
            return Some(Repo {
                working_tree: None,
                popped,
            });
        }
        let name = current.file_name()?.to_string_lossy().into_owned();
        popped.insert(0, name);
        current = current.parent()?.to_path_buf();
    }
}

fn git(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(repo)
        .env("LANGUAGE", "C")
        .env("LC_ALL", "C")
        .stdin(Stdio::null());
    command
}

/// Runs `command`, returning its stdout if it succeeded.
fn output_of(mut command: Command) -> Result<Vec<u8>> {
    let output = command.output().context("running git")?;
    if !output.status.success() {
        bail!(
            "git failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

/// The object `rev` names, peeled to `kind` (`commit` or `tree`); `None` is `HEAD`.
fn rev_parse(repo: &Path, rev: Option<&str>, kind: &str) -> Option<String> {
    let rev = rev.unwrap_or("HEAD");
    if rev.starts_with('-') {
        return None;
    }
    let output = git(repo)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("{rev}^{{{kind}}}"))
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// `is_valid_gitref(ref)`: whether the current directory is in a repository that has the
/// revision. `None` asks for `HEAD`; a repository without commits then raises upstream.
fn is_valid_gitref(rev: Option<&str>) -> Result<bool> {
    let Some(repo) = get_repo(Path::new(".")) else {
        return Ok(false);
    };
    let Some(root) = repo.working_tree.as_deref().or(None) else {
        return Ok(false);
    };
    match (rev_parse(root, rev, "commit"), rev) {
        (Some(_), _) => Ok(true),
        (None, None) => bail!("Reference at 'HEAD' does not exist"),
        (None, Some(_)) => Ok(false),
    }
}

/// `is_gitref(candidate)`: a revision, as opposed to a file name. A name that is both is a file.
pub fn is_gitref(candidate: Option<&str>) -> Result<bool> {
    let usable = match candidate {
        None => true,
        Some(c) => !Path::new(c).exists() && c != EXPLICIT_MISSING_FILE && !c.is_empty(),
    };
    if !usable {
        return Ok(false);
    }
    is_valid_gitref(candidate)
}

/// `os.path.relpath(path, start)` for absolute paths.
fn relpath(path: &Path, start: &Path) -> PathBuf {
    let path: Vec<Component> = path.components().collect();
    let start: Vec<Component> = start.components().collect();
    let common = path.iter().zip(&start).take_while(|(a, b)| a == b).count();
    let mut result = PathBuf::new();
    for _ in common..start.len() {
        result.push("..");
    }
    for component in &path[common..] {
        result.push(component);
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    result
}

/// One line of `git diff --raw -z` output.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RawEntry {
    a_path: String,
    b_path: String,
    a_blob: Option<String>,
    b_blob: Option<String>,
}

const NULL_SHA: &str = "0000000000000000000000000000000000000000";

/// Parses the `--raw -z` format the way GitPython's `Diff` does: a blob is absent for an added
/// (a side) or deleted (b side) file and for the null id.
fn parse_raw(output: &[u8]) -> Result<Vec<RawEntry>> {
    let text = String::from_utf8_lossy(output);
    let mut tokens = text.split('\0').filter(|t| !t.is_empty());
    let mut entries = Vec::new();
    while let Some(meta) = tokens.next() {
        let meta = meta
            .strip_prefix(':')
            .ok_or_else(|| anyhow!("unexpected git diff output: {meta:?}"))?;
        let fields: Vec<&str> = meta.split_whitespace().collect();
        let [_, _, a_sha, b_sha, status] = fields.as_slice() else {
            bail!("unexpected git diff output: {meta:?}");
        };
        let mut next_path = || {
            tokens
                .next()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("truncated git diff output"))
        };
        let a_path = next_path()?;
        let b_path = if status.starts_with(['R', 'C']) {
            next_path()?
        } else {
            a_path.clone()
        };
        let blob = |sha: &str, absent: bool| (!absent && sha != NULL_SHA).then(|| sha.to_owned());
        entries.push(RawEntry {
            a_path,
            b_path,
            a_blob: blob(a_sha, status.starts_with('A')),
            b_blob: blob(b_sha, status.starts_with('D')),
        });
    }
    Ok(entries)
}

/// The clean filter configured for `git_path`, run over the file of the same name; `None` if no
/// filter applies.
fn apply_possible_filter(git_path: &str) -> Result<Option<Source>> {
    let Some(attr) = interrogate_filter(git_path)? else {
        return Ok(None);
    };
    let Some(command) = clean_filter_command(&attr) else {
        return Ok(None);
    };
    let input = fs::File::open(git_path).with_context(|| format!("opening {git_path}"))?;
    let output = Command::new("sh")
        .arg("-c")
        // Python merges the filter's stderr into its stdout.
        .arg(format!("exec 2>&1; {command}"))
        .stdin(Stdio::from(input))
        .output()
        .context("running the git filter")?;
    if !output.status.success() {
        bail!("git filter {command:?} failed ({})", output.status);
    }
    Ok(Some(Source::Stream {
        name: git_path.to_owned(),
        text: String::from_utf8_lossy(&output.stdout).into_owned(),
    }))
}

fn interrogate_filter(path: &str) -> Result<Option<String>> {
    let Ok(output) = Command::new("git")
        .args(["check-attr", "-z", "filter"])
        .arg(path)
        .stderr(Stdio::inherit())
        .output()
    else {
        return Ok(None);
    };
    if !output.status.success() {
        return Ok(None);
    }
    let parts: Vec<String> = output
        .stdout
        .split(|b| *b == 0)
        .take(3)
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect();
    let [path_list, attr, info] = parts.as_slice() else {
        return Ok(None);
    };
    if attr != "filter" || path_list != path {
        bail!("Unexpected output from git check-attr: {path_list:?} {attr:?}");
    }
    Ok(match info.as_str() {
        "" | "unspecified" | "set" | "unset" => None,
        other => Some(other.to_owned()),
    })
}

fn clean_filter_command(attr: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["config", "--get", "-z"])
        .arg(format!("filter.{attr}.clean"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let command = output.stdout.split(|b| *b == 0).next()?;
    let command = String::from_utf8_lossy(command).into_owned();
    (!command.is_empty()).then_some(command)
}

/// `_get_diff_entry_stream`: the notebook at `path` in `rev` (`None`: the working tree), the
/// null file if it is absent there, or `None` if `path` is not a notebook.
fn diff_entry_stream(
    repo: &Path,
    repo_dir: &Path,
    path: &str,
    blob: Option<&str>,
    rev: Option<&str>,
) -> Result<Option<Source>> {
    if !path.ends_with(".ipynb") {
        return Ok(None);
    }
    let Some(rev) = rev else {
        // `pushd` upstream restores the directory it was given, which is the current one.
        std::env::set_current_dir(repo_dir)
            .with_context(|| format!("changing to {}", repo_dir.display()))?;
        if let Some(filtered) = apply_possible_filter(path)? {
            return Ok(Some(filtered));
        }
        return Ok(Some(match fs::read(path) {
            Ok(bytes) => Source::Stream {
                name: path.to_owned(),
                text: String::from_utf8(bytes).map_err(|e| anyhow!("{path}: {e}"))?,
            },
            Err(_) => Source::Path(EXPLICIT_MISSING_FILE.into()),
        }));
    };
    let Some(blob) = blob else {
        return Ok(Some(Source::Path(EXPLICIT_MISSING_FILE.into())));
    };
    let bytes = output_of({
        let mut command = git(repo);
        command.args(["cat-file", "blob", blob]);
        command
    })?;
    let text = String::from_utf8(bytes).map_err(|e| anyhow!("{path}: {e}"))?;
    Ok(Some(Source::Stream {
        name: format!("{path} ({rev})"),
        text,
    }))
}

/// `changed_notebooks(base, remote, paths)`: calls `handle` with the base and remote notebook of
/// each changed `.ipynb` file between two revisions (`remote` of `None`: the working tree) and
/// stops at the first non-zero status it returns.
pub fn changed_notebooks(
    base: Option<&str>,
    remote: Option<&str>,
    paths: Option<&[String]>,
    mut handle: impl FnMut(Source, Source) -> Result<i32>,
) -> Result<i32> {
    let repo = get_repo(Path::new(".")).ok_or_else(|| anyhow!("not a git repository"))?;
    let root = repo
        .working_tree
        .clone()
        .ok_or_else(|| anyhow!("a bare repository has no working tree"))?;
    let cwd = std::env::current_dir().context("reading the current directory")?;
    let repo_dir = relpath(&root, &cwd);
    let paths: Vec<String> = match paths {
        Some(paths) if !repo.popped.is_empty() => paths
            .iter()
            .map(|p| {
                let mut joined: PathBuf = repo.popped.iter().collect();
                joined.push(p);
                joined.to_string_lossy().into_owned()
            })
            .collect(),
        Some(paths) => paths.to_vec(),
        None => Vec::new(),
    };

    let tree_of = |rev: Option<&str>| {
        rev_parse(&root, rev, "tree").ok_or_else(|| {
            anyhow!(
                "Ref {:?} did not resolve to a commit",
                rev.unwrap_or("HEAD")
            )
        })
    };
    let base_tree = tree_of(base)?;
    let mut command = git(&root);
    match remote {
        None => command.args(["diff", &base_tree]),
        Some(remote) => command.args(["diff-tree", &base_tree, &tree_of(Some(remote))?, "-r"]),
    };
    command.args([
        "--abbrev=40",
        "--full-index",
        "-M",
        "--raw",
        "-z",
        "--no-color",
    ]);
    let paths: Vec<&String> = paths.iter().filter(|p| !p.is_empty()).collect();
    if !paths.is_empty() {
        command.arg("--").args(paths);
    }
    let entries = parse_raw(&output_of(command)?)?;

    let mut status = 0;
    for entry in entries {
        let Some(fa) = diff_entry_stream(
            &root,
            &repo_dir,
            &entry.a_path,
            entry.a_blob.as_deref(),
            base,
        )?
        else {
            continue;
        };
        let Some(fb) = diff_entry_stream(
            &root,
            &repo_dir,
            &entry.b_path,
            entry.b_blob.as_deref(),
            remote,
        )?
        else {
            continue;
        };
        status = handle(fa, fb)?;
        if status != 0 {
            break;
        }
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relpath_between_directories() {
        let root = Path::new("/r");
        assert_eq!(relpath(root, Path::new("/r/sub")), PathBuf::from(".."));
        assert_eq!(relpath(root, root), PathBuf::from("."));
        assert_eq!(
            relpath(Path::new("/r/a"), Path::new("/r/b/c")),
            PathBuf::from("../../a")
        );
    }

    #[test]
    fn raw_diff_format() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let raw = format!(
            ":100644 100644 {a} {b} M\0x.ipynb\0:000000 100644 {NULL_SHA} {b} A\0new.ipynb\0\
             :100644 000000 {a} {NULL_SHA} D\0gone.ipynb\0:100644 100644 {a} {b} R090\0old.ipynb\0moved.ipynb\0"
        );
        let entries = parse_raw(raw.as_bytes()).unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].a_blob.as_deref(), Some(a.as_str()));
        assert_eq!(entries[1].a_blob, None);
        assert_eq!(entries[1].b_blob.as_deref(), Some(b.as_str()));
        assert_eq!(entries[2].b_blob, None);
        assert_eq!(entries[3].a_path, "old.ipynb");
        assert_eq!(entries[3].b_path, "moved.ipynb");
        assert!(parse_raw(b"garbage\0").is_err());
    }

    #[test]
    fn working_tree_diff_has_no_b_blob() {
        let a = "a".repeat(40);
        let raw = format!(":100644 100644 {a} {NULL_SHA} M\0x.ipynb\0");
        let entries = parse_raw(raw.as_bytes()).unwrap();
        assert_eq!(entries[0].b_blob, None);
    }

    #[test]
    fn names_that_are_not_revisions() {
        assert!(!is_gitref(Some("/dev/null")).unwrap());
        assert!(!is_gitref(Some("/")).unwrap());
        assert!(!is_gitref(Some("")).unwrap());
    }

    #[test]
    fn repo_discovery_stops_at_the_root() {
        assert_eq!(get_repo(Path::new("/proc/self/fd")), None);
    }
}
