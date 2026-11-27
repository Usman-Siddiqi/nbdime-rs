//! `git-nbdifftool`: nbdime as a git difftool (`nbdime.vcs.git.difftool`). The web interface it
//! opens for notebooks is not part of nbdime-rs (F-029); the other files go to `git diff`.

use std::io::Write;
use std::process::Command;

use anyhow::{anyhow, bail, Result};
use nbdime_core::diffing::NotebookDiffConfig;
use nbdime_core::Value;

use crate::argparse::{Action, Kind, Nargs, Parser};
use crate::args::{
    add_diff_args, add_filename_args, add_filter_args, add_generic_args, add_git_config_subcommand,
    add_web_args, apply_config_tree, not_provided, parse_or_exit, SystemExit,
};
use crate::config::Context;
use crate::filter_integration::apply_possible_filter;
use crate::gitattributes::{check_call, config_command, config_output, must_call};
use crate::osstr;
use crate::utils::Source;

const PROG: &str = "git-nbdifftool";

const DOC: &str = concat!(
    "A git difftool plugin for notebooks.\n\n",
    "Uses nbdime to display diffs of notebooks instead of plain text diffs of JSON.\n",
    "Other files are displayed via `git diff`.\n\n",
    "Enable in your global git config with:\n\n",
    "    git-nbdifftool config --enable [--global | --system]\n\n",
    "Use with:\n\n",
    "    git difftool [<commit> [<commit>]]\n",
);

/// `enable`: registers the tool, and with `set_default` makes it the default GUI difftool.
pub fn enable(scope: Option<&str>, set_default: bool) -> Result<()> {
    let call = |key: &str, value: &str| -> Result<()> {
        let mut command = config_command(scope);
        command.extend([key.to_owned(), value.to_owned()]);
        must_call(&command)
    };
    call(
        "difftool.nbdime.cmd",
        "git-nbdifftool diff \"$LOCAL\" \"$REMOTE\" \"$BASE\"",
    )?;
    if set_default {
        call("diff.guitool", "nbdime")?;
    }
    call("difftool.prompt", "false")
}

/// `disable`: unsets `diff.guitool` if it is nbdime.
pub fn disable(scope: Option<&str>) -> Result<()> {
    if config_output(scope, "diff.guitool")?.as_deref() == Some("nbdime") {
        let mut command = config_command(scope);
        command.extend(["--unset".to_owned(), "diff.guitool".to_owned()]);
        check_call(&command)?;
    }
    Ok(())
}

pub fn build_parser() -> Parser {
    let mut parser = Parser::new(PROG, DOC);
    parser.raw_description = true;
    add_generic_args(&mut parser);
    parser.add_subparsers("subcommand");

    let mut diff = Parser::new(
        &format!("{PROG} diff"),
        "The actual entrypoint for the diff tool. Git will call this.",
    );
    add_filter_args(&mut diff);
    add_generic_args(&mut diff);
    add_web_args(&mut diff, 0);
    add_diff_args(&mut diff);
    add_filename_args(&mut diff, &["local", "remote"]);
    diff.add(Action::positional("path", Nargs::One, Value::Null));
    parser.add_parser("diff", diff);

    let mut config = add_git_config_subcommand(
        PROG,
        "Configure git to use nbdime via `git difftool`",
        "enable nbdime difftool via git config",
        "disable nbdime difftool via git config",
    );
    config.add(
        Action::flag(
            Kind::StoreTrue,
            &["--set-default"],
            "set_default",
            Value::Bool(false),
        )
        .help("set nbdime as default gui difftool"),
    );
    parser.add_parser("config", config);
    parser
}

/// Runs `git-nbdifftool` with the arguments `args` (without the program name).
pub fn main(prog: &str, args: &[String], out: &mut dyn Write) -> Result<i32> {
    run(&Context::process(), prog, args, out)
}

