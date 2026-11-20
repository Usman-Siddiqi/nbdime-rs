//! GitPython's revision parser (`git.repo.fun.rev_parse`) as `repo.commit(rev)` uses it.
//!
//! The grammar is GitPython's, not git's: it decides which strings are revisions (`BadName`,
//! "not a revision"), which are errors of another kind (a `ValueError`, `IndexError` or
//! `KeyError` that nbdime does not catch) and where a commit is found. Looking objects up is
//! left to `git`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Result};

use crate::gitfiles::Repo;
use crate::osstr;

/// Why a revision did not resolve.
#[derive(Debug)]
enum Stop {
    /// `gitdb.exc.BadName`: nbdime treats the string as something else (a path).
    BadName,
    /// Any other exception, which nbdime lets escape.
    Other(String),
}

type Parse<T> = Result<T, Stop>;

fn other<T>(message: impl Into<String>) -> Parse<T> {
    Err(Stop::Other(message.into()))
}

struct Obj {
    id: String,
    kind: String,
}

thread_local! {
    /// `git rev-parse --verify` answers, per working directory and revision.
    static VERIFIED: RefCell<HashMap<(PathBuf, String), Option<String>>> = RefCell::new(HashMap::new());
}

/// The object id `git rev-parse --verify --quiet <rev>` names, if there is one.
fn verify(repo: &Repo, rev: &str) -> Option<String> {
    if rev.starts_with('-') || rev.is_empty() {
        return None;
    }
    let key = (repo.work_dir().to_path_buf(), rev.to_owned());
    if let Some(answer) = VERIFIED.with(|cache| cache.borrow().get(&key).cloned()) {
        return answer;
    }
    let output = repo
        .git()
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(osstr::to_os(rev))
        .stderr(Stdio::null())
        .output()
        .ok();
    let answer = output
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
    VERIFIED.with(|cache| cache.borrow_mut().insert(key, answer.clone()));
    answer
}

fn object(repo: &Repo, id: &str) -> Parse<Obj> {
    let output = repo
        .git()
        .args(["cat-file", "-t", id])
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success());
    match output {
        Some(output) => Ok(Obj {
            id: id.to_owned(),
            kind: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        }),
        None => other(format!("BadObject: {id}")),
    }
}

fn deref_tag(repo: &Repo, obj: Obj) -> Parse<Obj> {
    if obj.kind != "tag" {
        return Ok(obj);
    }
    match verify(repo, &format!("{}^{{}}", obj.id)) {
        Some(id) => object(repo, &id),
        None => other(format!("BadObject: {}", obj.id)),
    }
}

fn to_commit(repo: &Repo, obj: Obj) -> Parse<Obj> {
    let obj = deref_tag(repo, obj)?;
    if obj.kind != "commit" {
        return other(format!("Cannot convert object {} to type commit", obj.id));
    }
    Ok(obj)
}

fn is_short_hex(name: &str) -> bool {
    (4..=40).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The full name of the reference `name` is, by git's own lookup rules (`name`, `refs/name`,
/// `refs/tags/name`, `refs/heads/name`, `refs/remotes/name`, `refs/remotes/name/HEAD`).
fn reference_of(repo: &Repo, name: &str) -> Option<String> {
    if name.starts_with('-') || name.is_empty() {
        return None;
    }
    let output = repo
        .git()
        .args(["rev-parse", "--symbolic-full-name"])
        .arg(osstr::to_os(name))
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let full = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!full.is_empty()).then_some(full)
}

fn name_to_object(repo: &Repo, name: &str) -> Parse<Obj> {
    match verify(repo, name) {
        Some(id) => object(repo, &id),
        None if name.len() == 40 && is_short_hex(name) => object(repo, name),
        None => Err(Stop::BadName),
    }
}

/// `name_to_object(..., return_ref=True)`: the full name of the reference.
fn name_to_ref(repo: &Repo, name: &str) -> Parse<String> {
    match reference_of(repo, name) {
        Some(full) => Ok(full),
        None => other(format!("BadObject: Couldn't find reference named {name:?}")),
    }
}

fn ref_commit(repo: &Repo, reference: &str) -> Parse<Obj> {
    match verify(repo, reference) {
        Some(id) => to_commit(repo, object(repo, &id)?),
        None => other(format!("Reference at {reference:?} does not exist")),
    }
}

/// `repo.head.ref`, or `HEAD` itself when it is detached.
fn current_reflog_ref(repo: &Repo) -> String {
    let output = repo
        .git()
        .args(["symbolic-ref", "-q", "HEAD"])
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success());
    match output {
        Some(output) => String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        None => "HEAD".to_owned(),
    }
}

