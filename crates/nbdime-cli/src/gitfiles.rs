//! Notebooks in git revisions (`nbdime.gitfiles`, `nbdime.vcs.git.filter_integration`).
//!
//! Upstream drives git through GitPython; here the same questions go to `git` subprocesses:
//! `rev-parse` for revisions and the `diff` and `diff-tree` raw formats GitPython parses, and
//! `check-attr` and `config` for clean filters.

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};

use crate::osstr;
use crate::pathexpand::{Env, ProcessEnv};
use crate::revparse::{resolve_commit, tree_id};
use crate::utils::{Source, EXPLICIT_MISSING_FILE};

/// A git repository found from a directory (GitPython's `Repo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    /// `None` for a bare repository.
    pub working_tree: Option<PathBuf>,
    /// Directories between the working tree and the directory the search started in.
    pub popped: Vec<String>,
}

impl Repo {
    /// The directory git commands run in (`Repo.working_dir`).
    pub(crate) fn work_dir(&self) -> &Path {
        self.working_tree.as_deref().unwrap_or(&self.common_dir)
    }

    /// A git command as GitPython runs it: in the working directory, and with the repository
    /// that was found pinned in the environment when the environment named one.
    pub(crate) fn git(&self) -> Command {
        self.git_with(&ProcessEnv)
    }

    fn git_with(&self, env: &dyn Env) -> Command {
        let mut command = Command::new("git");
        command
            .current_dir(self.work_dir())
            .env("LANGUAGE", "C")
            .env("LC_ALL", "C")
            .stdin(Stdio::null());
        if env.get("GIT_COMMON_DIR").is_some() {
            command
                .env("GIT_DIR", &self.git_dir)
                .env("GIT_COMMON_DIR", &self.common_dir);
        } else if env.get("GIT_DIR").is_some() {
            command.env("GIT_DIR", &self.git_dir);
        }
        if let Some(objects) = env.get("GIT_OBJECT_DIRECTORY") {
            if let Ok(objects) = std::path::absolute(osstr::to_path(&objects)) {
                command.env("GIT_OBJECT_DIRECTORY", objects);
            }
        }
        command
    }
}

/// What `git.repo.fun.is_git_dir` makes of a directory.
enum GitDirKind {
    Yes,
    No,
    /// A linked worktree's administrative directory in a state GitPython refuses
    /// (`WorkTreeRepositoryUnsupported`, an `InvalidGitRepositoryError`).
    Unsupported,
}

fn var(name: &str) -> Option<String> {
    ProcessEnv.get(name)
}

/// `os.path.normpath` for paths with a root or without.
fn normpath(path: &Path) -> PathBuf {
    let mut parts: Vec<Component> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match parts.last() {
                Some(Component::Normal(_)) => {
                    parts.pop();
                }
                Some(Component::RootDir) => {}
                _ => parts.push(component),
            },
            other => parts.push(other),
        }
    }
    let result: PathBuf = parts.iter().collect();
    if result.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        result
    }
}

