//! Function table for `nbdime-rs-dev batch`, used by the parity harness.
//!
//! Each entry takes the request's `args` object and returns a JSON value that the harness
//! compares with what upstream nbdime returns for the same call.

use anyhow::{anyhow, bail, Context, Result};
use nbdime_core::diff_format::{diff_from_value, diff_to_value};
use nbdime_core::diff_utils::{flatten_list_of_string_diff, StrOrLines};
use nbdime_core::diffing::{
    self, snakes::compute_snakes_multilevel, NotebookDiffConfig, Predicate, Targets,
};
use nbdime_core::difflib::SequenceMatcher;
use nbdime_core::json::{self, DumpOptions};
use nbdime_core::nbformat;
use nbdime_core::patch::patch;
use nbdime_core::{Int, Value};

/// Stack size for threads that run the core on untrusted input.
pub const WORKER_STACK: usize = 512 << 20;

pub fn dispatch(name: &str, args: &Value) -> Result<Value> {
    match name {
        "ping" => Ok(Value::from("pong")),
        "json.loads_dumps" => json_loads_dumps(args),
        "splitlines" => {
            let s = str_arg(args, "s")?;
            Ok(Value::List(
                nbdime_core::py::splitlines_keepends(s)
                    .into_iter()
                    .map(Value::from)
                    .collect(),
            ))
        }
        "difflib.opcodes" => difflib_opcodes(args),
        "difflib.matching_blocks" => difflib_matching_blocks(args),
        "difflib.ratios" => difflib_ratios(args),
        "compare_strings_approximate" => compare_strings_approximate(args),
        "diff_strings_by_char" => {
            let (a, b) = (str_arg(args, "a")?, str_arg(args, "b")?);
            Ok(diff_to_value(&diffing::diff_strings_by_char(a, b)))
        }
        "diff_strings_linewise" => {
            let (a, b) = (str_arg(args, "a")?, str_arg(args, "b")?);
            Ok(diff_to_value(&diffing::diff_strings_linewise(a, b)))
        }
        "snakes_multilevel_lines" => snakes_multilevel_lines(args),
        "diff_sequence_bruteforce" => {
            let (a, b) = (list_arg(args, "a")?, list_arg(args, "b")?);
            Ok(diff_to_value(&diffing::sequence::diff_sequence_bruteforce(
                a,
                b,
                |x, y| x.py_eq(y),
            )))
        }
        "patch" => {
            let diff = diff_from_value(arg(args, "diff")?)?;
            Ok(patch(arg(args, "obj")?, &diff)?)
        }
        "flatten_list_of_string_diff" => flatten(args),
        "nbformat.reads" => Ok(nbformat::reads(str_arg(args, "text")?)?),
        "nbformat.writes" => Ok(Value::from(nbformat::writes(arg(args, "nb")?)?)),
        "diff" => {
            let diff = diffing::diff(arg(args, "a")?, arg(args, "b")?)?;
            Ok(diff_to_value(&diff))
        }
        "diff_notebooks" => diff_notebooks(args),
        "notebook.compare" => notebook_compare(args),
        _ => bail!("unknown function {name:?}"),
    }
}

/// `{a, b, ignores?, targets?}`: ignores apply first, in object order, then the targets.
fn diff_notebooks(args: &Value) -> Result<Value> {
    let mut config = NotebookDiffConfig::new();
    if let Some(ignores) = args.get("ignores").and_then(Value::as_object) {
        let ignores: Vec<_> = ignores
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        config.set_ignores(&ignores)?;
    }
    if let Some(targets) = args.get("targets") {
        let flag = |name: &str| arg(targets, name).map(Value::truthy);
        config.set_targets(&Targets {
            sources: flag("sources")?,
            outputs: flag("outputs")?,
            attachments: flag("attachments")?,
            metadata: flag("metadata")?,
            identifier: flag("identifier")?,
            details: flag("details")?,
        });
    }
    let diff = diffing::diff_notebooks(arg(args, "a")?, arg(args, "b")?, &config)?;
    Ok(diff_to_value(&diff))
}

/// `{pred, x, y}` with the default notebook configuration.
fn notebook_compare(args: &Value) -> Result<Value> {
    let predicate = match str_arg(args, "pred")? {
        "cell_approximate" => Predicate::CellApproximate,
        "cell_moderate" => Predicate::CellModerate,
        "cell_strict" => Predicate::CellStrict,
        "cell_by_ids" => Predicate::CellById,
        "output_approximate" => Predicate::OutputApproximate,
        "output_strict" => Predicate::OutputStrict,
        other => bail!("unknown predicate {other:?}"),
    };
    let config = NotebookDiffConfig::new();
    Ok(Value::Bool(predicate.compare(
        arg(args, "x")?,
        arg(args, "y")?,
        config.config(),
    )?))
}

fn chars_arg(args: &Value, name: &str) -> Result<Vec<char>> {
    Ok(str_arg(args, name)?.chars().collect())
}

fn difflib_opcodes(args: &Value) -> Result<Value> {
    let (a, b) = (chars_arg(args, "a")?, chars_arg(args, "b")?);
    Ok(Value::List(
        SequenceMatcher::new(&a, &b)
            .opcodes()
            .into_iter()
            .map(|op| {
                Value::List(vec![
                    Value::from(op.tag.as_str()),
                    Value::from(op.a.start),
                    Value::from(op.a.end),
                    Value::from(op.b.start),
                    Value::from(op.b.end),
                ])
            })
            .collect(),
    ))
}

