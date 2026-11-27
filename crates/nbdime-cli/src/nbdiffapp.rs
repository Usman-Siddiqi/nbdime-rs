//! `nbdiff`: the difference between two notebooks (`nbdime.nbdiffapp`).

use std::fs;
use std::io::Write;

use anyhow::{bail, Context as _, Result};
use nbdime_core::diff_format::diff_into_value;
use nbdime_core::diffing::{diff_notebooks, NotebookDiffConfig};
use nbdime_core::json::{self, DumpOptions};
use nbdime_core::prettyprint::Printer;
use nbdime_core::textmode;
use nbdime_core::Value;

use crate::argparse::{Action, Namespace, Parser};
use crate::args::{
    add_base_args_for_diff, add_diff_args, add_diff_cli_args, add_generic_args,
    add_prettyprint_args, apply_config, parse_command_line, pathspecs, prettyprint_config,
    process_diff_flags, resolve_diff_args, Parsed,
};
use crate::config::Context;
use crate::gitfiles::{changed_notebooks, is_gitref};
use crate::osstr;
use crate::utils::{read_notebook, Fallback, Source, EXPLICIT_MISSING_FILE};

const DESCRIPTION: &str = "Compute the difference between two Jupyter notebooks.";

pub fn build_parser(prog: &str) -> Parser {
    let mut parser = Parser::new(prog, DESCRIPTION);
    add_generic_args(&mut parser);
    add_diff_args(&mut parser);
    add_diff_cli_args(&mut parser);
    add_prettyprint_args(&mut parser);
    add_base_args_for_diff(&mut parser);
    parser.add(Action::store(&["--out"], "out", Value::Null).help(
        "if supplied, the diff is written to this file. Otherwise it is printed to the terminal.",
    ));
    parser
}

/// Runs `nbdiff` with the arguments `args` (without the program name) and returns the exit
/// status. Text for stdout is written to `out`.
pub fn main(prog: &str, args: &[String], out: &mut dyn Write) -> Result<i32> {
    run(&Context::process(), prog, args, out)
}

/// [`main`] in the process `ctx` describes.
pub fn run(ctx: &Context, prog: &str, args: &[String], out: &mut dyn Write) -> Result<i32> {
    let mut diff_config = NotebookDiffConfig::new();
    let mut parser = build_parser(prog);
    apply_config(ctx, &mut parser, &mut diff_config)?;
    let mut ns = match parse_command_line(ctx, &parser, args, crate::VERSION, prog) {
        Parsed::Namespace(ns) => ns,
        Parsed::Exit {
            stdout,
            stderr,
            code,
        } => {
            out.write_all(stdout.as_bytes())?;
            out.flush()?;
            crate::osstr::eprint(&stderr);
            return Ok(code);
        }
    };
    main_diff(&mut ns, &mut diff_config, None, out)
}

/// `main_diff(args)`. `filtered_remote` replaces the remote file of the command line with a
/// notebook that has been read already (the diff driver's `--use-filter`).
pub(crate) fn main_diff(
    ns: &mut Namespace,
    diff_config: &mut NotebookDiffConfig,
    filtered_remote: Option<Source>,
    out: &mut dyn Write,
) -> Result<i32> {
    let output = match ns.get("out") {
        Some(value) if value.truthy() => Some(
            value
                .as_str()
                .context("--out must be a file name")?
                .to_owned(),
        ),
        _ => None,
    };
    process_diff_flags(ns, diff_config)?;
    let (base, remote, paths) = resolve_diff_args(ns)?;

    let source = |s: Option<String>| s.map_or(Source::Nothing, Source::Path);
    if let Some(stream) = filtered_remote {
        if is_gitref(base.as_deref())? {
            // `os.path.exists` of a file object.
            bail!("stat: path should be string, bytes, os.PathLike or integer, not NamedStringIO");
        }
        return handle_diff(
            source(base),
            stream,
            output.as_deref(),
            ns,
            diff_config,
            out,
        );
    }
    if is_gitref(base.as_deref())? && is_gitref(remote.as_deref())? {
        return changed_notebooks(
            base.as_deref(),
            remote.as_deref(),
            pathspecs(paths.as_ref())?.as_deref(),
            |a, b| handle_diff(a, b, output.as_deref(), ns, diff_config, out),
        );
    }
    handle_diff(
        source(base),
        source(remote),
        output.as_deref(),
        ns,
        diff_config,
        out,
    )
}

