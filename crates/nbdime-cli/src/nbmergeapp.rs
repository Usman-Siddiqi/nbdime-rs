//! `nbmerge`: three-way merge of notebooks (`nbdime.nbmergeapp`).

use std::fs;
use std::io::Write;

use anyhow::{bail, Context as _, Result};
use nbdime_core::diffing::{diff_notebooks, NotebookDiffConfig};
use nbdime_core::json::{self, DumpOptions};
use nbdime_core::log::{self, Level};
use nbdime_core::merging::{merge_notebooks, MergeDecision, MergeDecisionBuilder};
use nbdime_core::nbformat;
use nbdime_core::prettyprint::Printer;
use nbdime_core::value::Int;
use nbdime_core::{Map, Value};

use crate::argparse::{Action, Kind, Namespace, Nargs, Parser};
use crate::args::{
    add_diff_args, add_generic_args, add_merge_args, add_prettyprint_args, apply_config,
    merge_args, parse_command_line, prettyprint_config, process_diff_flags, Parsed,
};
use crate::config::Context;
use crate::logging::Logger;
use crate::osstr;
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
    let mut logger = Logger::default();
    if let Some(level) = &ns.log_level {
        logger.set_level(level);
    }
    main_merge(&mut ns, &mut diff_config, &logger, out)
}

/// A file argument: its name, or what a configuration file put there instead.
fn path_arg(ns: &Namespace, name: &str) -> Value {
    ns.get(name).cloned().unwrap_or(Value::Null)
}

/// `os.path.exists(fn)` of something that is not a string, which can only come from a
/// configuration file: an integer is a file descriptor, anything else is a `TypeError`.
fn odd_file_exists(value: &Value) -> Result<bool> {
    match value {
        Value::Int(Int::Small(fd)) if (0..=i64::from(i32::MAX)).contains(fd) => {
            Ok(fs::metadata(format!("/proc/self/fd/{fd}")).is_ok())
        }
        Value::Int(Int::Small(_)) => Ok(false),
        other => bail!("stat: path should be string, bytes, os.PathLike or integer, not {other:?}"),
    }
}

/// `%s` of a value that is not a string.
fn display_value(value: &Value) -> String {
    match value {
        Value::Int(Int::Small(n)) => n.to_string(),
        other => format!("{other:?}"),
    }
}

fn main_merge(
    ns: &mut Namespace,
    diff_config: &mut NotebookDiffConfig,
    logger: &Logger,
    out: &mut dyn Write,
) -> Result<i32> {
    let files = [
        path_arg(ns, "base"),
        path_arg(ns, "local"),
        path_arg(ns, "remote"),
    ];
    let mfn = match ns.get("out") {
        Some(value) if value.truthy() => Some(match value {
            Value::Str(out) => out.clone(),
            other => bail!("--out must be a file name, not {other:?}"),
        }),
        _ => None,
    };
    process_diff_flags(ns, diff_config)?;

    for file in &files {
        let missing = match file {
            Value::Str(name) => !osstr::to_path(name).exists() && name != EXPLICIT_MISSING_FILE,
            odd => !odd_file_exists(odd)?,
        };
        if missing {
            let shown = match file {
                Value::Str(name) => name.clone(),
                odd => display_value(odd),
            };
            logger.log(
                Level::Error,
                "nbmergeapp",
                35,
                &format!("Cannot find file '{}'", osstr::show(&shown)),
            );
            return Ok(1);
        }
    }
    let [Value::Str(bfn), Value::Str(lfn), Value::Str(rfn)] = files else {
        bail!("a notebook must be given as a file name");
    };

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

    log::begin();
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
            fs::write(osstr::to_path(mfn), text)
                .with_context(|| format!("writing {}", osstr::show(mfn)))?;
        } else {
            log_decisions(&b, &decisions, ns, logger, 70)?;
        }
    } else if let Some(mfn) = &mfn {
        write_notebook(mfn, &merged)?;
        logger.log(
            Level::Info,
            "nbmergeapp",
            74,
            &format!("Merge result written to {}", osstr::show(mfn)),
        );
    } else {
        out.write_all(nbformat::writes(&merged)?.as_bytes())?;
        out.flush()?;
    }
    Ok(returncode)
}

