//! `git-nbdiffdriver`: nbdime as a git diff driver (`nbdime.vcs.git.diffdriver`).

use std::io::Write;

use anyhow::Result;
use nbdime_core::diffing::NotebookDiffConfig;

use crate::argparse::{Namespace, Parser};
use crate::args::{
    add_diff_args, add_diff_cli_args, add_generic_args, add_git_config_subcommand,
    add_git_diff_driver_args, add_prettyprint_args, add_web_args, apply_config_tree, not_provided,
    parse_or_exit, SystemExit,
};
use crate::config::Context;
use crate::filter_integration::apply_possible_filter;
use crate::gitattributes::{add_attributes, check_call, config_command};
use crate::utils::Source;
use crate::{nbdiffapp, osstr};

const PROG: &str = "git-nbdiffdriver";

const DOC: &str = concat!(
    "A git diff driver for notebooks.\n\n",
    "Uses nbdime to create diffs for notebooks instead of plain text diffs of JSON.\n",
    "Note that this requires the following to be set in .gitattributes to correctly\n",
    "identify filetypes with the driver:\n\n",
    "    *.ipynb     diff=jupyternotebook\n\n",
    "Enable in your global git config with:\n\n",
    "    git-nbdiffdriver config --enable [--global | --system]\n\n",
    "Use with:\n\n",
    "    git diff [<commit> [<commit>]]\n",
);

/// `enable`: sets the driver in the git configuration and names it in the attributes.
pub fn enable(scope: Option<&str>) -> Result<()> {
    let mut command = config_command(scope);
    command.extend([
        "diff.jupyternotebook.command".to_owned(),
        "git-nbdiffdriver diff".to_owned(),
    ]);
    if !check_call(&command)? {
        // Not in a git repository.
        return Ok(());
    }
    add_attributes(
        scope,
        "diff=jupyternotebook",
        "\n*.ipynb\tdiff=jupyternotebook\n",
    )
}

/// `disable`: removes the driver's section; a section that is not there is not an error.
pub fn disable(scope: Option<&str>) -> Result<()> {
    let mut command = config_command(scope);
    command.extend([
        "--remove-section".to_owned(),
        "diff.jupyternotebook".to_owned(),
    ]);
    check_call(&command)?;
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
    add_git_diff_driver_args(&mut diff);
    add_diff_args(&mut diff);
    add_diff_cli_args(&mut diff);
    add_prettyprint_args(&mut diff);
    parser.add_parser("diff", diff);

    let mut webdiff = Parser::new(
        &format!("{PROG} webdiff"),
        "The actual entrypoint for the webdiff tool. Git will call this.",
    );
    add_git_diff_driver_args(&mut webdiff);
    add_diff_args(&mut webdiff);
    add_web_args(&mut webdiff, 0);
    parser.add_parser("webdiff", webdiff);

    parser.add_parser(
        "config",
        add_git_config_subcommand(
            PROG,
            "Configure git to use nbdime for notebooks in `git diff`",
            "enable nbdime diff driver via git config",
            "disable nbdime diff driver via git config",
        ),
    );
    parser
}

/// With `--use-filter`, the remote side is run through the clean filter of the path.
fn filtered_remote(ns: &Namespace) -> Result<Option<Source>> {
    let (Some(path), Some(remote)) = (ns.str("path"), ns.str("remote")) else {
        return Ok(None);
    };
    if !ns.truthy("use_filter") || remote.is_empty() {
        return Ok(None);
    }
    Ok(match apply_possible_filter(path, Some(remote))? {
        Source::Path(_) => None,
        stream => Some(stream),
    })
}

/// Runs `git-nbdiffdriver` with the arguments `args` (without the program name).
pub fn main(prog: &str, args: &[String], out: &mut dyn Write) -> Result<i32> {
    crate::stdstreams::setup_std_streams();
    run(&Context::process(), prog, args, out)
}

/// [`main`] in the process `ctx` describes.
pub fn run(ctx: &Context, _prog: &str, args: &[String], out: &mut dyn Write) -> Result<i32> {
    let mut diff_config = NotebookDiffConfig::new();
    let mut parser = build_parser();
    apply_config_tree(ctx, &mut parser, &mut diff_config)?;
    let mut ns = match parse_or_exit(ctx, &parser, args, out)? {
        Ok(ns) => ns,
        Err(code) => return Err(SystemExit(code).into()),
    };
    match ns.str("subcommand") {
        Some("diff") => {
            let remote = filtered_remote(&ns)?;
            nbdiffapp::main_diff(&mut ns, &mut diff_config, remote, out)
        }
        Some("webdiff") => {
            out.flush()?;
            Ok(not_provided("webdiff"))
        }
        Some("config") => {
            let scope = ns.str("scope");
            match ns.str("config_func") {
                Some("enable") => enable(scope)?,
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

    fn parse(args: &[&str]) -> std::result::Result<Namespace, String> {
        let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
        build_parser()
            .parse_args(&args)
            .map_err(|e| format!("{e:?}"))
    }

    #[test]
    fn git_gives_seven_arguments_and_only_three_are_kept() {
        let ns = parse(&[
            "diff",
            "--no-color",
            "a.ipynb",
            "/tmp/old",
            "abc",
            "100644",
            "/tmp/new",
            "def",
            "100644",
        ])
        .unwrap();
        assert_eq!(ns.str("subcommand"), Some("diff"));
        assert_eq!(ns.str("path"), Some("a.ipynb"));
        assert_eq!(ns.str("base"), Some("/tmp/old"));
        assert_eq!(ns.str("remote"), Some("/tmp/new"));
        assert!(!ns.truthy("use_color") && !ns.truthy("use_filter"));
        assert!(ns.get("base_sha1").is_none() && ns.get("==SUPPRESS==").is_none());
    }

    #[test]
    fn renames_bring_two_more_arguments() {
        let ns = parse(&[
            "diff",
            "a",
            "o",
            "h",
            "m",
            "n",
            "h",
            "m",
            "b",
            "similarity index 100%",
        ])
        .unwrap();
        assert_eq!(ns.str("remote"), Some("n"));
        assert!(parse(&["diff", "a", "o", "h", "m", "n", "h", "m", "b", "r", "extra"]).is_err());
    }

    #[test]
    fn an_unmerged_path_comes_alone() {
        let ns = parse(&["diff", "a.ipynb"]).unwrap();
        assert_eq!(ns.get("base"), Some(&nbdime_core::Value::Null));
        assert_eq!(ns.get("remote"), Some(&nbdime_core::Value::Null));
    }

    #[test]
    fn the_log_level_belongs_to_the_command_not_the_subcommand() {
        assert!(parse(&["--log-level", "ERROR", "diff", "a"]).is_ok());
        assert!(parse(&["diff", "a", "--log-level", "ERROR"]).is_err());
    }

    #[test]
    fn config_needs_exactly_one_of_enable_and_disable() {
        let ns = parse(&["config", "--enable", "--global"]).unwrap();
        assert_eq!(ns.str("config_func"), Some("enable"));
        assert_eq!(ns.str("scope"), Some("global"));
        assert!(parse(&["config"]).is_err());
        assert!(parse(&["config", "--enable", "--disable"]).is_err());
        assert!(parse(&["config", "--enable", "--global", "--system"]).is_err());
    }

    #[test]
    fn help_without_a_subcommand_shows_the_usage() {
        let mut out = Vec::new();
        let code = run(&crate::testutil::isolated(), PROG, &[], &mut out).unwrap();
        assert_eq!(code, 1);
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("usage: git-nbdiffdriver [-h]"));
        assert!(text.contains("{diff,webdiff,config} ..."));
    }
}
