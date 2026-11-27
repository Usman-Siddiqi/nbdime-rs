//! The command line tools run as processes: real git repositories (working trees,
//! subdirectories, linked worktrees, bare repositories), names that are not UTF-8 and exit
//! statuses. Every child gets a private HOME and git configuration.
#![cfg(unix)]

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("nbdime-rs-cli-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home")).unwrap();
        Sandbox {
            root: fs::canonicalize(root).unwrap(),
        }
    }

    fn dir(&self, name: &str) -> PathBuf {
        let dir = self.root.join(name);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A child process with an isolated environment, run in `cwd`.
    fn command(&self, program: impl AsRef<OsStr>, cwd: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(cwd)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", self.root.join("home"))
            .env("JUPYTER_CONFIG_DIR", self.root.join("home/jupyter"))
            .env("JUPYTER_NO_CONFIG", "1")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CEILING_DIRECTORIES", &self.root)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .env("LC_ALL", "C.UTF-8");
        command
    }

    fn git(&self, cwd: &Path, args: &[&str]) {
        let output = self.command("git", cwd).args(args).output().unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
    }

    fn nbdiff(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_nbdiff"), cwd)
            .args(args)
            .output()
            .unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn notebook(source: &str) -> String {
    format!(
        r#"{{"cells": [{{"cell_type": "code", "execution_count": null, "id": "cell0001", "metadata": {{}}, "outputs": [], "source": ["{source}"]}}], "metadata": {{}}, "nbformat": 4, "nbformat_minor": 5}}"#
    )
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A repository with two commits of `a.ipynb` (`x = 1`, `x = 2`) and `sub/b.ipynb`.
fn repository(sandbox: &Sandbox, name: &str) -> PathBuf {
    let repo = sandbox.dir(name);
    sandbox.git(&repo, &["init", "-q", "-b", "main"]);
    fs::create_dir_all(repo.join("sub")).unwrap();
    fs::write(repo.join("a.ipynb"), notebook("x = 1")).unwrap();
    fs::write(repo.join("sub/b.ipynb"), notebook("y = 1")).unwrap();
    sandbox.git(&repo, &["add", "-A"]);
    sandbox.git(&repo, &["commit", "-q", "-m", "first change"]);
    fs::write(repo.join("a.ipynb"), notebook("x = 2")).unwrap();
    sandbox.git(&repo, &["commit", "-q", "-am", "second"]);
    repo
}

const CHANGED_A: &str = "## modified /cells/0/source:";

#[test]
fn working_tree_changes_are_found_from_the_top_and_from_subdirectories() {
    let sandbox = Sandbox::new("tree");
    let repo = repository(&sandbox, "repo");
    fs::write(repo.join("a.ipynb"), notebook("x = 3")).unwrap();
    for cwd in [repo.clone(), repo.join("sub")] {
        let output = sandbox.nbdiff(&cwd, &["--no-color"]);
        assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
        let stdout = text(&output.stdout);
        assert!(
            stdout.starts_with("nbdiff a.ipynb (HEAD) a.ipynb\n"),
            "{stdout}"
        );
        assert!(stdout.contains(CHANGED_A), "{stdout}");
    }
}

#[test]
fn staged_and_unstaged_changes_both_show_against_a_revision() {
    let sandbox = Sandbox::new("staged");
    let repo = repository(&sandbox, "repo");
    fs::write(repo.join("a.ipynb"), notebook("x = 3")).unwrap();
    sandbox.git(&repo, &["add", "a.ipynb"]);
    let staged = sandbox.nbdiff(&repo, &["--no-color", "HEAD~1"]);
    assert_eq!(staged.status.code(), Some(0));
    assert!(
        text(&staged.stdout).contains("-  x = 1\n+  x = 3"),
        "{}",
        text(&staged.stdout)
    );
    fs::write(repo.join("a.ipynb"), notebook("x = 4")).unwrap();
    let unstaged = sandbox.nbdiff(&repo, &["--no-color", "HEAD~1"]);
    assert!(text(&unstaged.stdout).contains("-  x = 1\n+  x = 4"));
}

#[test]
fn linked_worktrees_are_repositories() {
    let sandbox = Sandbox::new("worktree");
    let repo = repository(&sandbox, "repo");
    let worktree = sandbox.root.join("wt");
    sandbox.git(
        &repo,
        &["worktree", "add", "-q", worktree.to_str().unwrap(), "HEAD"],
    );
    fs::write(worktree.join("a.ipynb"), notebook("x = 9")).unwrap();
    for args in [
        &["--no-color"][..],
        &["--no-color", "HEAD~1", "HEAD"],
        &["--no-color", "HEAD", "a.ipynb"],
    ] {
        let output = sandbox.nbdiff(&worktree, args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            text(&output.stderr)
        );
        assert!(
            text(&output.stdout).contains(CHANGED_A),
            "{args:?}: {}",
            text(&output.stdout)
        );
    }
}

#[test]
fn a_working_directory_inside_dot_git_uses_the_parent_as_working_tree() {
    let sandbox = Sandbox::new("dotgit");
    let repo = repository(&sandbox, "repo");
    let output = sandbox.nbdiff(&repo.join(".git"), &["--no-color", "main~1", "main"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert!(text(&output.stdout).contains(CHANGED_A));
}

#[test]
fn bare_repositories_have_no_working_tree() {
    let sandbox = Sandbox::new("bare");
    let repo = repository(&sandbox, "repo");
    let bare = sandbox.root.join("bare.git");
    sandbox.git(
        &repo,
        &["clone", "-q", "--bare", ".", bare.to_str().unwrap()],
    );
    let output = sandbox.nbdiff(&bare, &["--no-color", "HEAD~1", "HEAD"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn outside_a_repository_a_revision_is_a_missing_file() {
    let sandbox = Sandbox::new("outside");
    let dir = sandbox.dir("plain");
    let output = sandbox.nbdiff(&dir, &["HEAD"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(text(&output.stdout), "Missing file HEAD\n");
}

#[test]
fn revision_forms_follow_gitpython() {
    let sandbox = Sandbox::new("revisions");
    let repo = repository(&sandbox, "repo");
    // `:/text` finds the commit by its message.
    let found = sandbox.nbdiff(&repo, &["--no-color", ":/first change"]);
    assert_eq!(found.status.code(), Some(0), "{}", text(&found.stderr));
    assert!(text(&found.stdout).contains(CHANGED_A));
    // Objects that are not commits, and ranges, are errors with a message on stderr.
    for rev in [
        "HEAD^{tree}",
        "HEAD:a.ipynb",
        "HEAD~1..HEAD",
        "HEAD~1...HEAD",
        "HEAD^!",
        "HEAD^{foo}",
        "HEAD^{blob}",
        "HEAD^{tree}~1",
        "HEAD@{9}",
        "@{-5}",
        "HEAD@{-1}",
        "HEAD@{yesterday}",
        ":/",
        ":/!x",
        "HEAD:nope.ipynb",
        "deadbee@{1}",
        "unknown@{1}",
    ] {
        let output = sandbox.nbdiff(&repo, &["--no-color", rev]);
        assert_eq!(output.status.code(), Some(1), "{rev}");
        assert!(output.stdout.is_empty(), "{rev}: {}", text(&output.stdout));
        assert!(!output.stderr.is_empty(), "{rev}");
    }
    // A name that git does not know is a file, and so is a `:path` that is not in the index
    // (GitPython raises `BadName` for it).
    for name in ["nonexistent", ":nope.ipynb"] {
        let missing = sandbox.nbdiff(&repo, &["--no-color", name, "a.ipynb"]);
        assert_eq!(text(&missing.stdout), format!("Missing file {name}\n"));
    }
    // A path in the index is a blob, which is no commit.
    let blob = sandbox.nbdiff(&repo, &["--no-color", ":a.ipynb"]);
    assert_eq!(blob.status.code(), Some(1));
    assert!(blob.stdout.is_empty() && !blob.stderr.is_empty());
}

#[test]
fn notebook_text_is_never_rewritten_like_a_file_name() {
    let sandbox = Sandbox::new("private-use");
    let dir = sandbox.dir("nb");
    // U+10F780 stands for an undecodable byte in names; in notebook text it is just a character.
    let text_in = "x = '\u{10F780}\u{10F7FF}'";
    fs::write(dir.join("a.ipynb"), notebook("x = 1")).unwrap();
    fs::write(dir.join("b.ipynb"), notebook(text_in)).unwrap();
    let output = sandbox.nbdiff(&dir, &["--no-color", "a.ipynb", "b.ipynb"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = text(&output.stdout);
    assert!(stdout.contains(text_in), "{stdout}");
    assert!(!stdout.contains("\\udc"), "{stdout}");
    // The same text on stderr, through the decisions of a merge.
    let merge = sandbox
        .command(env!("CARGO_BIN_EXE_nbmerge"), &dir)
        .args(["a.ipynb", "a.ipynb", "b.ipynb", "--decisions"])
        .output()
        .unwrap();
    assert!(
        !text(&merge.stderr).contains("\\udc"),
        "{}",
        text(&merge.stderr)
    );
}

#[test]
fn empty_pathspecs_are_git_errors() {
    let sandbox = Sandbox::new("pathspec");
    let repo = repository(&sandbox, "repo");
    for args in [&["HEAD", ""][..], &[""], &["HEAD~1", "HEAD", "", "a.ipynb"]] {
        let output = sandbox.nbdiff(&repo, args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(
            text(&output.stderr).contains("empty string is not a valid pathspec"),
            "{args:?}"
        );
    }
}

fn non_utf8(name: &[u8]) -> std::ffi::OsString {
    std::ffi::OsString::from_vec(name.to_vec())
}

// APFS refuses names that are not UTF-8.
#[cfg(not(target_os = "macos"))]
#[test]
fn names_that_are_not_utf8_are_opened_and_shown_like_python() {
    let sandbox = Sandbox::new("bytes");
    let dir = sandbox.dir("files");
    let odd = dir.join(non_utf8(b"\xff.ipynb"));
    fs::write(&odd, notebook("x = 1")).unwrap();
    fs::write(dir.join("o.ipynb"), notebook("x = 2")).unwrap();
    let output = sandbox
        .command(env!("CARGO_BIN_EXE_nbdiff"), &dir)
        .arg("--no-color")
        .arg(&odd)
        .arg("o.ipynb")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let stdout = text(&output.stdout);
    let shown = format!("{}", odd.display());
    assert!(!shown.is_empty());
    assert!(stdout.contains("\\udcff.ipynb"), "{stdout}");
    assert!(stdout.contains(CHANGED_A), "{stdout}");

    // A missing file is reported with the escaped name on stdout.
    let output = sandbox
        .command(env!("CARGO_BIN_EXE_nbdiff"), &dir)
        .arg(non_utf8(b"gone\xfe.ipynb"))
        .arg("o.ipynb")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(text(&output.stdout), "Missing file gone\\udcfe.ipynb\n");

    // `nbmerge --out` writes the file with the exact bytes and logs the escaped name.
    let target = non_utf8(b"m\xfe.ipynb");
    let output = sandbox
        .command(env!("CARGO_BIN_EXE_nbmerge"), &dir)
        .arg(&odd)
        .arg(&odd)
        .arg("o.ipynb")
        .arg("--out")
        .arg(&target)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert!(dir.join(&target).is_file());
    assert!(
        text(&output.stderr).contains("Merge result written to m\\udcfe.ipynb"),
        "{}",
        text(&output.stderr)
    );
    let listing: Vec<Vec<u8>> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().as_bytes().to_vec())
        .collect();
    assert!(listing.contains(&b"m\xfe.ipynb".to_vec()));
}

#[test]
fn a_negative_number_may_end_in_a_newline() {
    let sandbox = Sandbox::new("negative");
    let dir = sandbox.dir("files");
    fs::write(dir.join("o.ipynb"), notebook("x = 2")).unwrap();
    let output = sandbox.nbdiff(&dir, &["-1\n", "o.ipynb"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(text(&output.stdout), "Missing file -1\n\n");
}

#[test]
fn mergetool_exits_like_python_for_a_git_killed_by_a_signal() {
    let sandbox = Sandbox::new("mergetool");
    let bin = sandbox.dir("bin");
    let fake_git = bin.join("git");
    fs::write(&fake_git, "#!/bin/sh\nkill -TERM $$\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&fake_git, fs::Permissions::from_mode(0o755)).unwrap();
    let output = sandbox
        .command(env!("CARGO_BIN_EXE_nbdime"), &bin)
        .env("PATH", &bin)
        .args(["mergetool", "x.txt", "it's.md", "a.ipynb"])
        .output()
        .unwrap();
    // `sys.exit(-15)`.
    assert_eq!(output.status.code(), Some(241));
    assert_eq!(
        text(&output.stderr),
        "Skipping given filenames without .ipynb extension: ['x.txt', \"it's.md\"]\n"
    );
}

#[test]
fn clean_filters_run_in_bin_sh() {
    let sandbox = Sandbox::new("filter");
    let repo = repository(&sandbox, "repo");
    // A `sh` earlier on the PATH must not be what runs the filter.
    let bin = sandbox.dir("bin");
    let fake_sh = bin.join("sh");
    fs::write(&fake_sh, "#!/bin/sh\necho fake sh used >&2\nexit 7\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&fake_sh, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(repo.join(".gitattributes"), "*.ipynb filter=nbclean\n").unwrap();
    sandbox.git(&repo, &["config", "filter.nbclean.clean", "cat"]);
    fs::write(repo.join("a.ipynb"), notebook("x = 5")).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let output = sandbox
        .command(env!("CARGO_BIN_EXE_nbdiff"), &repo)
        .env("PATH", path)
        .arg("--no-color")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert!(text(&output.stdout).contains(CHANGED_A));
}

// `ulimit -v` (RLIMIT_AS) is not enforced on macOS.
#[cfg(target_os = "linux")]
#[test]
fn a_small_address_space_gets_a_smaller_stack_instead_of_a_silent_exit() {
    let sandbox = Sandbox::new("ulimit");
    let dir = sandbox.dir("files");
    fs::write(dir.join("a.ipynb"), notebook("x = 1")).unwrap();
    fs::write(dir.join("b.ipynb"), notebook("x = 2")).unwrap();
    let output = sandbox
        .command("sh", &dir)
        .args([
            "-c",
            "ulimit -v 450000 && exec \"$0\" --no-color a.ipynb b.ipynb",
        ])
        .arg(env!("CARGO_BIN_EXE_nbdiff"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert!(text(&output.stdout).contains(CHANGED_A));
}

fn run_in(sandbox: &Sandbox, binary: &str, cwd: &Path, args: &[&str]) -> Output {
    sandbox.command(binary, cwd).args(args).output().unwrap()
}

fn config_dir(sandbox: &Sandbox, name: &str, config: &str) -> PathBuf {
    let dir = sandbox.dir(name);
    fs::write(dir.join("nbdime_config.json"), config).unwrap();
    fs::write(dir.join("a.ipynb"), notebook("x = 1")).unwrap();
    fs::write(dir.join("b.ipynb"), notebook("x = 2")).unwrap();
    dir
}

#[test]
fn config_values_of_the_wrong_type_fail_where_upstream_fails() {
    let sandbox = Sandbox::new("config-types");
    // `col_const[use_color]` is a KeyError once a colour is needed: the header is already out.
    for value in ["\"no\"", "2", "[]", "\"\""] {
        let dir = config_dir(
            &sandbox,
            "color",
            &format!(r#"{{"NbDiff": {{"use_color": {value}}}}}"#),
        );
        let output = run_in(
            &sandbox,
            env!("CARGO_BIN_EXE_nbdiff"),
            &dir,
            &["a.ipynb", "b.ipynb"],
        );
        assert_eq!(output.status.code(), Some(1), "{value}");
        assert!(
            text(&output.stdout).starts_with("nbdiff a.ipynb b.ipynb\n--- a.ipynb  "),
            "{value}"
        );
        assert_eq!(text(&output.stdout).lines().count(), 3, "{value}");
        assert!(!output.stderr.is_empty(), "{value}");
    }
    for value in ["1", "0", "true", "false", "1.0"] {
        let dir = config_dir(
            &sandbox,
            "color",
            &format!(r#"{{"NbDiff": {{"use_color": {value}}}}}"#),
        );
        let output = run_in(
            &sandbox,
            env!("CARGO_BIN_EXE_nbdiff"),
            &dir,
            &["a.ipynb", "b.ipynb"],
        );
        assert_eq!(output.status.code(), Some(0), "{value}");
    }
    // `paths = [base, remote] + paths` with a string.
    let dir = config_dir(&sandbox, "paths", r#"{"NbDiff": {"paths": "x"}}"#);
    let output = run_in(
        &sandbox,
        env!("CARGO_BIN_EXE_nbdiff"),
        &dir,
        &["a.ipynb", "b.ipynb"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty() && !output.stderr.is_empty());
    // `os.path.exists(987)` is false for a file descriptor that is not open.
    let dir = config_dir(&sandbox, "base", r#"{"NbMerge": {"base": 987}}"#);
    let output = run_in(
        &sandbox,
        env!("CARGO_BIN_EXE_nbmerge"),
        &dir,
        &["a.ipynb", "b.ipynb"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "[E nbmergeapp:35] Cannot find file '987'\n"
    );
}

#[test]
fn odd_config_files_follow_traitlets() {
    let sandbox = Sandbox::new("config-shapes");
    let ok = |config: &str| {
        let dir = config_dir(&sandbox, "shape", config);
        let output = run_in(
            &sandbox,
            env!("CARGO_BIN_EXE_nbdiff"),
            &dir,
            &["--no-color", "a.ipynb", "b.ipynb"],
        );
        (
            output.status.code(),
            text(&output.stdout).contains(CHANGED_A),
            output.stderr.is_empty(),
        )
    };
    let plain = ok("{}");
    assert_eq!(plain.0, Some(0));
    assert!(plain.1);
    // An empty string is an empty configuration; a list of pairs is a configuration.
    assert_eq!(ok("\"\""), plain);
    assert_eq!(ok(r#"[["NbDiff", {"outputs": false}]]"#), plain);
    // `dict(...)` with a wrong length is a ValueError, which is ignored.
    assert_eq!(ok("\"ab\""), plain);
    // TypeErrors and AttributeErrors are not.
    for bad in ["[1]", "\"version\"", "5", "null", r#"{"NbDiff": 5}"#] {
        let result = ok(bad);
        assert_eq!(result.0, Some(1), "{bad}");
        assert!(!result.2, "{bad}");
    }
}

#[test]
fn config_listing_is_sorted_and_complete() {
    let sandbox = Sandbox::new("config-listing");
    let dir = sandbox.dir("empty");
    let output = run_in(&sandbox, env!("CARGO_BIN_EXE_nbdiff"), &dir, &["--config"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "NbDiff:\n  Ignore: {}\n  attachments: <unset, resolves to true>\n  color_words: false\n  \
         details: <unset, resolves to true>\n  id: <unset, resolves to true>\n  \
         metadata: <unset, resolves to true>\n  outputs: <unset, resolves to true>\n  \
         sources: <unset, resolves to true>\n"
    );
    let all = run_in(&sandbox, env!("CARGO_BIN_EXE_nbdime"), &dir, &["--config"]);
    let all = text(&all.stderr);
    for class in [
        "NbDiff:",
        "NbDiffWeb:",
        "NbMerge:",
        "NbMergeWeb:",
        "NbShow:",
        "Server:",
        "Extension:",
        "NbDiffDriver:",
        "NbDiffTool:",
        "NbMergeDriver:",
        "NbMergeTool:",
    ] {
        assert!(all.contains(&format!("\n{class}\n")), "{class}");
    }
    // Mixed ignorables are an `ArgumentError` raised by the parser: a usage error.
    let dir = config_dir(
        &sandbox,
        "mixed",
        r#"{"NbDiff": {"sources": true, "outputs": false}}"#,
    );
    let output = run_in(&sandbox, env!("CARGO_BIN_EXE_nbdiff"), &dir, &["--config"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("Arguments must either all be negative or all positive"));
}

fn fake_jupyter(sandbox: &Sandbox) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let bin = sandbox.dir("bin");
    let log = sandbox.root.join("jupyter.log");
    let script = bin.join("jupyter");
    fs::write(
        &script,
        format!("#!/bin/sh\necho \"$@\" >> {}\nexit 3\n", log.display()),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    (bin, log)
}

#[test]
fn nbdime_extensions_calls_jupyter_and_ignores_its_status() {
    let sandbox = Sandbox::new("extensions");
    let dir = sandbox.dir("cwd");
    let (bin, log) = fake_jupyter(&sandbox);
    let run = |args: &[&str]| {
        sandbox
            .command(env!("CARGO_BIN_EXE_nbdime"), &dir)
            .env("PATH", &bin)
            .args(args)
            .output()
            .unwrap()
    };
    let output = run(&["extensions", "--enable", "--sys-prefix"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "serverextension enable --py nbdime --sys-prefix\nnbextension install --py nbdime --sys-prefix\n\
         nbextension enable --py nbdime --sys-prefix\nlabextension install nbdime-jupyterlab --sys-prefix\n"
    );
    let output = run(&["extensions", "--x"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "Unknown command: use either \"--enable\" or \"--disable\" with \"nbdime extensions\", not '--x'\n"
    );
    let output = run(&["extensions"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(!output.stderr.is_empty());
}
