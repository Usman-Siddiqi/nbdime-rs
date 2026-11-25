//! `nbshow`: a notebook in the terminal (`nbdime.nbshowapp`).

use std::fs;
use std::io::{Read, Write};

use anyhow::{Context as _, Result};
use nbdime_core::diffing::NotebookDiffConfig;
use nbdime_core::prettyprint::Printer;
use nbdime_core::Value;

use crate::argparse::{Action, Nargs, Parser};
use crate::args::{
    add_generic_args, apply_config, parse_command_line, prettyprint_config,
    process_exclusive_names, Parsed,
};
use crate::config::Context;
use crate::osstr;
use crate::utils::{read_notebook, Fallback, Source};

const DESCRIPTION: &str = "Show a Jupyter notebook in terminal.\nBy default shows all notebook fields.\nLimit to specific fields by passing options.\n";

/// The ignorables `nbshow` offers. The identifiers are among them but take no part in the
/// all-positive-or-all-negative rule, as upstream has it.
const EXCLUSIVE: [&str; 5] = ["sources", "outputs", "attachments", "metadata", "details"];

pub fn build_parser(prog: &str) -> Parser {
    let mut parser = Parser::new(prog, DESCRIPTION);
    add_generic_args(&mut parser);
    parser.add(
        Action::positional("notebook", Nargs::ZeroOrMore, Value::Null)
            .help("notebook filename(s) or - to read from stdin"),
    );
    for (options, dest, help) in [
        (["-s", "--sources"], "sources", "show/ignore sources."),
        (["-o", "--outputs"], "outputs", "show/ignore outputs."),
        (
            ["-a", "--attachments"],
            "attachments",
            "show/ignore attachments.",
        ),
        (["-m", "--metadata"], "metadata", "show/ignore metadata."),
        (["-i", "--id"], "id", "show/ignore identifiers."),
        (
            ["-d", "--details"],
            "details",
            "show/ignore details not covered by other options.",
        ),
    ] {
        parser.add(Action::ignorable(&options, dest).help(help).group(
            "ignorables",
            "Set which parts of the notebook (not) to show.",
        ));
    }
    parser
}

/// Runs `nbshow` with the arguments `args` (without the program name) and returns the exit
/// status. Text for stdout is written to `out`.
pub fn main(prog: &str, args: &[String], out: &mut dyn Write) -> Result<i32> {
    run(&Context::process(), prog, args, out)
}

/// [`main`] in the process `ctx` describes.
pub fn run(ctx: &Context, prog: &str, args: &[String], out: &mut dyn Write) -> Result<i32> {
    let mut parser = build_parser(prog);
    apply_config(ctx, &mut parser, &mut NotebookDiffConfig::new())?;
    let mut ns = match parse_command_line(ctx, &parser, args, crate::VERSION, prog) {
        Parsed::Namespace(ns) => ns,
        Parsed::Exit {
            stdout,
            stderr,
            code,
        } => {
            out.write_all(stdout.as_bytes())?;
            out.flush()?;
            osstr::eprint(&stderr);
            return Ok(code);
        }
    };
    process_exclusive_names(&mut ns, &EXCLUSIVE)?;
    let notebooks: Vec<String> = match ns.get("notebook") {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    };
    let config = prettyprint_config(&ns);
    main_show(&notebooks, &config, out)
}

fn main_show(
    notebooks: &[String],
    config: &nbdime_core::prettyprint::PrettyPrintConfig,
    out: &mut dyn Write,
) -> Result<i32> {
    let from_stdin = notebooks.len() == 1 && notebooks[0] == "-";
    if !from_stdin {
        for name in notebooks {
            if !osstr::to_path(name).exists() {
                writeln!(out, "Missing file {}", osstr::show(name))?;
                out.flush()?;
                return Ok(1);
            }
        }
        if notebooks.is_empty() {
            writeln!(out, "Missing filenames.")?;
            out.flush()?;
            return Ok(1);
        }
    }

    for name in notebooks {
        let nb = if from_stdin {
            read_stdin()?
        } else {
            read_file(name)?
        };
        let mut text = String::new();
        if notebooks.len() > 1 {
            // 'more' prints filenames with colons, should be good enough for us as well
            let banner = ":".repeat(14);
            text.push_str(&format!("{banner}\n{}\n{banner}\n", osstr::show(name)));
        }
        let result = Printer::new(config, &mut text).notebook(&nb);
        // Upstream prints as it goes, so what was printed before an error still appears.
        out.write_all(text.as_bytes())?;
        out.flush()?;
        result?;
    }
    Ok(0)
}

/// `nbformat.read(sys.stdin, as_version=4)`.
fn read_stdin() -> Result<Value> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .read_to_end(&mut bytes)
        .context("reading stdin")?;
    let text = String::from_utf8(bytes).context("reading stdin")?;
    read_notebook(
        &Source::Stream {
            name: "<stdin>".into(),
            text,
        },
        Fallback::Empty,
        None,
    )
}