struct LogEntry {
    new: String,
    time: i64,
    message: String,
}

/// The reflog of `reference`, oldest entry first.
fn reflog(repo: &Repo, reference: &str) -> Parse<Vec<LogEntry>> {
    let candidates: Vec<PathBuf> = if reference == "HEAD" {
        vec![repo.git_dir.join("logs").join("HEAD")]
    } else {
        vec![
            repo.git_dir.join("logs").join(reference),
            repo.common_dir.join("logs").join(reference),
            // The checked-out branch of a linked worktree logs through `HEAD`.
            if reference == current_reflog_ref(repo) {
                repo.git_dir.join("logs").join("HEAD")
            } else {
                PathBuf::new()
            },
        ]
    };
    let Some(file) = candidates
        .iter()
        .find(|path| !path.as_os_str().is_empty() && Path::new(path).is_file())
    else {
        return other(format!("FileNotFoundError: reflog of {reference}"));
    };
    let Ok(bytes) = fs::read(file) else {
        return other(format!("cannot read the reflog of {reference}"));
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut entries = Vec::new();
    for line in text.lines().filter(|line| !line.is_empty()) {
        let (head, message) = line.split_once('\t').unwrap_or((line, ""));
        let fields: Vec<&str> = head.split_whitespace().collect();
        if fields.len() < 4 {
            return other(format!("malformed reflog line {line:?}"));
        }
        let time = fields[fields.len() - 2].parse().unwrap_or(0);
        entries.push(LogEntry {
            new: fields[1].to_owned(),
            time,
            message: message.to_owned(),
        });
    }
    Ok(entries)
}

/// Python's `int(text)`.
fn python_int(text: &str) -> Option<i128> {
    let text = text.trim();
    let (negative, digits) = match text.as_bytes().first()? {
        b'-' => (true, &text[1..]),
        b'+' => (false, &text[1..]),
        _ => (false, text),
    };
    let well_formed = !digits.is_empty()
        && !digits.starts_with('_')
        && !digits.ends_with('_')
        && !digits.contains("__")
        && digits.chars().all(|c| c.is_ascii_digit() || c == '_');
    if !well_formed {
        return None;
    }
    let value: i128 = digits.replace('_', "").parse().ok()?;
    Some(if negative { -value } else { value })
}

fn previous_checked_out_branch(repo: &Repo, nth: i128) -> Parse<Obj> {
    if nth <= 0 {
        return other(format!("Invalid previous checkout selector: -{nth}"));
    }
    let mut seen = 0;
    for entry in reflog(repo, "HEAD")?.iter().rev() {
        let Some(moved) = entry.message.strip_prefix("checkout: moving from ") else {
            continue;
        };
        seen += 1;
        if seen == nth {
            let previous = moved.split(" to ").next().unwrap_or(moved);
            return name_to_object(repo, previous);
        }
    }
    other(format!("Invalid previous checkout selector: -{nth}"))
}

fn tracking_branch_object(repo: &Repo, reference: Option<&str>) -> Parse<Obj> {
    let head = match reference {
        None => {
            let current = current_reflog_ref(repo);
            if current == "HEAD" {
                return Err(Stop::BadName);
            }
            current
        }
        Some(reference) if reference.starts_with("refs/heads/") => reference.to_owned(),
        Some(_) => return Err(Stop::BadName),
    };
    match verify(repo, &format!("{head}@{{upstream}}")) {
        Some(id) => to_commit(repo, object(repo, &id)?),
        None => Err(Stop::BadName),
    }
}

/// The only date format supported is git's own, `<timestamp> <offset>`; GitPython's parser takes
/// RFC 2822 and ISO 8601 as well, which are not reproduced.
fn find_reflog_entry_by_date(repo: &Repo, reference: &str, spec: &str) -> Parse<String> {
    let unsupported = || other("Support for additional @{...} modes not implemented");
    if spec.matches(' ').count() != 1 || spec.contains(':') {
        return unsupported();
    }
    let (timestamp, offset) = spec.split_once(' ').unwrap_or_default();
    let offset_ok = offset.len() == 5
        && matches!(offset.as_bytes()[0], b'+' | b'-')
        && offset[1..].bytes().all(|b| b.is_ascii_digit());
    let Some(timestamp) = python_int(timestamp.trim_start_matches('@')).filter(|_| offset_ok)
    else {
        return unsupported();
    };
    let log = reflog(repo, reference)?;
    if log.is_empty() {
        return other(format!("Invalid revlog date: {spec}"));
    }
    for entry in log.iter().rev() {
        if i128::from(entry.time) <= timestamp {
            return Ok(entry.new.clone());
        }
    }
    Ok(log[0].new.clone())
}

fn apply_reflog(repo: &Repo, reference: Option<&str>, content: &str) -> Parse<Obj> {
    let content = content.strip_prefix('+').unwrap_or(content);
    if let Some(selector) = content.strip_prefix('-') {
        if reference.is_some() {
            return other("Previous checkout selectors do not take an explicit ref");
        }
        if content == "-0" {
            return other("Negative zero is invalid in reflog selector");
        }
        return match python_int(selector) {
            Some(nth) => previous_checked_out_branch(repo, nth),
            None => other(format!("Invalid previous checkout selector: {content}")),
        };
    }
    if ["u", "upstream", "push"].contains(&content.to_lowercase().as_str()) {
        return tracking_branch_object(repo, reference);
    }
    let reference = match reference {
        Some(reference) => reference.to_owned(),
        None => current_reflog_ref(repo),
    };
    let hexsha = match python_int(content) {
        None => find_reflog_entry_by_date(repo, &reference, content)?,
        Some(n) if n >= 100_000_000 => {
            find_reflog_entry_by_date(repo, &reference, &format!("{n} +0000"))?
        }
        Some(0) => return ref_commit(repo, &reference),
        Some(n) => {
            let log = reflog(repo, &reference)?;
            let index = usize::try_from(n)
                .ok()
                .filter(|n| *n < log.len())
                .map(|n| log.len() - 1 - n);
            match index {
                Some(index) => log[index].new.clone(),
                None => return other(format!("Invalid revlog index: {n}")),
            }
        }
    };
    object(repo, &hexsha)
}

/// The index of the brace that closes the one at `open`.
fn find_closing_brace(rev: &str, open: usize) -> Parse<usize> {
    let (mut depth, mut escaped) = (1, false);
    for (i, b) in rev.bytes().enumerate().skip(open + 1) {
        match b {
            _ if escaped => escaped = false,
            b'\\' => escaped = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            _ => {}
        }
    }
    other(format!("Missing closing brace to define type in {rev}"))
}

fn find_commit_by_message(repo: &Repo, rev: Option<Obj>, pattern: &str) -> Parse<Obj> {
    if pattern.is_empty() {
        return other("Revision search requires a pattern");
    }
    if pattern.starts_with('!') && !pattern.starts_with("!-") && !pattern.starts_with("!!") {
        return other("Need one character after /!, typically -");
    }
    let search = match rev {
        None => format!(":/{pattern}"),
        Some(rev) => format!("{}^{{/{pattern}}}", to_commit(repo, rev)?.id),
    };
    match verify(repo, &search) {
        Some(id) => object(repo, &id),
        None => Err(Stop::BadName),
    }
}

fn index_lookup(repo: &Repo, spec: &str) -> Parse<Obj> {
    if spec.is_empty() {
        return other("':' must be followed by a path");
    }
    let bytes = spec.as_bytes();
    let (stage, path) = if bytes.len() >= 2 && bytes[1] == b':' && b"0123".contains(&bytes[0]) {
        (bytes[0] - b'0', &spec[2..])
    } else {
        (0, spec)
    };
    match verify(repo, &format!(":{stage}:{path}")) {
        Some(id) => object(repo, &id),
        None => Err(Stop::BadName),
    }
}

fn tree_of(repo: &Repo, obj: Obj) -> Parse<Obj> {
    if obj.kind == "tree" {
        return Ok(obj);
    }
    let commit = to_commit(repo, obj)?;
    match verify(repo, &format!("{}^{{tree}}", commit.id)) {
        Some(id) => object(repo, &id),
        None => other(format!("{} has no tree", commit.id)),
    }
}

fn tree_lookup(repo: &Repo, obj: Obj, path: &str) -> Parse<Obj> {
    let tree = tree_of(repo, obj)?;
    if path.is_empty() {
        return Ok(tree);
    }
    match verify(repo, &format!("{}:{path}", tree.id)) {
        Some(id) => object(repo, &id),
        None => other(format!("KeyError: {path:?}")),
    }
}

fn peel(repo: &Repo, obj: Obj, output_type: &str, rev: &str) -> Parse<Obj> {
    if let Some(pattern) = output_type.strip_prefix('/') {
        return find_commit_by_message(repo, Some(obj), pattern);
    }
    match output_type {
        "" => deref_tag(repo, obj),
        "object" => Ok(obj),
        "commit" => to_commit(repo, obj),
        "tree" => tree_of(repo, obj),
        "blob" | "tag" => {
            let obj = if output_type == "blob" {
                deref_tag(repo, obj)?
            } else {
                obj
            };
            if obj.kind == output_type {
                Ok(obj)
            } else {
                other(format!(
                    "Could not accommodate requested object type {output_type:?}, got {}",
                    obj.kind
                ))
            }
        }
        _ => other(format!("Invalid output type: {output_type} ( in {rev} )")),
    }
}

/// The position of the first character that starts something other than a plain name.
fn first_rev_token(rev: &str) -> Option<usize> {
    let bytes = rev.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'^' | b'~' | b':' => return Some(i),
            b'@' => {
                let next = bytes.get(i + 1);
                if i == 0 && matches!(next, None | Some(b'^' | b'~' | b':' | b'{')) {
                    return Some(i);
                }
                if next == Some(&b'{') {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn rev_parse(repo: &Repo, rev: &str) -> Parse<Obj> {
    if let Some(pattern) = rev.strip_prefix(":/") {
        return find_commit_by_message(repo, None, pattern);
    }
    if let Some(path) = rev.strip_prefix(':') {
        return index_lookup(repo, path);
    }
    let bytes = rev.as_bytes();
    let Some(first) = first_rev_token(rev) else {
        return name_to_object(repo, rev);
    };
    let (mut obj, mut reference, mut start);
    if first == 0 {
        if bytes[0] != b'@' {
            return other(format!(
                "Revision specifier must start with an object name: {rev}"
            ));
        }
        obj = ref_commit(repo, &current_reflog_ref(repo))?;
        reference = None;
        start = if rev.starts_with("@{") { 0 } else { 1 };
    } else {
        if bytes[first] == b'@' {
            let full = name_to_ref(repo, &rev[..first])?;
            obj = ref_commit(repo, &full)?;
            reference = Some(full);
        } else {
            obj = name_to_object(repo, &rev[..first])?;
            reference = None;
        }
        start = first;
    }

    while start < bytes.len() {
        let token = bytes[start];
        if token == b'@' {
            if bytes.get(start + 1) != Some(&b'{') {
                return other(format!("Invalid @ token in revision specifier: {rev}"));
            }
            let end = find_closing_brace(rev, start + 1)?;
            let anchor = if start == first {
                reference.take()
            } else {
                None
            };
            obj = apply_reflog(repo, anchor.as_deref(), &rev[start + 2..end])?;
            reference = None;
            start = end + 1;
            continue;
        }
        if token == b':' {
            return tree_lookup(repo, obj, &rev[start + 1..]);
        }
        start += 1;
        if token == b'^' && bytes.get(start) == Some(&b'{') {
            let end = find_closing_brace(rev, start)?;
            obj = peel(repo, obj, &rev[start + 1..end], rev)?;
            reference = None;
            start = end + 1;
            continue;
        }
        let digits = bytes[start..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        let num: u64 = if digits == 0 {
            1
        } else {
            rev[start..start + digits].parse().unwrap_or(u64::MAX)
        };
        start += digits;
        let parent_spec = match token {
            b'~' => format!("{}~{num}", to_commit(repo, obj)?.id),
            b'^' => {
                let commit = to_commit(repo, obj)?;
                if num == 0 {
                    obj = commit;
                    continue;
                }
                format!("{}^{num}", commit.id)
            }
            _ => return other(format!("Invalid token: {:?}", char::from(token))),
        };
        obj = match verify(repo, &parent_spec) {
            Some(id) => object(repo, &id)?,
            None => return Err(Stop::BadName),
        };
    }
    Ok(obj)
}

/// `repo.commit(rev)`: the commit `rev` names, `Ok(None)` where GitPython raises `BadName` (the
/// caller then tries the string as a path), and an error for every other exception.
pub fn resolve_commit(repo: &Repo, rev: &str) -> Result<Option<String>> {
    match rev_parse(repo, rev).and_then(|obj| to_commit(repo, obj)) {
        Ok(commit) => Ok(Some(commit.id)),
        Err(Stop::BadName) => Ok(None),
        Err(Stop::Other(message)) => bail!("{}: {message}", osstr::show(rev)),
    }
}

/// `object^{tree}`.
pub fn tree_id(repo: &Repo, commit: &str) -> Option<String> {
    verify(repo, &format!("{commit}^{{tree}}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitfiles::get_repo;
    use crate::testutil::TempDir;

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .current_dir(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// Two commits on `main` (`a.ipynb`), a lightweight tag `light` and an annotated `v1`.
    fn repository(name: &str) -> (TempDir, Repo) {
        let dir = TempDir::new(name);
        let root = fs::canonicalize(&dir.0).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        fs::write(root.join("a.ipynb"), "one").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "one fix"]);
        git(&root, &["tag", "-a", "-m", "annotated", "v1"]);
        fs::write(root.join("a.ipynb"), "two").unwrap();
        git(&root, &["commit", "-q", "-am", "two"]);
        git(&root, &["tag", "light"]);
        git(&root, &["checkout", "-q", "-b", "other"]);
        git(&root, &["checkout", "-q", "main"]);
        let repo = get_repo(&root).unwrap();
        (dir, repo)
    }

    fn found(repo: &Repo, rev: &str) -> bool {
        resolve_commit(repo, rev)
            .unwrap_or_else(|e| panic!("{rev}: {e:#}"))
            .is_some()
    }

    #[test]
    fn revisions_that_resolve() {
        let (_dir, repo) = repository("rp-found");
        let head = resolve_commit(&repo, "HEAD").unwrap().unwrap();
        assert_eq!(head.len(), 40);
        for rev in [
            "main",
            "HEAD~1",
            "v1",
            "v1^{commit}",
            "HEAD^{commit}",
            "HEAD^{}",
            "HEAD^{object}",
            "v1^{tag}",
            ":/one fix",
            "@",
            "HEAD^0",
            "@{0}",
            "HEAD@{0}",
            "HEAD@{1}",
            "main@{1}",
            "@{-1}",
            "HEAD^{/one}",
            "light",
            "HEAD~1^{commit}",
        ] {
            assert!(found(&repo, rev), "{rev}");
        }
        assert_eq!(resolve_commit(&repo, "light").unwrap(), Some(head.clone()));
        assert_eq!(resolve_commit(&repo, "HEAD^{commit}").unwrap(), Some(head));
    }

    #[test]
    fn revisions_that_are_not_revisions() {
        let (_dir, repo) = repository("rp-badname");
        for rev in [
            "nonexistent",
            "v1..HEAD",
            "nope~1..x",
            ":/no such commit",
            "HEAD~9",
            ":nope.ipynb",
            ":2:a.ipynb",
            "HEAD^9",
            "HEAD@{u}x",
            "other@{upstream}x",
        ] {
            assert_eq!(resolve_commit(&repo, rev).unwrap(), None, "{rev}");
        }
    }

    #[test]
    fn revisions_gitpython_cannot_parse_or_use_are_errors() {
        let (_dir, repo) = repository("rp-errors");
        for rev in [
            "HEAD^{foo}",
            "HEAD^{blob}",
            "light^{tag}",
            "HEAD^{tree}~1",
            "HEAD@{9}",
            "@{-5}",
            "HEAD@{-1}",
            "main@{-1}",
            ":/",
            ":/!x",
            "HEAD:nope.ipynb",
            "HEAD~1:sub/nope",
            "deadbee@{1}",
            "unknown@{1}",
            "HEAD@{yesterday}",
            "HEAD^{tree}",
            "HEAD:a.ipynb",
            ":a.ipynb",
            "HEAD~1..HEAD",
            "HEAD~1...HEAD",
            "HEAD^!",
            "^HEAD",
            "HEAD^{tree",
            "HEAD~1@x",
            "nope@{upstream}x",
            "HEAD:",
        ] {
            assert!(resolve_commit(&repo, rev).is_err(), "{rev}");
        }
    }

    #[test]
    fn an_upstream_without_configuration_is_not_a_revision() {
        let (_dir, repo) = repository("rp-upstream");
        for rev in ["@{u}", "main@{u}", "HEAD@{u}"] {
            // `HEAD` is not a branch to GitPython, and `main` has no upstream: `BadName`.
            assert_eq!(resolve_commit(&repo, rev).unwrap(), None, "{rev}");
        }
    }

    #[test]
    fn answers_are_cached() {
        let (_dir, repo) = repository("rp-cache");
        let first = resolve_commit(&repo, "HEAD~1").unwrap();
        let key = (repo.work_dir().to_path_buf(), "HEAD".to_owned());
        assert!(VERIFIED.with(|cache| cache.borrow().contains_key(&key)));
        assert_eq!(resolve_commit(&repo, "HEAD~1").unwrap(), first);
    }

    #[test]
    fn python_integers() {
        assert_eq!(python_int(" 12 "), Some(12));
        assert_eq!(python_int("+1_0"), Some(10));
        assert_eq!(python_int("-3"), Some(-3));
        for bad in ["", "1__0", "_1", "1_", "x", "1.5", "-"] {
            assert_eq!(python_int(bad), None, "{bad:?}");
        }
    }
}
