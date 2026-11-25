//! Decoding of the option arguments that the dev tool and the Python extension share.
//!
//! Options arrive as [`Value`]s (JSON objects or Python dicts) and an absent option is `None` or
//! `Value::Null`, like a missing key.

use crate::diffing::{DiffError, NotebookDiffConfig, Targets};
use crate::merging::{MergeArgs, Strategies};
use crate::Value;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum OptionsError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Diff(#[from] DiffError),
}

fn invalid<T>(msg: impl Into<String>) -> Result<T, OptionsError> {
    Err(OptionsError::Invalid(msg.into()))
}

fn present(v: Option<&Value>) -> Option<&Value> {
    v.filter(|v| !matches!(v, Value::Null))
}

/// The notebook differ configured by `ignores` and `targets`: ignores apply first, in object
/// order, then the targets. `targets` has the keyword arguments of
/// `set_notebook_diff_targets`: absent ones are true and unknown ones are an error.
pub fn notebook_diff_config(
    ignores: Option<&Value>,
    targets: Option<&Value>,
) -> Result<NotebookDiffConfig, OptionsError> {
    let mut config = NotebookDiffConfig::new();
    match present(ignores) {
        None => {}
        Some(Value::Object(ignores)) => {
            let ignores: Vec<_> = ignores
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            config.set_ignores(&ignores)?;
        }
        Some(_) => return invalid("ignores must be an object"),
    }
    match present(targets) {
        None => {}
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
                return invalid(format!("unexpected target {unknown:?}"));
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
        Some(_) => return invalid("targets must be an object"),
    }
    Ok(config)
}

/// The merge strategies of `strategies` (path to strategy name, `null` removing the path),
/// `transients` (a list of paths) and `fall_back`, which is ignored unless it is a string.
pub fn strategies(
    strategies: Option<&Value>,
    transients: Option<&Value>,
    fall_back: Option<&Value>,
) -> Result<Strategies, OptionsError> {
    let mut result = Strategies::new();
    match present(strategies) {
        None => {}
        Some(Value::Object(map)) => {
            for (path, name) in map {
                let name = match name {
                    Value::Null => None,
                    Value::Str(name) => Some(name.as_str()),
                    _ => return invalid("strategy names must be strings"),
                };
                result.insert(path, name);
            }
        }
        Some(_) => return invalid("strategies must be an object"),
    }
    match present(transients) {
        None => {}
        Some(Value::List(paths)) => {
            for path in paths {
                let Value::Str(path) = path else {
                    return invalid("transients must be strings");
                };
                result.transients.push(path.clone());
            }
        }
        Some(_) => return invalid("transients must be a list"),
    }
    result.fall_back = fall_back.and_then(Value::as_str).map(str::to_owned);
    Ok(result)
}

/// The notebook merge options (`nbmerge` arguments): `None` for null or absent, which takes
/// the defaults; absent keys of an object take the defaults too.
pub fn merge_args(args: Option<&Value>) -> Result<Option<MergeArgs>, OptionsError> {
    let given = match present(args) {
        None => return Ok(None),
        Some(Value::Object(given)) => given,
        Some(_) => return invalid("args must be an object"),
    };
    let optional = |name: &str| -> Result<Option<String>, OptionsError> {
        match given.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Str(s)) => Ok(Some(s.clone())),
            Some(_) => invalid(format!("{name} must be a string or null")),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    fn v(text: &str) -> Value {
        parse(text).unwrap()
    }

    #[test]
    fn absent_and_null_options_are_the_same() {
        assert!(notebook_diff_config(None, None).is_ok());
        assert!(notebook_diff_config(Some(&Value::Null), Some(&Value::Null)).is_ok());
        assert_eq!(merge_args(Some(&Value::Null)).unwrap(), None);
        assert_eq!(merge_args(None).unwrap(), None);
    }

    #[test]
    fn targets_reject_unknown_names_and_non_objects() {
        let err = notebook_diff_config(None, Some(&v(r#"{"bogus": true}"#))).unwrap_err();
        assert_eq!(err.to_string(), "unexpected target \"bogus\"");
        let err = notebook_diff_config(None, Some(&v("[1]"))).unwrap_err();
        assert_eq!(err.to_string(), "targets must be an object");
        assert!(notebook_diff_config(None, Some(&v(r#"{"outputs": false}"#))).is_ok());
    }

    #[test]
    fn ignores_must_be_an_object_of_valid_rules() {
        let err = notebook_diff_config(Some(&v("3")), None).unwrap_err();
        assert_eq!(err.to_string(), "ignores must be an object");
        assert!(notebook_diff_config(Some(&v(r#"{"/cells/*/metadata": true}"#)), None).is_ok());
        assert!(notebook_diff_config(Some(&v(r#"{"/cells": 3}"#)), None).is_err());
    }

    #[test]
    fn strategies_collect_paths_transients_and_fall_back() {
        let s = strategies(
            Some(&v(r#"{"/cells": "union", "/metadata": null}"#)),
            Some(&v(r#"["/cells/*/execution_count"]"#)),
            Some(&v(r#""inline""#)),
        )
        .unwrap();
        assert_eq!(s.transients, ["/cells/*/execution_count"]);
        assert_eq!(s.fall_back.as_deref(), Some("inline"));
        // A fall_back that is not a string is dropped.
        assert_eq!(
            strategies(None, None, Some(&v("3"))).unwrap().fall_back,
            None
        );
    }

    #[test]
    fn strategies_reject_wrong_types() {
        let err = strategies(Some(&v(r#"{"/a": 1}"#)), None, None).unwrap_err();
        assert_eq!(err.to_string(), "strategy names must be strings");
        let err = strategies(None, Some(&v(r#"[1]"#)), None).unwrap_err();
        assert_eq!(err.to_string(), "transients must be strings");
        assert!(strategies(Some(&v("[]")), None, None).is_err());
        assert!(strategies(None, Some(&v("{}")), None).is_err());
    }

    #[test]
    fn merge_args_distinguish_null_from_absent() {
        let defaults = MergeArgs::default();
        let given = merge_args(Some(&v("{}"))).unwrap().unwrap();
        assert_eq!(given, defaults);
        let given = merge_args(Some(&v(r#"{"merge_strategy": null}"#)))
            .unwrap()
            .unwrap();
        assert_eq!(given.merge_strategy, None);
        let given = merge_args(Some(&v(
            r#"{"input_strategy": "use-local", "ignore_transients": 0}"#,
        )))
        .unwrap()
        .unwrap();
        assert_eq!(given.input_strategy.as_deref(), Some("use-local"));
        assert!(!given.ignore_transients);
        assert!(merge_args(Some(&v(r#"{"output_strategy": 1}"#))).is_err());
        assert!(merge_args(Some(&v("1"))).is_err());
    }
}
