//! The arguments the nbdime commands share (`nbdime.args`).

use anyhow::{anyhow, bail, Result};
use nbdime_core::diffing::{NotebookDiffConfig, Targets};
use nbdime_core::json::{self, DumpOptions};
use nbdime_core::merging::MergeArgs;
use nbdime_core::prettyprint::{PrettyPrintConfig, Printer};
use nbdime_core::{Map, Value};

use crate::argparse::{Action, ExitRequest, Kind, Namespace, Nargs, ParseError, Parser};
use crate::config::{self, ConfigError, Context};
use crate::gitfiles::is_gitref;

/// `diff_ignorables`.
pub const DIFF_IGNORABLES: [&str; 6] = [
    "sources",
    "outputs",
    "attachments",
    "metadata",
    "id",
    "details",
];

const LOG_LEVELS: [&str; 5] = ["DEBUG", "INFO", "WARN", "ERROR", "CRITICAL"];

/// The merge strategies `nbmerge` accepts (`cli_conflict_strategies`).
const CONFLICT_STRATEGIES: [&str; 4] = ["inline", "use-base", "use-local", "use-remote"];
const OUTPUT_STRATEGIES: [&str; 6] = [
    "inline",
    "use-base",
    "use-local",
    "use-remote",
    "remove",
    "clear-all",
];

pub fn add_generic_args(parser: &mut Parser) {
    parser.add(
        Action::flag(Kind::Version, &["--version"], "version", Value::Null)
            .help("show program's version number and exit"),
    );
    parser.add(
        Action::flag(Kind::ConfigHelp, &["--config"], "config", Value::Null)
            .help("list the valid config keys and their current effective values"),
    );
    parser.add(
        Action::store(&["--log-level"], "log_level", Value::from("INFO"))
            .choices(&LOG_LEVELS)
            .help("set the log level by name."),
    );
}

pub fn add_diff_args(parser: &mut Parser) {
    for (short_and_long, dest, help) in [
        (["-s", "--sources"], "sources", "process/ignore sources."),
        (["-o", "--outputs"], "outputs", "process/ignore outputs."),
        (
            ["-a", "--attachments"],
            "attachments",
            "process/ignore attachments.",
        ),
        (["-m", "--metadata"], "metadata", "process/ignore metadata."),
        (["-i", "--id"], "id", "process/ignore identifiers."),
        (
            ["-d", "--details"],
            "details",
            "process/ignore details not covered by other options.",
        ),
    ] {
        parser.add(Action::ignorable(&short_and_long, dest).help(help));
    }
}

pub fn add_diff_cli_args(parser: &mut Parser) {
    parser.add(
        Action::flag(
            Kind::StoreTrue,
            &["--color-words"],
            "color_words",
            Value::Bool(false),
        )
        .help("whether to pass the --color-words flag to any internal calls to git diff"),
    );
}

pub fn add_prettyprint_args(parser: &mut Parser) {
    for (option, dest, help) in [
        (
            "--no-color",
            "use_color",
            "prevent use of ANSI color code escapes for text output",
        ),
        (
            "--no-git",
            "use_git",
            "prevent use of git for formatting diff/merge text output",
        ),
        (
            "--no-use-diff",
            "use_diff",
            "prevent use of diff/diff3 for formatting diff/merge text output",
        ),
    ] {
        parser.add(Action::flag(Kind::StoreFalse, &[option], dest, Value::Bool(true)).help(help));
    }
}

pub fn add_merge_args(parser: &mut Parser) {
    parser.add(
        Action::store(
            &["--merge-strategy"],
            "merge_strategy",
            Value::from("inline"),
        )
        .choices(&CONFLICT_STRATEGIES)
        .help("the merge strategy to use."),
    );
    parser.add(
        Action::store(&["--input-strategy"], "input_strategy", Value::Null)
            .choices(&CONFLICT_STRATEGIES)
            .help("the merge strategy to use for inputs (overrides 'merge-strategy' for inputs)."),
    );
    parser.add(
        Action::store(&["--output-strategy"], "output_strategy", Value::Null)
            .choices(&OUTPUT_STRATEGIES)
            .help(
                "the merge strategy to use for outputs (overrides 'merge-strategy' for outputs).",
            ),
    );
    parser.add(
        Action::flag(
            Kind::StoreFalse,
            &["--no-ignore-transients"],
            "ignore_transients",
            Value::Bool(true),
        )
        .help("disallow deletion of transient data such as outputs and execution counts in order to resolve conflicts."),
    );
}

pub fn add_base_args_for_diff(parser: &mut Parser) {
    parser.add(
        Action::positional("base", Nargs::Optional, Value::from("HEAD"))
            .help("the base notebook filename OR base git-revision."),
    );
    parser.add(
        Action::positional("remote", Nargs::Optional, Value::Null)
            .help("the remote modified notebook filename OR remote git-revision."),
    );
    parser.add(
        Action::positional("paths", Nargs::ZeroOrMore, Value::Null)
            .help("filter diffs for git-revisions based on path"),
    );
}

