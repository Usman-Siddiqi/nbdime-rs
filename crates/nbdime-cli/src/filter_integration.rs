//! Git clean filters applied to the working-tree side of a diff
//! (`nbdime.vcs.git.filter_integration`).

use std::fs;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::osstr;
use crate::utils::{Source, EXPLICIT_MISSING_FILE};

/// The `filter` attribute git has for `path`, if it names a filter (`interrogate_filter`).
pub fn interrogate_filter(path: &str) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["check-attr", "-z", "filter"])
        .arg(osstr::to_os(path))
        .stderr(Stdio::inherit())
        .output()
        .context("running git")?;
    if !output.status.success() {
        return Ok(None);
    }
    let parts: Vec<_> = output
        .stdout
        .split(|b| *b == 0)
        .take(3)
        .map(String::from_utf8_lossy)
        .collect();
    // Older git lacks `-z` and its output does not split into three parts.
    let [path_list, attr, info] = parts.as_slice() else {
        return Ok(None);
    };
    if attr != "filter" || path_list != path {
        bail!(
            "Unexpected output from git check-attr. Expected to start with \"{path}\\0filter\", \
             started with \"{path_list}\\0{attr}\""
        );
    }
    Ok(match info.as_ref() {
        "" | "unspecified" | "set" | "unset" => None,
        other => Some(other.to_owned()),
    })
}

/// The clean filter command that `filter.<attr>.clean` names (`get_clean_filter_cmd`).
pub fn get_clean_filter_cmd(filter_attr: &str) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["config", "--get", "-z"])
        .arg(format!("filter.{filter_attr}.clean"))
        .stderr(Stdio::inherit())
        .output()
        .context("running git")?;
    if !output.status.success() {
        return Ok(None);
    }
    let command = output.stdout.split(|b| *b == 0).next().unwrap_or_default();
    let command = String::from_utf8_lossy(command).into_owned();
    Ok((!command.is_empty()).then_some(command))
}

/// The content of the file `path` run through the clean filter configured for `git_path`, or
/// `None` if no filter applies.
pub(crate) fn filtered(git_path: &str, path: &str) -> Result<Option<Source>> {
    if path == EXPLICIT_MISSING_FILE {
        return Ok(None);
    }
    let Some(attr) = interrogate_filter(git_path)? else {
        return Ok(None);
    };
    let Some(command) = get_clean_filter_cmd(&attr)? else {
        return Ok(None);
    };
    let input = fs::File::open(osstr::to_path(path))
        .with_context(|| format!("opening {}", osstr::show(path)))?;
    if input.metadata()?.is_dir() {
        bail!("opening {}: Is a directory", osstr::show(path));
    }
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
        name: path.to_owned(),
        text: String::from_utf8_lossy(&output.stdout).into_owned(),
    }))
}

/// `apply_possible_filter(git_path, path)`: `path` itself if no filter is configured for
/// `git_path`, else the filtered content, which keeps the name `path`. `path` defaults to
/// `git_path`.
pub fn apply_possible_filter(git_path: &str, path: Option<&str>) -> Result<Source> {
    let path = path.unwrap_or(git_path);
    Ok(filtered(git_path, path)?.unwrap_or_else(|| Source::Path(path.to_owned())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("HOME", dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// Runs `body` in a fresh repository whose current directory it is. Tests of this module
    /// change the directory, so they share one test.
    #[test]
    fn filters_follow_the_attributes_and_the_configuration() {
        let dir = crate::testutil::TempDir::new("filter-integration");
        let repo = fs::canonicalize(&dir.0).unwrap();
        git(&repo, &["init", "-q"]);
        fs::write(repo.join("a.ipynb"), "one\ntwo\n").unwrap();
        fs::write(repo.join("b.txt"), "plain\n").unwrap();
        fs::write(
            repo.join(".git/info/attributes"),
            "*.ipynb filter=upper\n*.txt filter=missing\n",
        )
        .unwrap();
        git(&repo, &["config", "filter.upper.clean", "tr a-z A-Z"]);
        std::env::set_current_dir(&repo).unwrap();

        assert_eq!(
            interrogate_filter("a.ipynb").unwrap().as_deref(),
            Some("upper")
        );
        assert_eq!(interrogate_filter("other.py").unwrap(), None);
        assert_eq!(
            get_clean_filter_cmd("upper").unwrap().as_deref(),
            Some("tr a-z A-Z")
        );
        assert_eq!(get_clean_filter_cmd("missing").unwrap(), None);

        let source = apply_possible_filter("a.ipynb", None).unwrap();
        assert_eq!(
            source,
            Source::Stream {
                name: "a.ipynb".into(),
                text: "ONE\nTWO\n".into()
            }
        );
        // The filter of one path runs over the content of another file, which names the result.
        let remote = repo.join("b.txt").to_string_lossy().into_owned();
        let source = apply_possible_filter("a.ipynb", Some(&remote)).unwrap();
        assert_eq!(
            source,
            Source::Stream {
                name: remote.clone(),
                text: "PLAIN\n".into()
            }
        );
        // A filter that git does not know leaves the path alone, and so does the null file.
        assert_eq!(
            apply_possible_filter("b.txt", None).unwrap(),
            Source::Path("b.txt".into())
        );
        assert_eq!(
            apply_possible_filter("a.ipynb", Some(EXPLICIT_MISSING_FILE)).unwrap(),
            Source::Path(EXPLICIT_MISSING_FILE.into())
        );
        git(&repo, &["config", "filter.upper.clean", "exit 3"]);
        assert!(apply_possible_filter("a.ipynb", None).is_err());
    }
}
