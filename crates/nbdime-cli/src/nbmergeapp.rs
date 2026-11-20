//! `nbmerge`: three-way merge of notebooks (`nbdime.nbmergeapp`).

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{bail, Context, Result};
use nbdime_core::diffing::{diff_notebooks, NotebookDiffConfig};
use nbdime_core::json::{self, DumpOptions};
use nbdime_core::log::{self, Level};
use nbdime_core::merging::{merge_notebooks, MergeDecision, MergeDecisionBuilder};
use nbdime_core::nbformat;
use nbdime_core::prettyprint::Printer;
use nbdime_core::{Map, Value};

use crate::argparse::{Action, Kind, Namespace, Nargs, Parser};
use crate::args::{
    add_diff_args, add_generic_args, add_merge_args, add_prettyprint_args, apply_config,
    merge_args, parse_command_line, prettyprint_config, process_diff_flags, Parsed,
};
use crate::logging::Logger;
use crate::utils::{read_notebook, Fallback, Source, EXPLICIT_MISSING_FILE};

const DESCRIPTION: &str = "Merge two Jupyter notebooks \"local\" and \"remote\" with a common ancestor \"base\". If base is left out, it uses an empty notebook as the base.";

pub fn build_parser(prog: &str) -> Parser {
    let mut parser = Parser::new(prog, DESCRIPTION);
    add_generic_args(&mut parser);
    add_diff_args(&mut parser);
    add_merge_args(&mut parser);
    add_prettyprint_args(&mut parser);
    parser.add(
        Action::positional("base", Nargs::Optional, Value::from(EXPLICIT_MISSING_FILE))
            .help("The base notebook filename."),
    );
    parser.add(
        Action::positional("local", Nargs::One, Value::Null)
            .help("The local modified notebook filename."),
    );
    parser.add(
        Action::positional("remote", Nargs::One, Value::Null)
            .help("The remote modified notebook filename."),
    );
    parser.add(
        Action::store(&["--out"], "out", Value::Null).help(
            "if supplied, the merged output is written to this file. Otherwise it is printed to the terminal.",
        ),
    );
    parser.add(
        Action::flag(Kind::StoreTrue, &["--decisions"], "decisions", Value::Bool(false)).help(
            "print a human-readable summary of conflicted merge decisions instead of merging the notebook.",
        ),
    );
    parser
}

/// Runs `nbmerge` with the arguments `args` (without the program name) and returns the exit
/// status.
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
    main_merge(&mut ns, &mut diff_config, &logger, out)
}

fn path_arg(ns: &Namespace, name: &str) -> Result<String> {
    match ns.get(name) {
        Some(Value::Str(s)) => Ok(s.clone()),
        other => bail!("{name} must be a file name, not {other:?}"),
    }
}

fn main_merge(
    ns: &mut Namespace,
    diff_config: &mut NotebookDiffConfig,
    logger: &Logger,
    out: &mut dyn Write,
) -> Result<i32> {
    let (bfn, lfn, rfn) = (
        path_arg(ns, "base")?,
        path_arg(ns, "local")?,
        path_arg(ns, "remote")?,
    );
    let mfn = match ns.get("out") {
        Some(value) if value.truthy() => Some(path_arg(ns, "out")?),
        _ => None,
    };
    process_diff_flags(ns, diff_config)?;

    for file in [&bfn, &lfn, &rfn] {
        if !Path::new(file).exists() && file != EXPLICIT_MISSING_FILE {
            logger.log(
                Level::Error,
                "nbmergeapp",
                35,
                &format!("Cannot find file '{file}'"),
            );
            return Ok(1);
        }
    }

    if lfn == EXPLICIT_MISSING_FILE && rfn == EXPLICIT_MISSING_FILE {
        handle_agreed_deletion(&bfn, mfn.as_deref(), ns, diff_config, logger)?;
        return Ok(0);
    }

    // Git gives an empty base file for double insertions.
    let b = read_notebook(
        &Source::Path(bfn),
        Fallback::Minimal,
        Some(Fallback::Minimal),
    )?;
    let l = read_notebook(&Source::Path(lfn), Fallback::Minimal, None)?;
    let r = read_notebook(&Source::Path(rfn), Fallback::Minimal, None)?;

    log::take();
    let merged = merge_notebooks(&b, &l, &r, Some(&merge_args(ns)?), diff_config);
    logger.log_records(&log::take());
    let (merged, decisions) = merged?;
    let conflicted = decisions.iter().any(|d| d.conflict);
    let returncode = i32::from(conflicted);

    if conflicted {
        logger.log(
            Level::Warning,
            "nbmergeapp",
            56,
            "Conflicts occurred during merge operation.",
        );
    }

    if ns.truthy("decisions") {
        if let Some(mfn) = &mfn {
            let list = Value::List(decisions.iter().map(MergeDecision::to_value).collect());
            let mut text = json::dumps(&list, &DumpOptions::nbdime_diff_file());
            text.push('\n');
            fs::write(mfn, text).with_context(|| format!("writing {mfn}"))?;
        } else {
            log_decisions(&b, &decisions, ns, logger, 70)?;
        }
    } else if let Some(mfn) = &mfn {
        // The file is truncated before the notebook is serialized, as `open(fp, "w")` does.
        let mut file = fs::File::create(mfn).with_context(|| format!("opening {mfn}"))?;
        file.write_all(nbformat::writes(&merged)?.as_bytes())?;
        logger.log(
            Level::Info,
            "nbmergeapp",
            74,
            &format!("Merge result written to {mfn}"),
        );
    } else {
        out.write_all(nbformat::writes(&merged)?.as_bytes())?;
        out.flush()?;
    }
    Ok(returncode)
}

/// `logger.warning("Decisions:\n%s", ...)` of the pretty-printed decisions.
fn log_decisions(
    base: &Value,
    decisions: &[MergeDecision],
    ns: &Namespace,
    logger: &Logger,
    line: u32,
) -> Result<()> {
    let config = prettyprint_config(ns);
    let mut text = String::new();
    Printer::new(&config, &mut text).merge_decisions(base, decisions)?;
    logger.log(
        Level::Warning,
        "nbmergeapp",
        line,
        &format!("Decisions:\n{text}"),
    );
    Ok(())
}

/// The notebook was deleted both locally and remotely.
fn handle_agreed_deletion(
    base_fn: &str,
    output_fn: Option<&str>,
    ns: &Namespace,
    diff_config: &NotebookDiffConfig,
    logger: &Logger,
) -> Result<()> {
    if base_fn == EXPLICIT_MISSING_FILE {
        bail!("sanity check failed: cannot have agreed decision on base {base_fn:?}");
    }
    let base = read_notebook(&Source::Path(base_fn.to_owned()), Fallback::Minimal, None)?;
    if ns.truthy("decisions") {
        let diff = diff_notebooks(&base, &Value::Object(Map::new()), diff_config)?;
        let mut builder = MergeDecisionBuilder::new();
        builder.agreement(Vec::new(), Some(diff.clone()), Some(diff))?;
        log_decisions(&base, &builder.validated(), ns, logger, 99)?;
    } else if let Some(output_fn) = output_fn {
        if Path::new(output_fn).exists() {
            fs::remove_file(output_fn).with_context(|| format!("removing {output_fn}"))?;
            logger.log(
                Level::Info,
                "nbmergeapp",
                105,
                &format!("Output file deleted: {output_fn}"),
            );
        }
    }
    Ok(())
}