/// What `ConfigBackedParser.parse_known_args` does before parsing: the config files give the
/// parser its defaults and the notebook differ its ignores.
pub fn apply_config(
    ctx: &Context,
    parser: &mut Parser,
    diff_config: &mut NotebookDiffConfig,
) -> Result<()> {
    let entrypoint = parser.prog.split(' ').next().unwrap_or_default().to_owned();
    let mut defaults = match config::build_config(ctx, &entrypoint, false) {
        Ok(defaults) => defaults,
        Err(ConfigError::Ignored(_)) => return Ok(()),
        Err(ConfigError::Fatal(message)) => bail!("{message}"),
    };
    let ignore = defaults.shift_remove("Ignore");
    parser.set_defaults(defaults);
    match ignore {
        None => {}
        Some(value) if !value.truthy() => {}
        Some(Value::Object(ignores)) => {
            let ignores: Vec<(String, Value)> = ignores.into_iter().collect();
            // An invalid entry is a `ValueError` upstream, which the parser ignores.
            let _ = diff_config.set_ignores(&ignores);
        }
        Some(other) => bail!("invalid Ignore configuration: {other:?}"),
    }
    Ok(())
}

/// `process_exclusive_ignorables`: all given ignorables must be positive or all negative; the
/// others get the opposite. Returns whether any was given.
pub fn process_exclusive_ignorables(ns: &mut Namespace) -> Result<bool> {
    let value = |ns: &Namespace, name: &str| ns.get(name).cloned().unwrap_or(Value::Null);
    let mut toggle = value(ns, DIFF_IGNORABLES[0]);
    for name in &DIFF_IGNORABLES[1..] {
        let opt = value(ns, name);
        if toggle == Value::Null {
            toggle = opt;
        } else if !toggle.py_eq(&opt) && opt != Value::Null {
            bail!("Arguments must either all be negative or all positive: {DIFF_IGNORABLES:?}");
        }
    }
    let default = if toggle == Value::Null {
        true
    } else {
        !toggle.truthy()
    };
    for name in DIFF_IGNORABLES {
        if value(ns, name) == Value::Null {
            ns.values.insert(name.to_owned(), Value::Bool(default));
        }
    }
    Ok(toggle != Value::Null)
}

/// `process_diff_flags`: flags given on the command line replace what the config set.
pub fn process_diff_flags(ns: &mut Namespace, diff_config: &mut NotebookDiffConfig) -> Result<()> {
    if process_exclusive_ignorables(ns)? {
        let flag = |name: &str| ns.truthy(name);
        diff_config.set_targets(&Targets {
            sources: flag("sources"),
            outputs: flag("outputs"),
            attachments: flag("attachments"),
            metadata: flag("metadata"),
            identifier: flag("id"),
            details: flag("details"),
        });
    }
    Ok(())
}

/// The base, remote and paths of an `nbdiff` command line: a revision or file, and the paths
/// that filter revision diffs. The paths are whatever the namespace held (a string or a list,
/// but a configuration file can put anything there).
pub type DiffArgs = (Option<String>, Option<String>, Option<Value>);

/// `resolve_diff_args`: whether the positionals are revisions or files and paths.
pub fn resolve_diff_args(ns: &Namespace) -> Result<DiffArgs> {
    let text = |name: &str| -> Result<Option<String>> {
        match ns.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Str(s)) => Ok(Some(s.clone())),
            Some(other) => bail!("{name} must be a file name or revision, not {other:?}"),
        }
    };
    let mut base = text("base")?;
    let mut remote = text("remote")?;
    let mut paths: Option<Value> = ns.get("paths").filter(|v| v.truthy()).cloned();
    let gitref = |s: &Option<String>| is_gitref(s.as_deref());
    let truthy = |s: &Option<String>| s.as_deref().is_some_and(|s| !s.is_empty());
    let prepend = |first: Vec<String>, paths: Option<Value>| -> Result<Option<Value>> {
        match paths {
            Some(Value::List(rest)) => Ok(Some(Value::List(
                first.into_iter().map(Value::from).chain(rest).collect(),
            ))),
            other => bail!("can only concatenate list (not {other:?}) to list"),
        }
    };
    if remote.is_none() && paths.is_none() {
        if !gitref(&base)? {
            paths = base.take().map(Value::from);
            base = Some("HEAD".to_owned());
        }
    } else if paths.is_none() {
        if gitref(&base)? && !gitref(&remote)? {
            paths = remote.take().map(Value::from);
        }
    } else if truthy(&base) && truthy(&remote) {
        if !gitref(&base)? {
            let first = vec![
                base.take().unwrap_or_default(),
                remote.take().unwrap_or_default(),
            ];
            paths = prepend(first, paths.take())?;
        } else if !gitref(&remote)? {
            paths = prepend(vec![remote.take().unwrap_or_default()], paths.take())?;
        }
    }
    Ok((base, remote, paths))
}