fn is_git_dir(dir: &Path) -> GitDirKind {
    if !dir.is_dir() {
        return GitDirKind::No;
    }
    let head = dir.join("HEAD");
    let valid_head = match fs::symlink_metadata(&head) {
        Ok(meta) if meta.file_type().is_symlink() => fs::read_link(&head)
            .map(|target| target.starts_with("refs"))
            .unwrap_or(false),
        _ => fs::File::open(&head)
            .and_then(|mut file| {
                let mut buffer = Vec::new();
                file.by_ref().take(256).read_to_end(&mut buffer)?;
                Ok(buffer)
            })
            .map(|head| {
                let is_sha = head.len() >= 40 && head[..40].iter().all(u8::is_ascii_hexdigit);
                let symbolic = head
                    .strip_prefix(b"ref:")
                    .is_some_and(|rest| rest.trim_ascii_start().starts_with(b"refs/"));
                symbolic || is_sha
            })
            .unwrap_or(false),
    };
    let common_dir = match var("GIT_COMMON_DIR") {
        Some(value) if value.is_empty() => return GitDirKind::No,
        Some(value) => osstr::to_path(&value),
        None => match fs::read(dir.join("commondir")) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if fs::symlink_metadata(dir.join("commondir")).is_ok() {
                    return GitDirKind::No;
                }
                dir.to_path_buf()
            }
            Err(_) => return GitDirKind::No,
            Ok(bytes) => {
                let text = osstr::from_bytes(&bytes);
                let text = text.trim_end_matches(['\r', '\n']);
                if text.is_empty() {
                    return GitDirKind::No;
                }
                match fs::canonicalize(dir.join(osstr::to_path(text))) {
                    Ok(path) => path,
                    Err(_) => normpath(&dir.join(osstr::to_path(text))),
                }
            }
        },
    };
    let objects = var("GIT_OBJECT_DIRECTORY")
        .map(|d| osstr::to_path(&d))
        .unwrap_or_else(|| common_dir.join("objects"));
    if valid_head && objects.is_dir() && common_dir.join("refs").is_dir() {
        return GitDirKind::Yes;
    }
    if dir.join("gitdir").is_file() && dir.join("commondir").is_file() && head.is_file() {
        return GitDirKind::Unsupported;
    }
    GitDirKind::No
}

/// `find_submodule_git_dir`: the git directory that `dotgit` (a directory, or a file naming one)
/// stands for.
fn find_submodule_git_dir(dotgit: &Path) -> Option<Result<PathBuf, ()>> {
    match is_git_dir(dotgit) {
        GitDirKind::Yes => return Some(Ok(dotgit.to_path_buf())),
        GitDirKind::Unsupported => return Some(Err(())),
        GitDirKind::No => {}
    }
    let meta = fs::metadata(dotgit).ok()?;
    if !meta.is_file() || meta.len() > 1 << 20 {
        return None;
    }
    let bytes = fs::read(dotgit).ok()?;
    let text = std::str::from_utf8(&bytes)
        .ok()?
        .trim_end_matches(['\r', '\n']);
    let target = text.strip_prefix("gitdir: ").filter(|t| !t.is_empty())?;
    let mut target = osstr::to_path(target);
    if target.is_relative() {
        target = normpath(&dotgit.parent()?.join(target));
    }
    match is_git_dir(&target) {
        GitDirKind::Yes => Some(Ok(target)),
        GitDirKind::Unsupported => Some(Err(())),
        GitDirKind::No => None,
    }
}

/// `core.bare` of a repository's own config file, which is all GitPython's reader looks at.
fn config_says_bare(config: &Path) -> bool {
    let Ok(text) = fs::read_to_string(config) else {
        return false;
    };
    let mut in_core = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(section) = line.strip_prefix('[') {
            in_core = section
                .split(']')
                .next()
                .is_some_and(|name| name.trim().eq_ignore_ascii_case("core"));
        } else if in_core {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim().eq_ignore_ascii_case("bare") {
                    return matches!(
                        value.trim().to_ascii_lowercase().as_str(),
                        "true" | "yes" | "on" | "1"
                    );
                }
            }
        }
    }
    false
}

