//! The `nbdime` command, which dispatches to the other commands (`nbdime.__main__`).

use std::io::Write;
use std::process::{Command, ExitStatus};

use anyhow::{bail, Context as _, Result};

use crate::args::{config_listing, ListingError};
use crate::config::{Context, ENTRYPOINTS};
use crate::{
    diffdriver, difftool, mergedriver, mergetool, nbdiffapp, nbmergeapp, nbshowapp, osstr, VERSION,
};

const COMMANDS: [&str; 8] = [
    "show",
    "diff",
    "merge",
    "diff-web",
    "merge-web",
    "mergetool",
    "config-git",
    "extensions",
];

fn help_message() -> String {
    format!(
        "Usage: nbdime [OPTIONS]\n\n\
         OPTIONS: -h, --version, COMMANDS{{{}}}\n\n\
         Examples: nbdime --version\n          nbdime show -h\n          nbdime merge-web\n\n\
         \x20         nbdime config-git (--enable | --disable)\n\
         \x20         nbdime extensions (--enable | --disable)\n\n\
         Documentation: https://nbdime.readthedocs.io",
        COMMANDS.join(", ")
    )
}

/// `sys.exit(message)`: the message on stderr and status 1.
fn exit_with(message: &str) -> Result<i32> {
    osstr::eprint(&format!("{message}\n"));
    Ok(1)
}

/// Runs `nbdime` with the arguments `args` (without the program name).
pub fn main(prog: &str, args: &[String], out: &mut dyn Write) -> Result<i32> {
    run(&Context::process(), prog, args, out)
}

/// [`main`] in the process `ctx` describes.
pub fn run(ctx: &Context, prog: &str, args: &[String], out: &mut dyn Write) -> Result<i32> {
    let Some((command, rest)) = args.split_first() else {
        return exit_with(&format!("Option missing.\n\n{}", help_message()));
    };
    match command.as_str() {
        "show" => nbshowapp::run(ctx, prog, rest, out),
        "diff" => nbdiffapp::run(ctx, prog, rest, out),
        "merge" => nbmergeapp::run(ctx, prog, rest, out),
        "mergetool" => mergetool(rest),
        "extensions" => extensions(rest),
        "reg-extensions" => toggle_extensions(true, rest).map(|()| 0),
        "--version" => exit_with(VERSION),
        "-h" | "--help" => exit_with(&help_message()),
        "--config" => {
            osstr::eprint("All available config options, and their current values:\n\n");
            for entrypoint in ENTRYPOINTS {
                let listing = config_listing(ctx, entrypoint).map_err(|e| match e {
                    ListingError::Usage(message) => anyhow::anyhow!(message),
                    ListingError::Other(error) => error,
                })?;
                osstr::eprint(&format!("{listing}\n"));
            }
            Ok(1)
        }
        "config-git" => {
            let args: Vec<String> = std::iter::once("config".to_owned())
                .chain(rest.iter().cloned())
                .collect();
            // Short-circuit on the first non-zero status.
            for tool in [
                diffdriver::run,
                mergedriver::run,
                difftool::run,
                mergetool::run,
            ] {
                let status = tool(ctx, prog, &args, out)?;
                if status != 0 {
                    return Ok(status);
                }
            }
            Ok(0)
        }
        "diff-web" | "merge-web" | "server" => exit_with(&format!(
            "nbdime-rs does not provide the '{command}' command yet."
        )),
        other => exit_with(&format!(
            "Unrecognized command '{other}'\n\n{}.",
            help_message()
        )),
    }
}

/// The status `sys.exit(returncode)` gives: a process killed by signal `n` is `-n` for Python,
/// which the shell sees as `256 - n`.
fn exit_status(status: ExitStatus) -> i32 {
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
        return (-signal) & 0xFF;
    }
    status.code().unwrap_or(1)
}

fn mergetool(args: &[String]) -> Result<i32> {
    let mut command = Command::new("git");
    command.args(["mergetool", "--tool=nbdime"]);
    if args.is_empty() {
        command.arg("*.ipynb");
    } else {
        let paths: Vec<&String> = args.iter().filter(|a| a.ends_with(".ipynb")).collect();
        if paths.len() != args.len() {
            let skipped: Vec<String> = args
                .iter()
                .filter(|a| !a.ends_with(".ipynb"))
                .map(|a| osstr::repr(a))
                .collect();
            osstr::eprint(&format!(
                "Skipping given filenames without .ipynb extension: [{}]\n",
                skipped.join(", ")
            ));
        }
        command.args(paths.into_iter().map(|p| osstr::to_os(p)));
    }
    match command.status() {
        Ok(status) => Ok(exit_status(status)),
        Err(_) => exit_with("Cannot use \"nbdime mergetool\" alias as git is not preset on path"),
    }
}

/// `nbdime extensions (--enable | --disable)`.
fn extensions(args: &[String]) -> Result<i32> {
    let Some((subcommand, rest)) = args.split_first() else {
        bail!("pop from empty list");
    };
    match subcommand.as_str() {
        "--enable" => toggle_extensions(true, rest).map(|()| 0),
        "--disable" => toggle_extensions(false, rest).map(|()| 0),
        other => exit_with(&format!(
            "Unknown command: use either \"--enable\" or \"--disable\" with \"nbdime extensions\", not {}",
            osstr::repr(other)
        )),
    }
}

/// `toggle_extensions`: runs the `jupyter` commands and ignores how they end.
fn toggle_extensions(enable: bool, args: &[String]) -> Result<()> {
    let commands: [&str; 4] = if enable {
        [
            "serverextension enable --py nbdime",
            "nbextension install --py nbdime",
            "nbextension enable --py nbdime",
            "labextension install nbdime-jupyterlab",
        ]
    } else {
        [
            "serverextension disable --py nbdime",
            "nbextension disable --py nbdime",
            "nbextension uninstall --py nbdime",
            "labextension uninstall nbdime-jupyterlab",
        ]
    };
    for command in commands {
        Command::new("jupyter")
            .args(command.split(' '))
            .args(args.iter().map(|a| osstr::to_os(a)))
            .status()
            .context("running jupyter")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_lists_the_commands() {
        let help = help_message();
        assert!(help.starts_with("Usage: nbdime [OPTIONS]"));
        assert!(help.contains(
            "COMMANDS{show, diff, merge, diff-web, merge-web, mergetool, config-git, extensions}"
        ));
        assert!(help.contains("          nbdime config-git (--enable | --disable)"));
    }

    #[test]
    fn dispatch_without_arguments_fails() {
        let mut out = Vec::new();
        assert_eq!(
            run(&crate::testutil::isolated(), "nbdime", &[], &mut out).unwrap(),
            1
        );
        let unknown = vec!["bogus".to_owned()];
        assert_eq!(
            run(&crate::testutil::isolated(), "nbdime", &unknown, &mut out).unwrap(),
            1
        );
        assert!(out.is_empty());
    }
}