/// The pathspecs of `changed_notebooks`: a single string or a list of strings.
pub fn pathspecs(paths: Option<&Value>) -> Result<Option<Vec<String>>> {
    match paths {
        None => Ok(None),
        Some(Value::Str(path)) => Ok(Some(vec![path.clone()])),
        Some(Value::List(items)) => items
            .iter()
            .map(|item| match item {
                Value::Str(path) => Ok(path.clone()),
                other => bail!("path must be a string, not {other:?}"),
            })
            .collect::<Result<Vec<_>>>()
            .map(Some),
        Some(other) => bail!("paths must be a string or a list, not {other:?}"),
    }
}

/// `prettyprint_config_from_args`.
pub fn prettyprint_config(ns: &Namespace) -> PrettyPrintConfig {
    let flag = |name: &str| ns.get(name).is_none_or(Value::truthy);
    // `col_const[use_color]` accepts what equals True or False.
    let invalid_color = !ns.get("use_color").is_none_or(|v| {
        matches!(v, Value::Bool(_)) || v.py_eq(&Value::Bool(true)) || v.py_eq(&Value::Bool(false))
    });
    PrettyPrintConfig {
        sources: flag("sources"),
        outputs: flag("outputs"),
        attachments: flag("attachments"),
        metadata: flag("metadata"),
        id: flag("id"),
        details: flag("details"),
        color_words: ns.truthy("color_words"),
        use_git: flag("use_git"),
        use_diff: flag("use_diff"),
        use_color: flag("use_color"),
        invalid_color,
    }
}

/// The merge options of the namespace.
pub fn merge_args(ns: &Namespace) -> Result<MergeArgs> {
    let strategy = |name: &str| -> Result<Option<String>> {
        match ns.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Str(s)) => Ok(Some(s.clone())),
            Some(other) => bail!("{name} must be a string, not {other:?}"),
        }
    };
    Ok(MergeArgs {
        merge_strategy: strategy("merge_strategy")?,
        input_strategy: strategy("input_strategy")?,
        output_strategy: strategy("output_strategy")?,
        ignore_transients: ns.get("ignore_transients").is_none_or(Value::truthy),
    })
}

/// Why `--config` could not print.
#[derive(Debug)]
pub enum ListingError {
    /// An `ArgumentError`, which `argparse` turns into a usage error.
    Usage(String),
    Other(anyhow::Error),
}

/// `modify_config_for_print`: the values of a configuration as the text `--config` shows.
fn modify_config_for_print(config: &Map) -> Result<Map, ListingError> {
    let mut output = Map::new();
    let mut resolved: Option<Namespace> = None;
    for (key, value) in config {
        let shown = match value {
            Value::Object(sub) => {
                let inner = modify_config_for_print(sub)?;
                if inner.is_empty() {
                    Value::from("{}")
                } else {
                    Value::Object(inner)
                }
            }
            Value::Null if DIFF_IGNORABLES.contains(&key.as_str()) => {
                if resolved.is_none() {
                    let mut ns = Namespace::default();
                    for name in DIFF_IGNORABLES {
                        let value = config.get(name).cloned().unwrap_or(Value::Null);
                        ns.values.insert(name.to_owned(), value);
                    }
                    process_exclusive_ignorables(&mut ns)
                        .map_err(|e| ListingError::Usage(format!("{e:#}")))?;
                    resolved = Some(ns);
                }
                let value = resolved
                    .as_ref()
                    .and_then(|ns| ns.get(key))
                    .unwrap_or(&Value::Null);
                Value::from(format!(
                    "<unset, resolves to {}>",
                    json::dumps(value, &DumpOptions::python_default())
                ))
            }
            other => Value::from(json::dumps(other, &DumpOptions::python_default())),
        };
        output.insert(key.clone(), shown);
    }
    Ok(output)
}

/// What `--config` prints for an entry point: its configuration class and the keys and effective
/// values of its configuration.
pub fn config_listing(ctx: &Context, entrypoint: &str) -> Result<String, ListingError> {
    let name = config::configurable_name(entrypoint)
        .ok_or_else(|| ListingError::Other(anyhow!("no configuration for {entrypoint:?}")))?;
    let config = config::build_config(ctx, entrypoint, true)
        .map_err(|e| ListingError::Other(anyhow!("{e:?}")))?;
    let mut section = Map::new();
    section.insert(
        name.to_owned(),
        Value::Object(modify_config_for_print(&config)?),
    );
    let mut text = String::new();
    Printer::new(&PrettyPrintConfig::default(), &mut text)
        .dict(&section, &[], "")
        .map_err(|e| ListingError::Other(e.into()))?;
    Ok(text)
}

