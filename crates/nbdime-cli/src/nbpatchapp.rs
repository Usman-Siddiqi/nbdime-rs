//! `nbpatch`: apply the diff of `nbdiff` to a notebook (`nbdime.nbpatchapp`).

use std::fs;
use std::io::Write;

use anyhow::{anyhow, Context as _, Result};
use nbdime_core::diff_format::diff_from_value;
use nbdime_core::diffing::NotebookDiffConfig;
use nbdime_core::json;
use nbdime_core::patch::patch_notebook;
use nbdime_core::prettyprint::{PrettyPrintConfig, Printer};
use nbdime_core::Value;

use crate::argparse::{Action, Parser};
use crate::args::{add_filename_args, add_generic_args, apply_config, parse_or_exit, SystemExit};
use crate::config::Context;
use crate::nbmergeapp::write_notebook;
use crate::osstr;
use crate::utils::{read_notebook, Fallback, Source, EXPLICIT_MISSING_FILE};

const DESCRIPTION: &str = "Apply patch from nbdiff to a Jupyter notebook.";

pub fn build_parser(prog: &str) -> Parser {
    let mut parser = Parser::new(prog, DESCRIPTION);
    add_generic_args(&mut parser);
    add_filename_args(&mut parser, &["base", "patch"]);
    parser.add(
        Action::store(&["-o", "--output"], "output", Value::Null).help(
            "if supplied, the patched notebook is written to this file. Otherwise it is printed to the terminal.",
        ),
    );
    parser
}

/// Runs `nbpatch` with the arguments `args` (without the program name) and returns the exit
/// status. Text for stdout is written to `out`.
pub fn main(prog: &str, args: &[String], out: &mut dyn Write) -> Result<i32> {
    run(&Context::process(), prog, args, out)
}

/// [`main`] in the process `ctx` describes.
pub fn run(ctx: &Context, prog: &str, args: &[String], out: &mut dyn Write) -> Result<i32> {
    crate::stdstreams::setup_std_streams();
    let mut parser = build_parser(prog);
    apply_config(ctx, &mut parser, &mut NotebookDiffConfig::new())?;
    let ns = match parse_or_exit(ctx, &parser, args, out)? {
        Ok(ns) => ns,
        Err(code) => return Err(SystemExit(code).into()),
    };
    let text = |name: &str| -> Result<&str> {
        ns.str(name)
            .ok_or_else(|| anyhow!("{name} must be a file name"))
    };
    let (base, patch_file) = (text("base")?, text("patch")?);
    let output = ns.str("output").filter(|o| !o.is_empty());

    for name in [base, patch_file] {
        if !osstr::to_path(name).exists() && name != EXPLICIT_MISSING_FILE {
            writeln!(out, "Missing file {}", osstr::show(name))?;
            out.flush()?;
            return Ok(1);
        }
    }

    let before = read_notebook(&Source::Path(base.to_owned()), Fallback::Empty, None)?;
    let bytes = fs::read(osstr::to_path(patch_file))
        .with_context(|| format!("reading {}", osstr::show(patch_file)))?;
    let diff_text =
        String::from_utf8(bytes).map_err(|e| anyhow!("{}: {e}", osstr::show(patch_file)))?;
    let diff = diff_from_value(&json::parse(&diff_text)?)?;
    let after = patch_notebook(&before, &diff)?;

    match output {
        Some(output) => write_notebook(output, &after)?,
        None => {
            let config = PrettyPrintConfig::default();
            let mut text = String::new();
            let result = Printer::new(&config, &mut text).notebook(&after);
            // Upstream prints as it goes, so what was printed before an error still appears.
            out.write_all(text.as_bytes())?;
            out.flush()?;
            result?;
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{notebook, run_app, TempDir};

    #[test]
    fn a_patch_is_applied_and_the_notebook_printed() {
        let dir = TempDir::new("nbpatch-print");
        let a = dir.write("a.ipynb", &notebook(&["x = 1"]));
        let patch = dir.write(
            "p.json",
            r#"[{"op": "patch", "key": "cells", "diff": [{"op": "patch", "key": 0, "diff": [{"op": "patch", "key": "source", "diff": [{"op": "addrange", "key": 0, "valuelist": ["y = 2"]}, {"op": "removerange", "key": 0, "length": 1}]}]}]}]"#,
        );
        let (code, out) = run_app(run, "nbpatch", &[&a, &patch]);
        assert_eq!(code, 0);
        assert!(out.starts_with("notebook format: 4.5\n"), "{out}");
        assert!(out.contains("y = 2") && !out.contains("x = 1"));
    }

    #[test]
    fn the_result_can_be_written() {
        let dir = TempDir::new("nbpatch-write");
        let a = dir.write("a.ipynb", &notebook(&["x = 1"]));
        let empty = dir.write("p.json", "[]");
        let target = dir.path("out.ipynb");
        let (code, out) = run_app(run, "nbpatch", &[&a, &empty, "-o", &target]);
        assert_eq!((code, out.as_str()), (0, ""));
        let written = fs::read_to_string(&target).unwrap();
        assert!(
            written.contains("\"x = 1\"")
                && written.ends_with(&*nbdime_core::textmode::native("}\n"))
        );
        let (code, _) = run_app(run, "nbpatch", &[&a, &empty, &format!("--output={target}")]);
        assert_eq!(code, 0);
    }

    #[test]
    fn missing_files_are_reported_on_stdout() {
        let dir = TempDir::new("nbpatch-missing");
        let a = dir.write("a.ipynb", &notebook(&["x = 1"]));
        let missing = dir.path("nope.json");
        let (code, out) = run_app(run, "nbpatch", &[&a, &missing]);
        assert_eq!(code, 1);
        assert_eq!(out, format!("Missing file {missing}\n"));
    }

    #[test]
    fn a_diff_that_does_not_fit_is_an_error() {
        let dir = TempDir::new("nbpatch-bad");
        let a = dir.write("a.ipynb", &notebook(&["x = 1"]));
        let bad = dir.write(
            "bad.json",
            r#"[{"op": "patch", "key": "nope", "diff": []}]"#,
        );
        let args: Vec<String> = [a.as_str(), bad.as_str()]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert!(run(
            &crate::testutil::isolated(),
            "nbpatch",
            &args,
            &mut Vec::new()
        )
        .is_err());
    }
}
