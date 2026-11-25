//! `git-nbmergetool`: nbdime as a git mergetool (`nbdime.vcs.git.mergetool`). The web interface
//! it opens is not part of nbdime-rs (F-029).

use std::io::Write;

use anyhow::Result;
use nbdime_core::diffing::NotebookDiffConfig;
use nbdime_core::Value;

use crate::argparse::{Action, Kind, Parser};
use crate::args::{
    add_diff_args, add_filename_args, add_generic_args, add_git_config_subcommand, add_merge_args,
    add_web_args, apply_config_tree, not_provided, parse_or_exit, SystemExit,
};
use crate::config::Context;
use crate::gitattributes::{check_call, config_command, must_call};
use crate::osstr;

const PROG: &str = "git-nbmergetool";

const DOC: &str = concat!(
    "A git mergetool plugin for notebooks.\n\n",
    "Uses nbdime to merge notebooks instead of plain text merges of JSON.\n\n",
    "Enable in your global git config with:\n\n",
    "    git-nbmergetool config --enable [--global | --system]\n\n",
    "Use with:\n\n",
    "    git mergetool [<commit> [<commit>]]\n",
);

/// `enable`: registers the tool, and with `set_default` makes it the default mergetool.
pub fn enable(scope: Option<&str>, set_default: bool) -> Result<()> {
    let call = |key: &str, value: &str| -> Result<()> {
        let mut command = config_command(scope);
        command.extend([key.to_owned(), value.to_owned()]);
        must_call(&command)
    };
    call(
        "mergetool.nbdime.cmd",
        "git-nbmergetool merge \"$BASE\" \"$LOCAL\" \"$REMOTE\" \"$MERGED\"",
    )?;
    call("mergetool.prompt", "false")?;
    if set_default {
        call("merge.tool", "nbdime")?;
    }
    Ok(())
}

/// `disable`: unsets `merge.tool`, whatever it is; if it is not set that is not an error.
pub fn disable(scope: Option<&str>) -> Result<()> {
    let mut command = config_command(scope);
    command.extend(["--unset".to_owned(), "merge.tool".to_owned()]);
    check_call(&command)?;
    Ok(())
}

pub fn build_parser() -> Parser {
    let mut parser = Parser::new(PROG, DOC);
    parser.raw_description = true;
    add_generic_args(&mut parser);
    parser.add_subparsers("subcommand");

    let mut merge = Parser::new(
        &format!("{PROG} merge"),
        "The actual entrypoint for the mergetool. Git will call this.",
    );
    add_generic_args(&mut merge);
    add_diff_args(&mut merge);
    add_merge_args(&mut merge);
    add_web_args(&mut merge, 0);
    add_filename_args(&mut merge, &["base", "local", "remote", "merged"]);
    parser.add_parser("merge", merge);

    let mut config = add_git_config_subcommand(
        PROG,
        "Configure git to use nbdime via `git mergetool`",
        "enable nbdime mergetool via git config",
        "disable nbdime mergetool via git config",
    );
    config.add(
        Action::flag(
            Kind::StoreTrue,
            &["--set-default"],
            "set_default",
            Value::Bool(false),
        )
        .help("set nbdime as default mergetool"),
    );
    parser.add_parser("config", config);
    parser
}

/// Runs `git-nbmergetool` with the arguments `args` (without the program name).
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
        Some("merge") => {
            out.flush()?;
            Ok(not_provided("mergetool"))
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
    fn git_gives_base_local_remote_and_merged() {
        let ns = parse(&["merge", "b", "l", "r", "m"]).unwrap();
        assert_eq!(ns.str("merged"), Some("m"));
        assert!(parse(&["merge", "b", "l", "r"]).is_err());
    }

    #[test]
    fn config_takes_set_default() {
        let ns = parse(&["config", "--enable", "--set-default", "--system"]).unwrap();
        assert!(ns.truthy("set_default"));
        assert_eq!(ns.str("scope"), Some("system"));
    }
}