/// Maps a parser result onto what the program does: usage errors exit with status 2.
pub enum Parsed {
    Namespace(Namespace),
    /// Text for stdout and stderr and the exit status.
    Exit {
        stdout: String,
        stderr: String,
        code: i32,
    },
}

pub fn parse_command_line(
    ctx: &Context,
    parser: &Parser,
    args: &[String],
    version: &str,
    entrypoint: &str,
) -> Parsed {
    match parser.parse_args(args) {
        Ok(ns) => Parsed::Namespace(ns),
        Err(ParseError::Usage(message)) => Parsed::Exit {
            stdout: String::new(),
            stderr: parser.error_text(&message),
            code: 2,
        },
        Err(ParseError::Exit(ExitRequest::Help)) => Parsed::Exit {
            stdout: parser.help_text(),
            stderr: String::new(),
            code: 0,
        },
        Err(ParseError::Exit(ExitRequest::Version)) => Parsed::Exit {
            stdout: format!("{} {version}\n", parser.prog),
            stderr: String::new(),
            code: 0,
        },
        Err(ParseError::Exit(ExitRequest::ConfigHelp)) => match config_listing(ctx, entrypoint) {
            Ok(listing) => Parsed::Exit {
                stdout: String::new(),
                stderr: listing,
                code: 1,
            },
            Err(ListingError::Usage(message)) => Parsed::Exit {
                stdout: String::new(),
                stderr: parser.error_text(&message),
                code: 2,
            },
            Err(ListingError::Other(error)) => Parsed::Exit {
                stdout: String::new(),
                stderr: format!("{error:#}\n"),
                code: 1,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ns(pairs: &[(&str, Value)]) -> Namespace {
        let mut ns = Namespace::default();
        for (k, v) in pairs {
            ns.values.insert((*k).to_owned(), v.clone());
        }
        ns
    }

    #[test]
    fn no_flags_means_everything() {
        let mut n = ns(&[]);
        assert!(!process_exclusive_ignorables(&mut n).unwrap());
        assert!(DIFF_IGNORABLES.iter().all(|k| n.truthy(k)));
    }

    #[test]
    fn positive_flags_exclude_the_rest() {
        let mut n = ns(&[
            ("sources", Value::Bool(true)),
            ("outputs", Value::Bool(true)),
        ]);
        assert!(process_exclusive_ignorables(&mut n).unwrap());
        assert!(n.truthy("sources") && !n.truthy("metadata") && !n.truthy("details"));
    }

    #[test]
    fn negative_flags_keep_the_rest() {
        let mut n = ns(&[("metadata", Value::Bool(false))]);
        assert!(process_exclusive_ignorables(&mut n).unwrap());
        assert!(!n.truthy("metadata") && n.truthy("sources"));
    }

    #[test]
    fn mixed_flags_are_an_error() {
        let mut n = ns(&[
            ("sources", Value::Bool(true)),
            ("outputs", Value::Bool(false)),
        ]);
        assert!(process_exclusive_ignorables(&mut n).is_err());
    }

    #[test]
    fn python_equality_between_flag_values() {
        // 1 == True, so these agree.
        let mut n = ns(&[("sources", Value::from(1)), ("outputs", Value::Bool(true))]);
        assert!(process_exclusive_ignorables(&mut n).is_ok());
    }

    #[test]
    fn merge_args_from_namespace() {
        let n = ns(&[
            ("merge_strategy", Value::from("use-local")),
            ("input_strategy", Value::Null),
            ("ignore_transients", Value::Bool(false)),
        ]);
        let args = merge_args(&n).unwrap();
        assert_eq!(args.merge_strategy.as_deref(), Some("use-local"));
        assert!(!args.ignore_transients);
        assert!(merge_args(&ns(&[("merge_strategy", Value::from(3))])).is_err());
    }

    #[test]
    fn parser_wiring() {
        let mut parser = Parser::new("nbdiff", "d");
        add_generic_args(&mut parser);
        add_diff_args(&mut parser);
        add_diff_cli_args(&mut parser);
        add_prettyprint_args(&mut parser);
        add_base_args_for_diff(&mut parser);
        let args: Vec<String> = ["-s", "--no-color", "--color-words", "a", "b"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let n = parser.parse_args(&args).unwrap();
        assert!(n.truthy("sources") && !n.truthy("use_color") && n.truthy("color_words"));
        let config = prettyprint_config(&{
            let mut n = n.clone();
            process_exclusive_ignorables(&mut n).unwrap();
            n
        });
        assert!(config.sources && !config.outputs && !config.use_color);
    }
}
