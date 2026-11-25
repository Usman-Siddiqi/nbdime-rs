//! Function table for `nbdime-rs-dev batch`, used by the parity harness.
//!
//! Each entry takes the request's `args` object and returns a JSON value that the harness
//! compares with what upstream nbdime returns for the same call.

use anyhow::{anyhow, bail, Context, Result};
use nbdime_core::diff_format::{diff_from_value, diff_into_value};
use nbdime_core::diff_utils::{flatten_list_of_string_diff, StrOrLines};
use nbdime_core::diffing::{
    self, snakes::compute_snakes_multilevel, NotebookDiffConfig, Predicate, Targets,
};
use nbdime_core::difflib::SequenceMatcher;
use nbdime_core::json::{self, DumpOptions};
use nbdime_core::merging::{self, MergeArgs, MergeDecision, Strategies};
use nbdime_core::nbformat;
use nbdime_core::patch::patch;
use nbdime_core::prettyprint::{render, PrettyPrintConfig, Printer};
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
            Ok(diff_into_value(diffing::diff_strings_by_char(a, b)))
        }
        "diff_strings_linewise" => {
            let (a, b) = (str_arg(args, "a")?, str_arg(args, "b")?);
            Ok(diff_into_value(diffing::diff_strings_linewise(a, b)))
        }
        "snakes_multilevel_lines" => snakes_multilevel_lines(args),
        "diff_sequence_bruteforce" => {
            let (a, b) = (list_arg(args, "a")?, list_arg(args, "b")?);
            Ok(diff_into_value(
                diffing::sequence::diff_sequence_bruteforce(a, b, |x, y| x.py_eq(y)),
            ))
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
            Ok(diff_into_value(diff))
        }
        "diff_notebooks" => diff_notebooks(args),
        "decide_merge" => decide_merge(args),
        "decide_merge_with_diff" => decide_merge_with_diff(args),
        "apply_decisions" => apply_decisions(args),
        "decide_notebook_merge" => decide_notebook_merge(args),
        "merge_notebooks" => merge_notebooks(args),
        "notebook.compare" => notebook_compare(args),
        "pp.notebook_diff" => pp_notebook_diff(args),
        "pp.merge_decisions" => pp_merge_decisions(args),
        "pp.value" => pp_value(args),
        "git.diff_render" => git_diff_render(args),
        "git.merge_render" => git_merge_render(args),
        _ => bail!("unknown function {name:?}"),
    }
}

/// The notebook differ configured by the `ignores` and `targets` arguments: ignores apply
/// first, in object order, then the targets. Both may be null, like an absent key. `targets` has
/// the keyword arguments of `set_notebook_diff_targets`: absent ones are true and unknown ones
/// are an error.
fn notebook_diff_config(args: &Value) -> Result<NotebookDiffConfig> {
    let mut config = NotebookDiffConfig::new();
    match args.get("ignores") {
        None | Some(Value::Null) => {}
        Some(Value::Object(ignores)) => {
            let ignores: Vec<_> = ignores
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            config.set_ignores(&ignores)?;
        }
        Some(_) => bail!("ignores must be an object"),
    }
    match args.get("targets") {
        None | Some(Value::Null) => {}
        Some(Value::Object(targets)) => {
            const NAMES: [&str; 6] = [
                "sources",
                "outputs",
                "attachments",
                "metadata",
                "identifier",
                "details",
            ];
            if let Some(unknown) = targets.keys().find(|k| !NAMES.contains(&k.as_str())) {
                bail!("unexpected target {unknown:?}");
            }
            let flag = |name: &str| targets.get(name).is_none_or(Value::truthy);
            config.set_targets(&Targets {
                sources: flag("sources"),
                outputs: flag("outputs"),
                attachments: flag("attachments"),
                metadata: flag("metadata"),
                identifier: flag("identifier"),
                details: flag("details"),
            });
        }
        Some(_) => bail!("targets must be an object"),
    }
    Ok(config)
}