fn handle_diff(
    base: Source,
    remote: Source,
    output: Option<&str>,
    ns: &Namespace,
    diff_config: &NotebookDiffConfig,
    out: &mut dyn Write,
) -> Result<i32> {
    for source in [&base, &remote] {
        if let Source::Path(path) = source {
            if !osstr::to_path(path).exists() && path != EXPLICIT_MISSING_FILE {
                writeln!(out, "Missing file {}", osstr::show(path))?;
                out.flush()?;
                return Ok(1);
            }
        }
    }
    if base.is_missing_file() && remote.is_missing_file() {
        bail!("cannot diff {EXPLICIT_MISSING_FILE:?} against {EXPLICIT_MISSING_FILE:?}");
    }

    let a = read_notebook(&base, Fallback::Empty, None)?;
    let b = read_notebook(&remote, Fallback::Empty, None)?;
    let diff = diff_notebooks(&a, &b, diff_config)?;

    if let Some(output) = output {
        // Opened (and truncated) before anything is written, like `open(output, "w")`.
        let mut file = fs::File::create(osstr::to_path(output))
            .with_context(|| format!("opening {}", osstr::show(output)))?;
        let text = json::dumps(&diff_into_value(diff), &DumpOptions::nbdime_diff_file());
        file.write_all(textmode::native(&text).as_bytes())?;
    } else {
        let config = prettyprint_config(ns);
        let mut text = String::new();
        let result = Printer::new(&config, &mut text).notebook_diff(
            base.name().unwrap_or_default(),
            remote.name().unwrap_or_default(),
            &a,
            &diff,
        );
        // Upstream prints as it goes, so what was printed before an error still appears.
        out.write_all(text.as_bytes())?;
        out.flush()?;
        result?;
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{notebook, run_app, TempDir};

    #[test]
    fn missing_file_is_reported() {
        let dir = TempDir::new("nbdiff-missing");
        let a = dir.write("a.ipynb", &notebook(&["x = 1"]));
        let missing = dir.path("nope.ipynb");
        let (code, out) = run_app(run, "nbdiff", &[&a, &missing]);
        assert_eq!(code, 1);
        assert_eq!(out, format!("Missing file {missing}\n"));
    }

    #[test]
    fn identical_notebooks_print_nothing() {
        let dir = TempDir::new("nbdiff-same");
        let a = dir.write("a.ipynb", &notebook(&["x = 1"]));
        let (code, out) = run_app(run, "nbdiff", &[&a, &a]);
        assert_eq!((code, out.as_str()), (0, ""));
    }

    #[test]
    fn json_output_has_no_trailing_newline() {
        let dir = TempDir::new("nbdiff-json");
        let a = dir.write("a.ipynb", &notebook(&["x = 1"]));
        let b = dir.write("b.ipynb", &notebook(&["x = 2"]));
        let target = dir.path("diff.json");
        let (code, out) = run_app(run, "nbdiff", &[&a, &b, "--out", &target]);
        assert_eq!((code, out.as_str()), (0, ""));
        let written = fs::read_to_string(&target).unwrap();
        assert!(written.starts_with(&*textmode::native("[\n  {\n    \"op\": \"patch\"")));
        assert!(!written.ends_with('\n'));
    }

    #[test]
    fn text_output_names_both_files() {
        let dir = TempDir::new("nbdiff-text");
        let a = dir.write("a.ipynb", &notebook(&["x = 1"]));
        let b = dir.write("b.ipynb", &notebook(&["x = 2"]));
        let (code, out) = run_app(run, "nbdiff", &[&a, &b, "--no-color", "--no-git"]);
        assert_eq!(code, 0);
        assert!(out.starts_with(&format!("nbdiff {a} {b}\n--- {a}  ")));
        assert!(out.contains("## modified /cells/0/source:\n-  x = 1\n+  x = 2\n"));
    }

    #[test]
    fn null_file_against_a_notebook() {
        let dir = TempDir::new("nbdiff-null");
        let a = dir.write("a.ipynb", &notebook(&["x = 1"]));
        let nul = EXPLICIT_MISSING_FILE;
        let (code, out) = run_app(run, "nbdiff", &[nul, &a, "--no-color"]);
        assert_eq!(code, 0);
        assert!(out.starts_with(&format!("nbdiff {nul} {a}\n")));
        assert!(run_app_err(&[nul, nul]));
    }

    fn run_app_err(args: &[&str]) -> bool {
        let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
        run(
            &crate::testutil::isolated(),
            "nbdiff",
            &args,
            &mut Vec::new(),
        )
        .is_err()
    }

    #[test]
    fn broken_input_is_an_error() {
        let dir = TempDir::new("nbdiff-broken");
        let a = dir.write("a.ipynb", &notebook(&["x = 1"]));
        let empty = dir.write("empty.ipynb", "");
        assert!(run_app_err(&[&empty, &a]));
        let garbage = dir.write("garbage.ipynb", "{not json");
        assert!(run_app_err(&[&garbage, &a]));
    }

    #[test]
    fn usage_errors_exit_with_two() {
        let dir = TempDir::new("nbdiff-usage");
        let a = dir.write("a.ipynb", &notebook(&["x = 1"]));
        let (code, _) = run_app(run, "nbdiff", &["--bogus-flag", &a, &a]);
        assert_eq!(code, 2);
        let (code, out) = run_app(run, "nbdiff", &["--version"]);
        assert_eq!(code, 0);
        assert!(out.starts_with("nbdiff "));
    }

    #[test]
    fn mixed_ignorables_fail_after_parsing() {
        let dir = TempDir::new("nbdiff-mixed");
        let a = dir.write("a.ipynb", &notebook(&["x = 1"]));
        assert!(run_app_err(&["-s", "-O", &a, &a]));
    }
}
