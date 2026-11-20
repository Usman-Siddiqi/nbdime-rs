//! `nbdiff`: the difference between two notebooks (`nbdime.nbdiffapp`).

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{bail, Context, Result};
use nbdime_core::diff_format::diff_to_value;
use nbdime_core::diffing::{diff_notebooks, NotebookDiffConfig};
use nbdime_core::json::{self, DumpOptions};
use nbdime_core::prettyprint::Printer;
use nbdime_core::Value;

use crate::argparse::{Action, Namespace, Parser};
use crate::args::{
    add_base_args_for_diff, add_diff_args, add_diff_cli_args, add_generic_args,
    add_prettyprint_args, apply_config, parse_command_line, prettyprint_config, process_diff_flags,
    resolve_diff_args, Parsed,
};
use crate::gitfiles::{changed_notebooks, is_gitref};
use crate::logging::Logger;
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
    let mut diff_config = NotebookDiffConfig::new();
    let mut parser = build_parser(prog);
    apply_config(&mut parser, &mut diff_config)?;
    let mut ns = match parse_command_line(&parser, args, crate::VERSION, prog) {
        Parsed::Namespace(ns) => ns,
        Parsed::Exit {
            stdout,
            stderr,
            code,
        } => {
            out.write_all(stdout.as_bytes())?;
            out.flush()?;
            eprint!("{stderr}");
            return Ok(code);
        }
    };
    let mut logger = Logger::default();
    if let Some(level) = &ns.log_level {
        logger.set_level(level);
    }
    main_diff(&mut ns, &mut diff_config, out)
}

fn main_diff(
    ns: &mut Namespace,
    diff_config: &mut NotebookDiffConfig,
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

    if is_gitref(base.as_deref())? && is_gitref(remote.as_deref())? {
        return changed_notebooks(
            base.as_deref(),
            remote.as_deref(),
            paths.as_deref(),
            |a, b| handle_diff(a, b, output.as_deref(), ns, diff_config, out),
        );
    }
    let source = |s: Option<String>| s.map_or(Source::Nothing, Source::Path);
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
            if !Path::new(path).exists() && path != EXPLICIT_MISSING_FILE {
                writeln!(out, "Missing file {path}")?;
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
        let mut file = fs::File::create(output).with_context(|| format!("opening {output}"))?;
        let text = json::dumps(&diff_to_value(&diff), &DumpOptions::nbdime_diff_file());
        file.write_all(text.as_bytes())?;
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