/// `nbformat.read(path, as_version=4)`.
fn read_file(path: &str) -> Result<Value> {
    let bytes =
        fs::read(osstr::to_path(path)).with_context(|| format!("reading {}", osstr::show(path)))?;
    let text =
        String::from_utf8(bytes).with_context(|| format!("reading {}", osstr::show(path)))?;
    read_notebook(
        &Source::Stream {
            name: path.to_owned(),
            text,
        },
        Fallback::Empty,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{notebook, run_app, TempDir};

    fn notebook_with(source: &str, language: &str) -> String {
        format!(
            r#"{{"cells": [{{"cell_type": "code", "execution_count": null, "id": "c0", "metadata": {{}}, "outputs": [], "source": {source:?}}}], "metadata": {{"language_info": {language}}}, "nbformat": 4, "nbformat_minor": 5}}"#
        )
    }

    #[test]
    fn shows_the_parts_of_a_notebook() {
        let dir = TempDir::new("nbshow-basic");
        let a = dir.write("a.ipynb", &notebook(&["x = 1"]));
        let (code, out) = run_app(run, "nbshow", &[&a]);
        assert_eq!(code, 0);
        assert_eq!(
            out,
            "notebook format: 4.5\ncode cell 0:\n  id: cell0000\n  source:\n    x = 1\n"
        );
    }

    #[test]
    fn flags_select_the_parts_shown() {
        let dir = TempDir::new("nbshow-flags");
        let a = dir.write("a.ipynb", &notebook(&["x = 1"]));
        let (_, out) = run_app(run, "nbshow", &["-s", &a]);
        assert_eq!(out, "code cell 0:\n  source:\n    x = 1\n");
        let (_, out) = run_app(run, "nbshow", &["-S", &a]);
        assert_eq!(out, "notebook format: 4.5\ncode cell 0:\n  id: cell0000\n");
        // The identifiers take no part in the all-positive-or-all-negative rule.
        let (code, _) = run_app(run, "nbshow", &["-s", "-I", &a]);
        assert_eq!(code, 0);
    }

    #[test]
    fn several_files_get_banners_and_missing_ones_stop_everything() {
        let dir = TempDir::new("nbshow-many");
        let a = dir.write("a.ipynb", &notebook(&["x = 1"]));
        let (_, out) = run_app(run, "nbshow", &["-d", &a, &a]);
        let banner = format!("{}\n{a}\n{0}\n", ":".repeat(14));
        assert_eq!(
            out,
            format!("{banner}notebook format: 4.5\ncode cell 0:\n  id: cell0000\n{banner}notebook format: 4.5\ncode cell 0:\n  id: cell0000\n")
        );
        let missing = dir.path("missing.ipynb");
        let (code, out) = run_app(run, "nbshow", &[&a, &missing]);
        assert_eq!((code, out), (1, format!("Missing file {missing}\n")));
        let (code, out) = run_app(run, "nbshow", &[]);
        assert_eq!((code, out.as_str()), (1, "Missing filenames.\n"));
    }

    #[test]
    fn code_is_highlighted_in_the_language_of_the_notebook() {
        let dir = TempDir::new("nbshow-lang");
        let python = dir.write(
            "py.ipynb",
            &notebook_with("def f(): pass", r#"{"name": "python"}"#),
        );
        let (_, out) = run_app(run, "nbshow", &["-s", &python]);
        assert_eq!(
            out,
            "code cell 0:\n  source:\n    \x1b[38;5;28;01mdef\x1b[39;00m\x1b[38;5;250m \x1b[39m\x1b[38;5;21mf\x1b[39m(): \x1b[38;5;28;01mpass\x1b[39;00m\n"
        );
        // `pygments_lexer` wins over `name`; IPython lexers are not installed with nbdime.
        let ipython = dir.write(
            "ipy.ipynb",
            &notebook_with(
                "def f(): pass",
                r#"{"name": "python", "pygments_lexer": "ipython3"}"#,
            ),
        );
        let (_, out) = run_app(run, "nbshow", &["-s", &ipython]);
        assert_eq!(out, "code cell 0:\n  source:\n    def f(): pass\n");
    }

    #[test]
    fn broken_notebooks_are_errors() {
        let dir = TempDir::new("nbshow-broken");
        let args = |p: &str| vec![p.to_owned()];
        for (name, text) in [("empty", ""), ("garbage", "{not json")] {
            let path = dir.write(name, text);
            assert!(run(
                &crate::testutil::isolated(),
                "nbshow",
                &args(&path),
                &mut Vec::new()
            )
            .is_err());
        }
    }

    #[test]
    fn usage_errors_exit_with_two() {
        let (code, _) = run_app(run, "nbshow", &["--bogus"]);
        assert_eq!(code, 2);
        let (code, out) = run_app(run, "nbshow", &["--version"]);
        assert_eq!(code, 0);
        assert!(out.starts_with("nbshow "));
    }
}
