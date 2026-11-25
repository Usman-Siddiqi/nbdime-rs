//! `git-nbmergedriver`: nbdime as a git merge driver (`nbdime.vcs.git.mergedriver`).

use std::io::Write;

use anyhow::Result;
use nbdime_core::diffing::NotebookDiffConfig;
use nbdime_core::Value;

use crate::argparse::{Action, Nargs, Parser};
use crate::args::{
    add_diff_args, add_filename_args, add_generic_args, add_git_config_subcommand, add_merge_args,
    apply_config_tree, parse_or_exit, SystemExit,
};
use crate::config::Context;
use crate::gitattributes::{add_attributes, check_call, config_command, must_call};
use crate::logging::Logger;
use crate::{nbmergeapp, osstr};

const PROG: &str = "git-nbmergedriver";

const DOC: &str = concat!(
    "A git merge driver for notebooks.\n\n",
    "Uses nbdime to merge notebooks instead of plain text merges of JSON.\n",
    "Note that this requires the following to be set in .gitattributes to correctly\n",
    "identify filetypes with the driver:\n\n",
    "    *.ipynb     merge=jupyternotebook\n\n",
    "Enable in your global git config with:\n\n",
    "    git-nbmergedriver config --enable [--global | --system]\n\n",
    "Use with:\n\n",
    "    git merge [<commit> [<commit>]]\n",
);

/// `enable`: sets the driver in the git configuration and names it in the attributes.
pub fn enable(scope: Option<&str>) -> Result<()> {
    for (key, value) in [
        (
            "merge.jupyternotebook.driver",
            "git-nbmergedriver merge %O %A %B %L %P",
        ),
        (
            "merge.jupyternotebook.name",
            "jupyter notebook merge driver",
        ),
    ] {
        let mut command = config_command(scope);
        command.extend([key.to_owned(), value.to_owned()]);
        must_call(&command)?;
    }
    add_attributes(
        scope,
        "merge=jupyternotebook",
        "\n*.ipynb\tmerge=jupyternotebook\n",
    )
}

/// `disable`: removes the driver's section; a section that is not there is not an error.
pub fn disable(scope: Option<&str>) -> Result<()> {
    let mut command = config_command(scope);
    command.extend([
        "--remove-section".to_owned(),
        "merge.jupyternotebook".to_owned(),
    ]);
    check_call(&command)?;
    Ok(())
}

pub fn build_parser() -> Parser {
    let mut parser = Parser::new(PROG, DOC);
    parser.raw_description = true;
    parser.add_subparsers("subcommand");

    let mut merge = Parser::new(
        &format!("{PROG} merge"),
        "The actual entrypoint for the merge tool. Git will call this.",
    );
    add_diff_args(&mut merge);
    add_merge_args(&mut merge);
    // git's arguments: %O %A %B %L %P
    add_filename_args(&mut merge, &["base", "local", "remote"]);
    merge.add(Action::positional("marker", Nargs::One, Value::Null));
    merge.add(Action::positional("out", Nargs::Optional, Value::Null));
    parser.add_parser("merge", merge);

    add_generic_args(&mut parser);

    parser.add_parser(
        "config",
        add_git_config_subcommand(
            PROG,
            "Configure git to use nbdime for notebooks in `git merge`",
            "enable nbdime merge driver via git config",
            "disable nbdime merge driver via git config",
        ),
    );
    parser
}

/// Runs `git-nbmergedriver` with the arguments `args` (without the program name).
pub fn main(prog: &str, args: &[String], out: &mut dyn Write) -> Result<i32> {
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
        Some("merge") => {
            // "The merge driver is expected to leave the result of the merge in the file named
            // with %A by overwriting it, and exit with zero status if it managed to merge them
            // cleanly, or non-zero if there were conflicts."
            let local = ns.get("local").cloned().unwrap_or(Value::Null);
            ns.values.insert("out".to_owned(), local);
            ns.values.insert("decisions".to_owned(), Value::Bool(false));
            let mut logger = Logger::default();
            if let Some(level) = &ns.log_level {
                logger.set_level(level);
            }
            nbmergeapp::main_merge(&mut ns, &mut diff_config, &logger, out)
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

    fn parse(args: &[&str]) -> std::result::Result<crate::argparse::Namespace, String> {
        let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
        build_parser()
            .parse_args(&args)
            .map_err(|e| format!("{e:?}"))
    }

    #[test]
    fn git_gives_base_local_remote_marker_size_and_path() {
        let ns = parse(&["merge", ".merge_o", ".merge_a", ".merge_b", "7", "a.ipynb"]).unwrap();
        assert_eq!(ns.str("base"), Some(".merge_o"));
        assert_eq!(ns.str("local"), Some(".merge_a"));
        assert_eq!(ns.str("remote"), Some(".merge_b"));
        assert_eq!(ns.str("marker"), Some("7"));
        assert_eq!(ns.str("out"), Some("a.ipynb"));
        let ns = parse(&["merge", "o", "a", "b", "7"]).unwrap();
        assert_eq!(ns.get("out"), Some(&Value::Null));
    }

    #[test]
    fn options_come_after_the_merge_subcommand() {
        let ns = parse(&[
            "merge",
            "o",
            "a",
            "b",
            "7",
            "p",
            "--merge-strategy",
            "use-local",
            "-s",
        ])
        .unwrap();
        assert_eq!(ns.str("merge_strategy"), Some("use-local"));
        assert!(ns.truthy("sources"));
    }

    #[test]
    fn the_log_level_is_not_accepted_after_the_subcommand() {
        assert!(parse(&["--log-level", "ERROR", "merge", "o", "a", "b", "7", "p"]).is_ok());
        assert!(parse(&["merge", "o", "a", "b", "7", "p", "--log-level", "ERROR"]).is_err());
    }
}