/// [`main`] in the process `ctx` describes.
pub fn run(ctx: &Context, _prog: &str, args: &[String], out: &mut dyn Write) -> Result<i32> {
    let mut parser = build_parser();
    apply_config_tree(ctx, &mut parser, &mut NotebookDiffConfig::new())?;
    let ns = match parse_or_exit(ctx, &parser, args, out)? {
        Ok(ns) => ns,
        Err(code) => return Err(SystemExit(code).into()),
    };
    match ns.str("subcommand") {
        Some("diff") => {
            let (Some(before), Some(after)) = (ns.str("local"), ns.str("remote")) else {
                bail!("local and remote must be file names");
            };
            let filtered = match (ns.truthy("use_filter"), ns.str("path")) {
                (true, Some(path)) if !after.is_empty() => {
                    apply_possible_filter(path, Some(after))?
                }
                _ => Source::Path(after.to_owned()),
            };
            out.flush()?;
            // `before.endswith('.ipynb') or after.endswith('ipynb')`: the second is not looked
            // at when the first holds.
            if before.ends_with(".ipynb") {
                return Ok(not_provided("difftool"));
            }
            let Source::Path(after) = filtered else {
                return Err(anyhow!(
                    "'NamedStringIO' object has no attribute 'endswith'"
                ));
            };
            if after.ends_with("ipynb") {
                return Ok(not_provided("difftool"));
            }
            Ok(exec_git_diff(before, &after))
        }
        Some("config") => {
            let scope = ns.str("scope");
            match ns.str("config_func") {
                Some("enable") => enable(scope, ns.truthy("set_default"))?,
                _ => disable(scope)?,
            }
            Ok(0)
        }
        _ => {
            out.write_all(osstr::show(&parser.help_text()).as_bytes())?;
            Ok(1)
        }
    }
}

/// `os.execvp('git', ['git', 'diff', before, after])`: this process becomes `git diff`. Only
/// returns if `git` cannot be started.
#[cfg(unix)]
fn exec_git_diff(before: &str, after: &str) -> i32 {
    use std::os::unix::process::CommandExt;
    let error = git_diff(before, after).exec();
    osstr::eprint(&format!("git: {error}\n"));
    1
}

/// `os.execvp` on Windows starts the program and exits with its status.
#[cfg(not(unix))]
fn exec_git_diff(before: &str, after: &str) -> i32 {
    match git_diff(before, after).status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            osstr::eprint(&format!("git: {error}\n"));
            1
        }
    }
}

fn git_diff(before: &str, after: &str) -> Command {
    let mut command = Command::new("git");
    command
        .args(["diff"])
        .arg(osstr::to_os(before))
        .arg(osstr::to_os(after));
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> std::result::Result<crate::argparse::Namespace, String> {
        let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
        build_parser()
            .parse_args(&args)
            .map_err(|e| format!("{e:?}"))
    }

    #[test]
    fn git_gives_local_remote_and_base() {
        let ns = parse(&["diff", "--use-filter", "/tmp/l", "/tmp/r", "path.ipynb"]).unwrap();
        assert_eq!(ns.str("local"), Some("/tmp/l"));
        assert_eq!(ns.str("remote"), Some("/tmp/r"));
        assert_eq!(ns.str("path"), Some("path.ipynb"));
        assert!(ns.truthy("use_filter"));
        assert!(parse(&["diff", "/tmp/l", "/tmp/r"]).is_err());
    }

    #[test]
    fn only_the_difftool_knows_set_default() {
        let ns = parse(&["config", "--enable", "--set-default"]).unwrap();
        assert!(ns.truthy("set_default"));
        let ns = parse(&["config", "--disable"]).unwrap();
        assert!(!ns.truthy("set_default"));
    }

    #[test]
    fn the_web_options_are_checked() {
        let ns = parse(&["diff", "--port", "8000", "l", "r", "p"]).unwrap();
        assert_eq!(ns.get("port"), Some(&Value::from(8000)));
        assert!(parse(&["diff", "--port", "x", "l", "r", "p"]).is_err());
    }
}
