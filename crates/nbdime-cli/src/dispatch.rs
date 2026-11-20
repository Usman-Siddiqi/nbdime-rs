//! The `nbdime` command, which dispatches to the other commands (`nbdime.__main__`).

use std::io::Write;
use std::process::Command;

use anyhow::Result;

use crate::args::config_listing;
use crate::{nbdiffapp, nbmergeapp, VERSION};

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
    eprintln!("{message}");
    Ok(1)
}

/// Runs `nbdime` with the arguments `args` (without the program name).
pub fn main(prog: &str, args: &[String], out: &mut dyn Write) -> Result<i32> {
    let Some((command, rest)) = args.split_first() else {
        return exit_with(&format!("Option missing.\n\n{}", help_message()));
    };
    match command.as_str() {
        "diff" => nbdiffapp::main(prog, rest, out),
        "merge" => nbmergeapp::main(prog, rest, out),
        "mergetool" => mergetool(rest),
        "--version" => exit_with(VERSION),
        "-h" | "--help" => exit_with(&help_message()),
        "--config" => {
            eprintln!("All available config options, and their current values:\n");
            for entrypoint in ["nbdiff", "nbmerge"] {
                eprintln!("{}", config_listing(entrypoint)?);
            }
            Ok(1)
        }
        "show" | "diff-web" | "merge-web" | "server" | "config-git" | "reg-extensions"
        | "extensions" => exit_with(&format!(
            "nbdime-rs does not provide the '{command}' command yet."
        )),
        other => exit_with(&format!(
            "Unrecognized command '{other}'\n\n{}.",
            help_message()
        )),
    }
}

fn mergetool(args: &[String]) -> Result<i32> {
    let mut command = Command::new("git");
    command.args(["mergetool", "--tool=nbdime"]);
    if args.is_empty() {
        command.arg("*.ipynb");
    } else {
        let paths: Vec<&String> = args.iter().filter(|a| a.ends_with(".ipynb")).collect();
        if paths.len() != args.len() {
            let skipped: Vec<&String> = args.iter().filter(|a| !a.ends_with(".ipynb")).collect();
            eprintln!("Skipping given filenames without .ipynb extension: {skipped:?}");
        }
        command.args(paths);
    }
    match command.status() {
        Ok(status) => Ok(status.code().unwrap_or(1)),
        Err(_) => exit_with("Cannot use \"nbdime mergetool\" alias as git is not preset on path"),
    }
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
        assert_eq!(main("nbdime", &[], &mut out).unwrap(), 1);
        let unknown = vec!["bogus".to_owned()];
        assert_eq!(main("nbdime", &unknown, &mut out).unwrap(), 1);
        assert!(out.is_empty());
    }
}