/// `Repo(path)` without searching parent directories.
fn repo_at(dir: &Path) -> Option<Repo> {
    let dotgit = dir.join(".git");
    let (git_dir, mut working_tree) = match find_submodule_git_dir(&dotgit) {
        Some(Ok(git_dir)) => (normpath(&dir.join(git_dir)), dir.to_path_buf()),
        Some(Err(())) => return None,
        None => {
            // Like git, never fall back to the parent when `.git` is a file that is not a gitfile.
            if dotgit.exists() && !dotgit.is_dir() {
                return None;
            }
            match is_git_dir(dir) {
                GitDirKind::Yes => {}
                GitDirKind::No | GitDirKind::Unsupported => return None,
            }
            let linked = dir.join("gitdir").is_file() && dir.join("commondir").is_file();
            let tree = if linked {
                match var("GIT_WORK_TREE") {
                    Some(tree) => osstr::to_path(&tree),
                    None => {
                        let text = osstr::from_bytes(&fs::read(dir.join("gitdir")).ok()?);
                        let mut gitfile = osstr::to_path(text.trim());
                        if gitfile.is_relative() {
                            gitfile = normpath(&dir.join(gitfile));
                        }
                        gitfile.parent()?.to_path_buf()
                    }
                }
            } else {
                var("GIT_WORK_TREE")
                    .map(|tree| osstr::to_path(&tree))
                    .unwrap_or_else(|| dir.parent().map(Path::to_path_buf).unwrap_or_default())
            };
            (dir.to_path_buf(), tree)
        }
    };
    let common_dir = match var("GIT_COMMON_DIR") {
        Some(common) => std::path::absolute(osstr::to_path(&common)).ok()?,
        None => match fs::read(git_dir.join("commondir")) {
            Ok(bytes) => {
                let text = osstr::from_bytes(&bytes);
                git_dir.join(osstr::to_path(text.trim_end_matches(['\r', '\n'])))
            }
            Err(_) => PathBuf::new(),
        },
    };
    let config_dir = if common_dir.as_os_str().is_empty() {
        &git_dir
    } else {
        &common_dir
    };
    let mut bare = config_says_bare(&config_dir.join("config"));
    // A linked worktree is not bare even when its main repository is.
    if bare && git_dir.join("commondir").is_file() {
        bare = false;
    }
    let common_dir = if common_dir.as_os_str().is_empty() {
        git_dir.clone()
    } else {
        common_dir
    };
    if bare {
        working_tree = PathBuf::new();
    }
    Some(Repo {
        git_dir,
        common_dir,
        working_tree: (!bare).then_some(working_tree),
        popped: Vec::new(),
    })
}