/// `nbformat.write(notebook, path)`: the text is made before the file is opened, so a notebook
/// that cannot be serialized leaves an existing file alone.
fn write_notebook(path: &str, notebook: &Value) -> Result<()> {
    let text = nbformat::writes(notebook)?;
    let mut file = fs::File::create(osstr::to_path(path))
        .with_context(|| format!("opening {}", osstr::show(path)))?;
    file.write_all(text.as_bytes())?;
    Ok(())
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
        bail!(
            "sanity check failed: cannot have agreed decision on base {:?}",
            osstr::show(base_fn)
        );
    }
    let base = read_notebook(&Source::Path(base_fn.to_owned()), Fallback::Minimal, None)?;
    if ns.truthy("decisions") {
        let diff = diff_notebooks(&base, &Value::Object(Map::new()), diff_config)?;
        let mut builder = MergeDecisionBuilder::new();
        builder.agreement(Vec::new(), Some(diff.clone()), Some(diff))?;
        log_decisions(&base, &builder.validated(), ns, logger, 99)?;
    } else if let Some(output_fn) = output_fn {
        if osstr::to_path(output_fn).exists() {
            fs::remove_file(osstr::to_path(output_fn))
                .with_context(|| format!("removing {}", osstr::show(output_fn)))?;
            logger.log(
                Level::Info,
                "nbmergeapp",
                105,
                &format!("Output file deleted: {}", osstr::show(output_fn)),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{notebook, run_app, TempDir};
    use std::path::Path;

    #[test]
    fn a_notebook_that_cannot_be_written_leaves_the_file_alone() {
        let dir = TempDir::new("nbmerge-unwritable");
        let target = dir.write("out.ipynb", "old");
        let invalid = nbdime_core::json::parse(r#"{"nbformat": 4}"#).unwrap();
        assert!(write_notebook(&target, &invalid).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "old");
        write_notebook(&target, &nbformat::new_notebook()).unwrap();
        assert!(fs::read_to_string(&target).unwrap().contains("\"cells\""));
    }

    #[test]
    fn a_file_argument_from_a_config_file_may_be_a_number() {
        assert!(!odd_file_exists(&Value::from(99_999)).unwrap());
        assert!(!odd_file_exists(&Value::from(-1)).unwrap());
        assert!(odd_file_exists(&Value::from(0)).unwrap());
        assert!(odd_file_exists(&Value::Float(5.5)).is_err());
        assert!(odd_file_exists(&Value::Null).is_err());
        assert_eq!(display_value(&Value::from(5)), "5");
    }

    #[test]
    fn clean_merge_goes_to_stdout() {
        let dir = TempDir::new("nbmerge-clean");
        let base = dir.write("b.ipynb", &notebook(&["a = 1", "b = 2"]));
        let local = dir.write("l.ipynb", &notebook(&["a = 1", "b = 3"]));
        let remote = dir.write("r.ipynb", &notebook(&["a = 0", "b = 2"]));
        let (code, out) = run_app(run, "nbmerge", &[&base, &local, &remote]);
        assert_eq!(code, 0);
        assert!(out.contains("a = 0") && out.contains("b = 3"));
        assert!(out.ends_with("}\n"));
    }

    #[test]
    fn conflict_sets_the_exit_status_and_writes_the_file() {
        let dir = TempDir::new("nbmerge-conflict");
        let base = dir.write("b.ipynb", &notebook(&["a = 1"]));
        let local = dir.write("l.ipynb", &notebook(&["a = 2"]));
        let remote = dir.write("r.ipynb", &notebook(&["a = 3"]));
        let target = dir.path("merged.ipynb");
        let (code, out) = run_app(run, "nbmerge", &[&base, &local, &remote, "--out", &target]);
        assert_eq!((code, out.as_str()), (1, ""));
        let merged = fs::read_to_string(&target).unwrap();
        assert!(merged.contains("<<<<<<< local") && merged.contains(">>>>>>> remote"));
    }

    #[test]
    fn base_defaults_to_the_null_file() {
        let dir = TempDir::new("nbmerge-nobase");
        let local = dir.write("l.ipynb", &notebook(&["a = 2"]));
        let remote = dir.write("r.ipynb", &notebook(&["a = 2"]));
        let (code, out) = run_app(run, "nbmerge", &[&local, &remote]);
        assert_eq!(code, 0);
        assert!(out.contains("a = 2"));
    }

    #[test]
    fn missing_file_exits_with_one() {
        let dir = TempDir::new("nbmerge-missing");
        let base = dir.write("b.ipynb", &notebook(&["a = 1"]));
        let missing = dir.path("nope.ipynb");
        let (code, out) = run_app(run, "nbmerge", &[&base, &missing, &base]);
        assert_eq!((code, out.as_str()), (1, ""));
    }

    #[test]
    fn decisions_are_written_as_json() {
        let dir = TempDir::new("nbmerge-decisions");
        let base = dir.write("b.ipynb", &notebook(&["a = 1"]));
        let local = dir.write("l.ipynb", &notebook(&["a = 2"]));
        let remote = dir.write("r.ipynb", &notebook(&["a = 1"]));
        let target = dir.path("decisions.json");
        let (code, _) = run_app(
            run,
            "nbmerge",
            &[&base, &local, &remote, "--decisions", "--out", &target],
        );
        assert_eq!(code, 0);
        let written = fs::read_to_string(&target).unwrap();
        assert!(written.starts_with("[\n  {") && written.ends_with("]\n"));
    }

    #[test]
    fn agreed_deletion_removes_the_output() {
        let dir = TempDir::new("nbmerge-deleted");
        let base = dir.write("b.ipynb", &notebook(&["a = 1"]));
        let target = dir.write("out.ipynb", "old");
        let (code, _) = run_app(
            run,
            "nbmerge",
            &[&base, "/dev/null", "/dev/null", "--out", &target],
        );
        assert_eq!(code, 0);
        assert!(!Path::new(&target).exists());
        let args: Vec<String> = ["/dev/null", "/dev/null", "/dev/null"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert!(run(
            &crate::testutil::isolated(),
            "nbmerge",
            &args,
            &mut Vec::new()
        )
        .is_err());
    }

    #[test]
    fn invalid_strategy_is_a_usage_error() {
        let dir = TempDir::new("nbmerge-strategy");
        let base = dir.write("b.ipynb", &notebook(&["a = 1"]));
        let (code, _) = run_app(
            run,
            "nbmerge",
            &[&base, &base, &base, "--merge-strategy", "nonsense"],
        );
        assert_eq!(code, 2);
    }
}