/// `{a, b, ignores?, targets?}`.
fn diff_notebooks(args: &Value) -> Result<Value> {
    let config = notebook_diff_config(args)?;
    let diff = diffing::diff_notebooks(arg(args, "a")?, arg(args, "b")?, &config)?;
    Ok(diff_into_value(diff))
}

/// `{strategies?: {path: name}, transients?: [path], fall_back?: name}`.
fn strategies_arg(args: &Value) -> Result<Strategies> {
    let mut strategies = Strategies::new();
    match args.get("strategies") {
        None | Some(Value::Null) => {}
        Some(Value::Object(map)) => {
            for (path, name) in map {
                let name = match name {
                    Value::Null => None,
                    other => Some(other.as_str().context("strategy names must be strings")?),
                };
                strategies.insert(path, name);
            }
        }
        Some(_) => bail!("strategies must be an object"),
    }
    match args.get("transients") {
        None | Some(Value::Null) => {}
        Some(Value::List(paths)) => {
            for path in paths {
                strategies.transients.push(
                    path.as_str()
                        .context("transients must be strings")?
                        .to_owned(),
                );
            }
        }
        Some(_) => bail!("transients must be a list"),
    }
    strategies.fall_back = args
        .get("fall_back")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(strategies)
}

fn decisions_to_value(decisions: &[MergeDecision]) -> Value {
    Value::List(decisions.iter().map(MergeDecision::to_value).collect())
}

/// `{base, local, remote, strategies?, transients?, fall_back?}`.
fn decide_merge(args: &Value) -> Result<Value> {
    let decisions = merging::decide_merge(
        arg(args, "base")?,
        arg(args, "local")?,
        arg(args, "remote")?,
        &strategies_arg(args)?,
    )?;
    Ok(decisions_to_value(&decisions))
}

/// `decide_merge` with the diffs `local_diff` and `remote_diff` given.
fn decide_merge_with_diff(args: &Value) -> Result<Value> {
    let decisions = merging::decide_merge_with_diff(
        arg(args, "base")?,
        arg(args, "local")?,
        arg(args, "remote")?,
        &diff_from_value(arg(args, "local_diff")?)?,
        &diff_from_value(arg(args, "remote_diff")?)?,
        &strategies_arg(args)?,
    )?;
    Ok(decisions_to_value(&decisions))
}

/// `{base, decisions}`.
fn apply_decisions(args: &Value) -> Result<Value> {
    let decisions = list_arg(args, "decisions")?
        .iter()
        .map(MergeDecision::from_value)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(merging::apply_decisions(arg(args, "base")?, &decisions)?)
}

/// `args`: the merge options, null or absent for the defaults; absent keys take the defaults.
fn merge_args(args: &Value) -> Result<Option<MergeArgs>> {
    let given = match args.get("args") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Object(given)) => given,
        Some(_) => bail!("args must be an object"),
    };
    let optional = |name: &str| -> Result<Option<String>> {
        match given.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Str(s)) => Ok(Some(s.clone())),
            Some(_) => bail!("{name} must be a string or null"),
        }
    };
    let defaults = MergeArgs::default();
    Ok(Some(MergeArgs {
        // An explicit null is Python's `None`, not the default.
        merge_strategy: if given.contains_key("merge_strategy") {
            optional("merge_strategy")?
        } else {
            defaults.merge_strategy
        },
        input_strategy: optional("input_strategy")?,
        output_strategy: optional("output_strategy")?,
        ignore_transients: given
            .get("ignore_transients")
            .map_or(defaults.ignore_transients, Value::truthy),
    }))
}

/// `{base, local, remote, args?, ignores?, targets?}`.
fn decide_notebook_merge(args: &Value) -> Result<Value> {
    let decisions = merging::decide_notebook_merge(
        arg(args, "base")?,
        arg(args, "local")?,
        arg(args, "remote")?,
        merge_args(args)?.as_ref(),
        &notebook_diff_config(args)?,
    )?;
    Ok(decisions_to_value(&decisions))
}