/// `get_repo(path)`: the repository `path` is in (or is the top of), or `None`.
pub fn get_repo(path: &Path) -> Option<Repo> {
    let absolute = std::path::absolute(path).ok()?;
    let mut current = fs::canonicalize(&absolute).unwrap_or(absolute);
    let mut popped = Vec::new();
    loop {
        if let Some(mut repo) = repo_at(&current) {
            repo.popped = popped;
            return Some(repo);
        }
        let name = osstr::from_os(current.file_name()?);
        popped.insert(0, name);
        current = current.parent()?.to_path_buf();
    }
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

/// `is_valid_gitref(ref)`: whether the current directory is in a repository that has the
/// revision. `None` asks for `HEAD`; a repository without commits then raises upstream.
fn is_valid_gitref(rev: Option<&str>) -> Result<bool> {
    let Some(repo) = get_repo(Path::new(".")) else {
        return Ok(false);
    };
    let Some(rev) = rev else {
        return match resolve_commit(&repo, "HEAD")? {
            Some(_) => Ok(true),
            None => bail!("Reference at 'HEAD' does not exist"),
        };
    };
    Ok(resolve_commit(&repo, rev)?.is_some())
}

/// `is_gitref(candidate)`: a revision, as opposed to a file name. A name that is both is a file.
pub fn is_gitref(candidate: Option<&str>) -> Result<bool> {
    let usable = match candidate {
        None => true,
        Some(c) => !osstr::to_path(c).exists() && c != EXPLICIT_MISSING_FILE && !c.is_empty(),
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
    let text = osstr::from_bytes(output);
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
    let input = fs::File::open(osstr::to_path(git_path))
        .with_context(|| format!("opening {}", osstr::show(git_path)))?;
    // `shell=True` runs `/bin/sh`, whatever `sh` the PATH offers.
    let output = Command::new("/bin/sh")
        .arg("-c")
        // Python merges the filter's stderr into its stdout.
        .arg(osstr::to_os(&format!("exec 2>&1; {command}")))
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
        .arg(osstr::to_os(path))
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
        .map(osstr::from_bytes)
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
    repo: &Repo,
    repo_dir: &Path,
    path: &str,
    blob: Option<&str>,
    rev: Option<&str>,
) -> Result<Option<Source>> {
    if !path.ends_with(".ipynb") {
        return Ok(None);
    }
    let Some(rev) = rev else {
        // Upstream's `pushd` goes back to `os.curdir`, which is `"."`: the process stays in
        // `repo_dir`, and `repo_dir` (relative to where it started) is applied again for every
        // further entry. Timestamps and later files depend on it, so it is reproduced.
        std::env::set_current_dir(repo_dir)
            .with_context(|| format!("changing to {}", repo_dir.display()))?;
        if let Some(filtered) = apply_possible_filter(path)? {
            return Ok(Some(filtered));
        }
        return Ok(Some(match fs::read(osstr::to_path(path)) {
            Ok(bytes) => Source::Stream {
                name: path.to_owned(),
                text: String::from_utf8(bytes)
                    .map_err(|e| anyhow!("{}: {e}", osstr::show(path)))?,
            },
            Err(_) => Source::Path(EXPLICIT_MISSING_FILE.into()),
        }));
    };
    let Some(blob) = blob else {
        return Ok(Some(Source::Path(EXPLICIT_MISSING_FILE.into())));
    };
    let bytes = output_of({
        let mut command = repo.git();
        command.args(["cat-file", "blob", blob]);
        command
    })?;
    let text = String::from_utf8(bytes).map_err(|e| anyhow!("{}: {e}", osstr::show(path)))?;
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
    // `os.path.relpath(None, ...)` is a TypeError upstream.
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
                let mut joined: PathBuf = repo.popped.iter().map(|d| osstr::to_path(d)).collect();
                joined.push(osstr::to_path(p));
                osstr::from_os(joined.as_os_str())
            })
            .collect(),
        Some(paths) => paths.to_vec(),
        None => Vec::new(),
    };

    let tree_of = |rev: Option<&str>| -> Result<String> {
        let name = rev.unwrap_or("HEAD");
        let commit = resolve_commit(&repo, name)?
            .ok_or_else(|| anyhow!("Ref {name:?} did not resolve to a commit"))?;
        tree_id(&repo, &commit).ok_or_else(|| anyhow!("Ref {name:?} has no tree"))
    };
    let base_tree = tree_of(base)?;
    let mut command = repo.git();
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
    if !paths.is_empty() {
        command
            .arg("--")
            .args(paths.iter().map(|p| osstr::to_os(p)));
    }
    let entries = parse_raw(&output_of(command)?)?;

    let mut status = 0;
    for entry in entries {
        let Some(fa) = diff_entry_stream(
            &repo,
            &repo_dir,
            &entry.a_path,
            entry.a_blob.as_deref(),
            base,
        )?
        else {
            continue;
        };
        let Some(fb) = diff_entry_stream(
            &repo,
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
    use crate::testutil::TempDir;

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

    fn sandbox(name: &str) -> TempDir {
        TempDir::new(&format!("gitfiles-{name}"))
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("HOME", dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
    }

    /// A repository with two commits, a tag on the first and a subdirectory.
    fn repository(dir: &TempDir) -> PathBuf {
        let repo = fs::canonicalize(&dir.0).unwrap().join("repo");
        fs::create_dir_all(repo.join("sub/deeper")).unwrap();
        run_git(&repo, &["init", "-q", "-b", "main"]);
        fs::write(repo.join("a.ipynb"), "1").unwrap();
        run_git(&repo, &["add", "-A"]);
        run_git(&repo, &["commit", "-q", "-m", "one fix"]);
        run_git(&repo, &["tag", "v1"]);
        run_git(&repo, &["tag", "treetag", "HEAD^{tree}"]);
        fs::write(repo.join("a.ipynb"), "2").unwrap();
        run_git(&repo, &["commit", "-q", "-am", "two"]);
        repo
    }

    #[test]
    fn children_are_pinned_to_the_repository_the_environment_named() {
        let dir = sandbox("pinned");
        let repo = get_repo(&repository(&dir)).unwrap();
        let pinned = |env: &[(&str, &str)]| -> Vec<(String, String)> {
            let env: std::collections::HashMap<String, String> = env
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect();
            repo.git_with(&env)
                .get_envs()
                .filter(|(k, _)| k.to_string_lossy().starts_with("GIT_"))
                .map(|(k, v)| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.unwrap().to_string_lossy().into_owned(),
                    )
                })
                .collect()
        };
        assert!(pinned(&[]).is_empty());
        let git_dir = repo.git_dir.to_string_lossy().into_owned();
        // GitPython exports the repository it found, as an absolute path, whatever the
        // environment's spelling was.
        assert_eq!(
            pinned(&[("GIT_DIR", ".git")]),
            [("GIT_DIR".to_owned(), git_dir.clone())]
        );
        let common = repo.common_dir.to_string_lossy().into_owned();
        let mut both = pinned(&[("GIT_COMMON_DIR", "x")]);
        both.sort();
        assert_eq!(
            both,
            [
                ("GIT_COMMON_DIR".to_owned(), common),
                ("GIT_DIR".to_owned(), git_dir)
            ]
        );
    }

    #[test]
    fn repositories_are_found_from_inside_them() {
        let dir = sandbox("find");
        let repo = repository(&dir);
        let found = get_repo(&repo).unwrap();
        assert_eq!(found.working_tree.as_deref(), Some(repo.as_path()));
        assert!(found.popped.is_empty());
        assert_eq!(found.git_dir, repo.join(".git"));
        let sub = get_repo(&repo.join("sub/deeper")).unwrap();
        assert_eq!(sub.working_tree.as_deref(), Some(repo.as_path()));
        assert_eq!(sub.popped, ["sub", "deeper"]);
        // Inside the git directory the parent is the working tree.
        let inside = get_repo(&repo.join(".git")).unwrap();
        assert_eq!(inside.working_tree.as_deref(), Some(repo.as_path()));
        assert!(inside.popped.is_empty());
        let objects = get_repo(&repo.join(".git/objects")).unwrap();
        assert_eq!(objects.working_tree.as_deref(), Some(repo.as_path()));
        assert_eq!(objects.popped, ["objects"]);
    }

    #[test]
    fn linked_worktrees_follow_the_common_directory() {
        let dir = sandbox("worktree");
        let repo = repository(&dir);
        let linked = repo.parent().unwrap().join("linked");
        run_git(
            &repo,
            &["worktree", "add", "-q", linked.to_str().unwrap(), "HEAD"],
        );
        let found = get_repo(&linked.join("..").join("linked")).unwrap();
        assert_eq!(found.working_tree.as_deref(), Some(linked.as_path()));
        assert_eq!(normpath(&found.common_dir), repo.join(".git"));
        assert!(found.git_dir.starts_with(repo.join(".git/worktrees")));
        let commit = resolve_commit(&found, "HEAD").unwrap();
        assert!(commit.is_some());
        // The administrative directory itself is a worktree as well.
        let admin = get_repo(&found.git_dir).unwrap();
        assert_eq!(admin.working_tree.as_deref(), Some(linked.as_path()));
    }

    #[test]
    fn bare_repositories_have_no_working_tree() {
        let dir = sandbox("bare");
        let repo = repository(&dir);
        let bare = repo.parent().unwrap().join("bare.git");
        run_git(
            &repo,
            &["clone", "-q", "--bare", ".", bare.to_str().unwrap()],
        );
        let found = get_repo(&bare).unwrap();
        assert_eq!(found.working_tree, None);
        assert_eq!(found.work_dir(), found.common_dir);
        assert!(resolve_commit(&found, "HEAD~1").unwrap().is_some());
        assert!(get_repo(&bare.join("refs")).unwrap().working_tree.is_none());
    }

    #[test]
    fn directories_outside_repositories_are_not_found() {
        let dir = sandbox("outside");
        let plain = fs::canonicalize(&dir.0).unwrap().join("plain");
        fs::create_dir_all(&plain).unwrap();
        // A `.git` file that is not a gitfile ends the search like git does for GitPython.
        fs::write(plain.join(".git"), "not a gitfile").unwrap();
        assert_eq!(repo_at(&plain), None);
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
