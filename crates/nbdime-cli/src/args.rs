//! The arguments the nbdime commands share (`nbdime.args`).

use anyhow::{anyhow, bail, Result};
use nbdime_core::diffing::{NotebookDiffConfig, Targets};
use nbdime_core::json::{self, DumpOptions};
use nbdime_core::merging::MergeArgs;
use nbdime_core::prettyprint::PrettyPrintConfig;
use nbdime_core::Value;

use crate::argparse::{Action, ExitRequest, Kind, Namespace, Nargs, ParseError, Parser};
use crate::config::{self, ConfigError};
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
pub fn apply_config(parser: &mut Parser, diff_config: &mut NotebookDiffConfig) -> Result<()> {
    let entrypoint = parser.prog.split(' ').next().unwrap_or_default().to_owned();
    let mut defaults = match config::build_config(&entrypoint, false) {
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
/// that filter revision diffs.
pub type DiffArgs = (Option<String>, Option<String>, Option<Vec<String>>);

/// `resolve_diff_args`: whether the positionals are revisions or files and paths.
pub fn resolve_diff_args(ns: &Namespace) -> Result<DiffArgs> {
    let text = |name: &str| ns.str(name).map(str::to_owned);
    let mut base = text("base");
    let mut remote = text("remote");
    let mut paths: Option<Vec<String>> = match ns.get("paths") {
        Some(Value::List(items)) if !items.is_empty() => Some(
            items
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| anyhow!("bad path"))
                })
                .collect::<Result<_>>()?,
        ),
        _ => None,
    };
    let gitref = |s: &Option<String>| is_gitref(s.as_deref());
    let truthy = |s: &Option<String>| s.as_deref().is_some_and(|s| !s.is_empty());
    if remote.is_none() && paths.is_none() {
        if !gitref(&base)? {
            paths = base.take().map(|b| vec![b]);
            base = Some("HEAD".to_owned());
        }
    } else if paths.is_none() {
        if gitref(&base)? && !gitref(&remote)? {
            paths = remote.take().map(|r| vec![r]);
        }
    } else if truthy(&base) && truthy(&remote) {
        if !gitref(&base)? {
            let mut all = vec![
                base.take().unwrap_or_default(),
                remote.take().unwrap_or_default(),
            ];
            all.extend(paths.take().unwrap_or_default());
            paths = Some(all);
        } else if !gitref(&remote)? {
            let mut all = vec![remote.take().unwrap_or_default()];
            all.extend(paths.take().unwrap_or_default());
            paths = Some(all);
        }
    }
    Ok((base, remote, paths))
}

/// `prettyprint_config_from_args`.
pub fn prettyprint_config(ns: &Namespace) -> PrettyPrintConfig {
    let flag = |name: &str| ns.get(name).is_none_or(Value::truthy);
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

/// What `--config` prints: the keys of the entry point's configuration and their values.
pub fn config_listing(entrypoint: &str) -> Result<String> {
    let name = config::configurable_name(entrypoint)
        .ok_or_else(|| anyhow!("no configuration for {entrypoint:?}"))?;
    let config = config::build_config(entrypoint, true).map_err(|e| anyhow!("{e:?}"))?;
    let mut out = format!("{name}:\n");
    let mut ns = Namespace {
        values: config.clone(),
        log_level: None,
    };
    let _ = process_exclusive_ignorables(&mut ns);
    for (key, value) in &config {
        let shown = match value {
            Value::Null if DIFF_IGNORABLES.contains(&key.as_str()) => format!(
                "<unset, resolves to {}>",
                json::dumps(
                    ns.get(key).unwrap_or(&Value::Null),
                    &DumpOptions::python_default()
                )
            ),
            Value::Object(map) if map.is_empty() => "{}".to_owned(),
            other => json::dumps(other, &DumpOptions::python_default()),
        };
        out.push_str(&format!("  {key}: {shown}\n"));
    }
    Ok(out)
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
        Err(ParseError::Exit(ExitRequest::ConfigHelp)) => match config_listing(entrypoint) {
            Ok(listing) => Parsed::Exit {
                stdout: String::new(),
                stderr: listing,
                code: 1,
            },
            Err(error) => Parsed::Exit {
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