/// `decide_notebook_merge`, returning `[merged, decisions]`.
fn merge_notebooks(args: &Value) -> Result<Value> {
    let (merged, decisions) = merging::merge_notebooks(
        arg(args, "base")?,
        arg(args, "local")?,
        arg(args, "remote")?,
        merge_args(args)?.as_ref(),
        &notebook_diff_config(args)?,
    )?;
    Ok(Value::List(vec![merged, decisions_to_value(&decisions)]))
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

/// The pretty-printer options of a request: `include` (the `diff_ignorables`, absent ones true)
/// and the booleans `use_color`, `color_words`, `use_git` and `use_diff`, which must be booleans
/// when given (upstream indexes its colour table with them).
fn pretty_print_config(args: &Value) -> Result<PrettyPrintConfig> {
    let mut config = PrettyPrintConfig::default();
    match args.get("include") {
        None | Some(Value::Null) => {}
        Some(Value::Object(include)) => {
            let flag = |name: &str, default: bool| include.get(name).map_or(default, Value::truthy);
            config.sources = flag("sources", true);
            config.outputs = flag("outputs", true);
            config.attachments = flag("attachments", true);
            config.metadata = flag("metadata", true);
            config.id = flag("id", true);
            config.details = flag("details", true);
        }
        Some(_) => bail!("include must be an object"),
    }
    let flag = |name: &str, default: bool| match args.get(name) {
        None => Ok(default),
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(other) => bail!("{name} must be a boolean, not {other:?}"),
    };
    config.use_color = flag("use_color", true)?;
    config.color_words = flag("color_words", false)?;
    config.use_git = flag("use_git", true)?;
    config.use_diff = flag("use_diff", true)?;
    Ok(config)
}

/// `{afn, bfn, a, di, include?, use_color, color_words, use_git, use_diff}`.
fn pp_notebook_diff(args: &Value) -> Result<Value> {
    let config = pretty_print_config(args)?;
    let diff = diff_from_value(arg(args, "di")?)?;
    let mut out = String::new();
    Printer::new(&config, &mut out).notebook_diff(
        str_arg(args, "afn")?,
        str_arg(args, "bfn")?,
        arg(args, "a")?,
        &diff,
    )?;
    Ok(Value::from(out))
}

/// `{base, decisions, include?, use_color, use_git, use_diff}`.
fn pp_merge_decisions(args: &Value) -> Result<Value> {
    let config = pretty_print_config(args)?;
    let mut out = String::new();
    Printer::new(&config, &mut out)
        .merge_decision_values(arg(args, "base")?, list_arg(args, "decisions")?)?;
    Ok(Value::from(out))
}

/// `{value, path, prefix, include?, use_color}`.
fn pp_value(args: &Value) -> Result<Value> {
    let config = pretty_print_config(args)?;
    let path = match args.get("path") {
        None | Some(Value::Null) => None,
        Some(Value::Str(path)) => Some(path.as_str()),
        Some(_) => bail!("path must be a string or null"),
    };
    let mut out = String::new();
    Printer::new(&config, &mut out).value_at(
        arg(args, "value")?,
        path,
        str_arg(args, "prefix")?,
    )?;
    Ok(Value::from(out))
}

/// `{a, b, use_color, color_words}`.
fn git_diff_render(args: &Value) -> Result<Value> {
    let config = pretty_print_config(args)?;
    Ok(Value::from(render::diff_render_with_git(
        str_arg(args, "a")?,
        str_arg(args, "b")?,
        &config,
    )?))
}

/// `{base, local, remote, strategy}` giving `[merged, status]`.
fn git_merge_render(args: &Value) -> Result<Value> {
    let strategy = match args.get("strategy") {
        None | Some(Value::Null) => None,
        Some(Value::Str(s)) => Some(s.as_str()),
        Some(_) => bail!("strategy must be a string or null"),
    };
    let (merged, status) = render::merge_render_with_git(
        str_arg(args, "base")?,
        str_arg(args, "local")?,
        str_arg(args, "remote")?,
        strategy,
    )?;
    Ok(Value::List(vec![Value::from(merged), Value::from(status)]))
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
    Ok(diff_into_value(flat))
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