fn difflib_matching_blocks(args: &Value) -> Result<Value> {
    let (a, b) = (chars_arg(args, "a")?, chars_arg(args, "b")?);
    Ok(Value::List(
        SequenceMatcher::new(&a, &b)
            .matching_blocks()
            .iter()
            .map(|m| {
                Value::List(vec![
                    Value::from(m.a),
                    Value::from(m.b),
                    Value::from(m.size),
                ])
            })
            .collect(),
    ))
}

fn difflib_ratios(args: &Value) -> Result<Value> {
    let (a, b) = (chars_arg(args, "a")?, chars_arg(args, "b")?);
    let matcher = SequenceMatcher::new(&a, &b);
    Ok(Value::List(vec![
        Value::Float(matcher.ratio()),
        Value::Float(matcher.quick_ratio()),
        Value::Float(matcher.real_quick_ratio()),
    ]))
}

fn compare_strings_approximate(args: &Value) -> Result<Value> {
    let (x, y) = (str_arg(args, "x")?, str_arg(args, "y")?);
    let threshold = match args.get("threshold") {
        None | Some(Value::Null) => 0.7,
        Some(Value::Float(f)) => *f,
        Some(Value::Int(Int::Small(i))) => *i as f64,
        Some(other) => bail!("bad threshold {other:?}"),
    };
    let maxlen = match args.get("maxlen") {
        None | Some(Value::Null) => None,
        Some(Value::Int(Int::Small(i))) => Some(usize::try_from(*i).context("bad maxlen")?),
        Some(other) => bail!("bad maxlen {other:?}"),
    };
    Ok(Value::Bool(diffing::compare_strings_approximate(
        x, y, threshold, maxlen,
    )))
}

fn snakes_multilevel_lines(args: &Value) -> Result<Value> {
    let lines = |name: &str| -> Result<Vec<&str>> {
        list_arg(args, name)?
            .iter()
            .map(|v| v.as_str().context("lines must be strings"))
            .collect()
    };
    let (a, b) = (lines("a")?, lines("b")?);
    let approximate = |x: &&str, y: &&str| diffing::compare_strings_approximate(x, y, 0.7, None);
    let equal = |x: &&str, y: &&str| x == y;
    let snakes = compute_snakes_multilevel(&a, &b, &[&approximate, &equal]);
    Ok(Value::List(
        snakes
            .into_iter()
            .map(|s| Value::List(vec![Value::from(s.i), Value::from(s.j), Value::from(s.n)]))
            .collect(),
    ))
}

fn flatten(args: &Value) -> Result<Value> {
    let diff = diff_from_value(arg(args, "diff")?)?;
    let flat = match arg(args, "a")? {
        Value::Str(s) => flatten_list_of_string_diff(StrOrLines::Str(s), &diff)?,
        Value::List(items) => {
            let lines: Vec<&str> = items
                .iter()
                .map(|v| v.as_str().context("lines must be strings"))
                .collect::<Result<_>>()?;
            flatten_list_of_string_diff(StrOrLines::Lines(&lines), &diff)?
        }
        _ => bail!("argument \"a\" must be a string or a list of strings"),
    };
    Ok(diff_to_value(&flat))
}

fn list_arg<'a>(args: &'a Value, name: &str) -> Result<&'a [Value]> {
    arg(args, name)?
        .as_list()
        .map(Vec::as_slice)
        .ok_or_else(|| anyhow!("argument {name:?} must be a list"))
}

/// Parses `text` and serializes it with Python `json.dumps` options; returns the text.
fn json_loads_dumps(args: &Value) -> Result<Value> {
    let text = str_arg(args, "text")?;
    let value = json::parse(text).context("parse")?;
    let options = dump_options(args.get("options"))?;
    Ok(Value::from(json::dumps(&value, &options)))
}

fn dump_options(v: Option<&Value>) -> Result<DumpOptions> {
    let mut options = DumpOptions::python_default();
    let Some(v) = v else { return Ok(options) };
    if let Some(indent) = v.get("indent") {
        options.indent = match indent {
            Value::Null => None,
            Value::Int(i) => Some(
                i.as_i64()
                    .and_then(|i| usize::try_from(i).ok())
                    .ok_or_else(|| anyhow!("bad indent"))?,
            ),
            _ => bail!("bad indent"),
        };
        if options.indent.is_some() {
            options.item_separator = ",".into();
        }
    }
    if let Some(Value::List(seps)) = v.get("separators") {
        let [item, key] = seps.as_slice() else {
            bail!("separators must have two items")
        };
        options.item_separator = item.as_str().context("separator")?.to_owned();
        options.key_separator = key.as_str().context("separator")?.to_owned();
    }
    if let Some(Value::Bool(b)) = v.get("ensure_ascii") {
        options.ensure_ascii = *b;
    }
    if let Some(Value::Bool(b)) = v.get("sort_keys") {
        options.sort_keys = *b;
    }
    Ok(options)
}

pub fn str_arg<'a>(args: &'a Value, name: &str) -> Result<&'a str> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing string argument {name:?}"))
}

pub fn arg<'a>(args: &'a Value, name: &str) -> Result<&'a Value> {
    args.get(name)
        .ok_or_else(|| anyhow!("missing argument {name:?}"))
}
