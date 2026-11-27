//! The git configuration the `config` subcommands change: `git config` calls and the attributes
//! files that name the drivers (`nbdime.utils.locate_gitattributes` and `ensure_dir_exists`, and
//! the parts of `enable`/`disable` the git commands share).

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};
use nbdime_core::py::is_space;

use crate::osstr;
use crate::pathexpand::{expanduser, expandvars, Env, ProcessEnv};

/// The `git config` command line for a scope (`--global`, `--system`, or the repository).
pub fn config_command(scope: Option<&str>) -> Vec<String> {
    let mut command = vec!["git".to_owned(), "config".to_owned()];
    if let Some(scope) = scope {
        command.push(format!("--{scope}"));
    }
    command
}

fn run(command: &[String]) -> Command {
    let mut run = Command::new(&command[0]);
    run.args(command[1..].iter().map(|a| osstr::to_os(a)));
    run
}

/// `subprocess.check_call`: the command runs with this process's standard streams. `Ok(false)`
/// is a non-zero status (`CalledProcessError`); failing to start the command is an error.
pub fn check_call(command: &[String]) -> Result<bool> {
    let status = run(command)
        .status()
        .with_context(|| format!("running {}", command[0]))?;
    Ok(status.success())
}

/// `check_call` for a command that must succeed.
pub fn must_call(command: &[String]) -> Result<()> {
    if check_call(command)? {
        Ok(())
    } else {
        bail!("Command {command:?} returned a non-zero exit status")
    }
}

/// `subprocess.check_output` decoded with `utf8`/`replace`: `Ok(None)` is a non-zero status.
fn check_output(mut command: Command, name: &str) -> Result<Option<String>> {
    let output = command
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("running {name}"))?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned()))
}

/// `git config <scope> <key>`'s output, stripped; `None` if the command failed.
pub fn config_output(scope: Option<&str>, key: &str) -> Result<Option<String>> {
    let mut command = config_command(scope);
    command.push(key.to_owned());
    Ok(check_output(run(&command), "git")?.map(|text| text.trim_matches(is_space).to_owned()))
}

/// `os.path.dirname`.
fn dirname(path: &str) -> &str {
    // `ntpath` splits at either slash.
    const SEPARATORS: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };
    match path.rfind(SEPARATORS) {
        None => "",
        Some(i) => {
            let head = &path[..=i];
            if head.chars().all(|c| SEPARATORS.contains(&c)) {
                head
            } else {
                head.trim_end_matches(SEPARATORS)
            }
        }
    }
}

/// `ensure_dir_exists`: creates the directory and its parents unless it is there.
pub fn ensure_dir_exists(path: &str) -> Result<()> {
    if osstr::to_path(path).exists() {
        return Ok(());
    }
    if path.is_empty() {
        // `os.makedirs('')`.
        bail!("[Errno 2] No such file or directory: ''");
    }
    match fs::create_dir_all(osstr::to_path(path)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error).with_context(|| format!("creating {}", osstr::show(path))),
    }
}

/// `locate_gitattributes(scope)`: the attributes file for a scope, or `None` if the current
/// directory has no `.git` and no scope was asked for.
pub fn locate_gitattributes(scope: Option<&str>) -> Result<Option<String>> {
    match scope {
        Some("global") => {
            let mut command = config_command(Some("global"));
            command.push("core.attributesfile".to_owned());
            let configured = check_output(run(&command), "git")?;
            let env = ProcessEnv;
            Ok(Some(match configured {
                Some(text) => expanduser(text.trim_matches(is_space), &env),
                None => match env.get("XDG_CONFIG_HOME") {
                    Some(home) if !home.is_empty() => {
                        expandvars("$XDG_CONFIG_HOME/git/attributes", &env)
                    }
                    _ => expanduser("~/.config/git/attributes", &env),
                },
            }))
        }
        Some("system") => {
            let mut command = Command::new("git");
            command
                .args(["config", "--system", "-e"])
                .env("GIT_EDITOR", "echo");
            match check_output(command, "git")? {
                Some(text) => {
                    let gitconfig = text.trim_matches(is_space);
                    let dir = dirname(gitconfig);
                    Ok(Some(if dir.is_empty() {
                        "gitattributes".to_owned()
                    } else {
                        format!("{}/gitattributes", dir.trim_end_matches('/'))
                    }))
                }
                None => {
                    if !Path::new("/etc").exists() {
                        bail!("Could not find system gitattributes location!");
                    }
                    // `os.path.join(['etc', 'gitattributes'])` raises (F-033).
                    Err(anyhow!(
                        "join() argument must be str, bytes, or os.PathLike object, not 'list'"
                    ))
                }
            }
        }
        Some(other) => bail!("unknown git scope {other:?}"),
        None => {
            let cwd = std::env::current_dir().context("reading the current directory")?;
            if !cwd.join(".git").exists() {
                return Ok(None);
            }
            Ok(Some(osstr::from_os(cwd.join(".gitattributes").as_os_str())))
        }
    }
}

/// What `enable` does after it has set the git configuration: puts `line` into the attributes
/// file of the scope unless the file already has `marker`.
pub fn add_attributes(scope: Option<&str>, marker: &str, line: &str) -> Result<()> {
    let Some(gitattributes) = locate_gitattributes(scope)? else {
        osstr::eprint("No .git directory in ., skipping git attributes\n");
        return Ok(());
    };
    let path = osstr::to_path(&gitattributes);
    if path.exists() {
        let bytes =
            fs::read(&path).with_context(|| format!("reading {}", osstr::show(&gitattributes)))?;
        let text = String::from_utf8(bytes)
            .map_err(|e| anyhow!("{}: {e}", osstr::show(&gitattributes)))?;
        if text.contains(marker) {
            return Ok(());
        }
    } else {
        ensure_dir_exists(dirname(&gitattributes))?;
    }
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .with_context(|| format!("opening {}", osstr::show(&gitattributes)))?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn dirname_follows_os_path() {
        assert_eq!(dirname("/a/b/c"), "/a/b");
        assert_eq!(dirname("c"), "");
        assert_eq!(dirname("/c"), "/");
        assert_eq!(dirname("/a//b"), "/a");
        assert_eq!(dirname("a/b/"), "a/b");
    }

    #[test]
    fn config_commands_name_the_scope() {
        assert_eq!(config_command(None), ["git", "config"]);
        assert_eq!(
            config_command(Some("global")),
            ["git", "config", "--global"]
        );
    }

    #[test]
    fn directories_are_created_when_missing() {
        let dir = TempDir::new("gitattributes-dirs");
        let nested = dir.path("a/b/c");
        ensure_dir_exists(&nested).unwrap();
        ensure_dir_exists(&nested).unwrap();
        assert!(Path::new(&nested).is_dir());
        assert!(ensure_dir_exists("").is_err());
    }
}
