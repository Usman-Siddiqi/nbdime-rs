//! `nbdime_config.json` files: where they are, how they combine, and the defaults they give the
//! command line parsers (`nbdime.config`).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use nbdime_core::json;
use nbdime_core::{Map, Value};

/// Why a configuration could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// A `ValueError` upstream: the command line parsers ignore it and use no configuration.
    Ignored(String),
    /// Any other exception upstream.
    Fatal(String),
}

/// The classes upstream's configuration of an entry point is built from, base class first, with
/// the defaults of their configurable traits (`None` for unset).
type Traits = Vec<(&'static str, Value)>;

fn classes(entrypoint: &str) -> Option<Vec<(&'static str, Traits)>> {
    let ignorables: Traits = vec![
        ("sources", Value::Null),
        ("outputs", Value::Null),
        ("metadata", Value::Null),
        ("id", Value::Null),
        ("attachments", Value::Null),
        ("details", Value::Null),
        ("Ignore", Value::Object(Map::new())),
    ];
    let diffing = vec![("color_words", Value::Bool(false))];
    let merge = vec![
        ("merge_strategy", Value::from("inline")),
        ("input_strategy", Value::Null),
        ("output_strategy", Value::Null),
        ("ignore_transients", Value::Bool(true)),
    ];
    let mut chain = vec![
        ("NbdimeConfigurable", vec![]),
        ("_Ignorables", ignorables),
        ("_Diffing", diffing),
    ];
    match entrypoint {
        "nbdiff" => chain.extend([("Diff", vec![]), ("GitDiff", vec![]), ("NbDiff", vec![])]),
        "nbmerge" => chain.extend([("Merge", merge), ("NbMerge", vec![])]),
        _ => return None,
    }
    Some(chain)
}

/// The class name of an entry point's configurable (the key `--config` prints it under).
pub fn configurable_name(entrypoint: &str) -> Option<&'static str> {
    match entrypoint {
        "nbdiff" => Some("NbDiff"),
        "nbmerge" => Some("NbMerge"),
        _ => None,
    }
}

fn envset(name: &str) -> Option<bool> {
    env::var(name)
        .ok()
        .map(|v| !["no", "n", "false", "off", "0", "0.0"].contains(&v.to_lowercase().as_str()))
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.is_empty())
}

fn home_dir() -> PathBuf {
    let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    fs::canonicalize(&home).unwrap_or(home)
}

/// `jupyter_core.paths.jupyter_config_path()`, in descending priority. The directory of a Python
/// prefix is replaced by `$VIRTUAL_ENV` or `$CONDA_PREFIX` (findings F-029).
pub fn jupyter_config_path() -> Vec<PathBuf> {
    if non_empty_env("JUPYTER_NO_CONFIG").is_some() {
        return Vec::new();
    }
    let mut paths = Vec::new();
    if let Some(list) = non_empty_env("JUPYTER_CONFIG_PATH") {
        paths.extend(
            list.split(':')
                .map(|p| PathBuf::from(p.trim_end_matches('/'))),
        );
    }
    let mut user = vec![non_empty_env("JUPYTER_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".jupyter"))];
    let virtual_env = non_empty_env("VIRTUAL_ENV");
    let prefix = virtual_env
        .clone()
        .or_else(|| non_empty_env("CONDA_PREFIX"));
    if virtual_env.is_none() && env::var_os("PYTHONNOUSERSITE").is_none() {
        user.push(home_dir().join(".local/etc/jupyter"));
    }
    let system = [
        PathBuf::from("/usr/local/etc/jupyter"),
        PathBuf::from("/etc/jupyter"),
    ];
    let env_dirs: Vec<PathBuf> = prefix
        .map(|p| PathBuf::from(p).join("etc/jupyter"))
        .into_iter()
        .filter(|p| !system.contains(p))
        .collect();
    let prefer_env = envset("JUPYTER_PREFER_ENV_PATH").unwrap_or_else(|| {
        virtual_env.is_some()
            || (env::var_os("CONDA_PREFIX").is_some()
                && env::var("CONDA_DEFAULT_ENV").is_ok_and(|e| e != "base"))
    });
    if prefer_env {
        paths.extend(env_dirs);
        paths.extend(user);
    } else {
        paths.extend(user);
        paths.extend(env_dirs);
    }
    paths.extend(system);
    paths
}

/// `recursive_update`: dicts merge, `None` deletes its key unless `include_none`.
fn recursive_update(target: &mut Map, new: &Map, include_none: bool) -> Result<(), ConfigError> {
    for (key, value) in new {
        match value {
            Value::Object(sub) => {
                if !target.contains_key(key) {
                    target.insert(key.clone(), Value::Object(Map::new()));
                }
                let Some(Value::Object(inner)) = target.get_mut(key) else {
                    return Err(ConfigError::Fatal(format!(
                        "cannot merge a dict into the non-dict value of {key:?}"
                    )));
                };
                recursive_update(inner, sub, include_none)?;
                if !include_none && inner.is_empty() {
                    target.shift_remove(key);
                }
            }
            Value::Null if !include_none => {
                target.shift_remove(key);
            }
            other => {
                target.insert(key.clone(), other.clone());
            }
        }
    }
    Ok(())
}

/// The contents of `nbdime_config.json` in `dir`, if there is such a file and it holds a
/// non-empty configuration.
fn load_config_file(dir: &Path) -> Result<Option<Map>, ConfigError> {
    let file = dir.join("nbdime_config.json");
    if !file.is_file() {
        return Ok(None);
    }
    let bytes =
        fs::read(&file).map_err(|e| ConfigError::Fatal(format!("{}: {e}", file.display())))?;
    let text = String::from_utf8(bytes)
        .map_err(|e| ConfigError::Ignored(format!("{}: {e}", file.display())))?;
    let parsed =
        json::parse(&text).map_err(|e| ConfigError::Ignored(format!("{}: {e}", file.display())))?;
    let mut config = match parsed {
        Value::Object(map) => map,
        Value::List(items) if items.is_empty() => return Ok(None),
        Value::List(_) | Value::Str(_) => {
            return Err(ConfigError::Ignored(format!(
                "{}: not a configuration",
                file.display()
            )))
        }
        _ => {
            return Err(ConfigError::Fatal(format!(
                "{}: not a configuration",
                file.display()
            )))
        }
    };
    if let Some(version) = config.shift_remove("version") {
        if !version.py_eq(&Value::from(1)) {
            return Err(ConfigError::Ignored(format!(
                "Unknown version of JSON config file: {version:?}"
            )));
        }
    }
    Ok((!config.is_empty()).then_some(config))
}

/// `build_config(entrypoint, include_none)`: the trait defaults of the entry point's classes with
/// the values from the config files on top, as a flat map (sections already merged).
pub fn build_config(entrypoint: &str, include_none: bool) -> Result<Map, ConfigError> {
    let chain = classes(entrypoint).ok_or_else(|| {
        ConfigError::Ignored(format!(
            "Config for entrypoint name {entrypoint:?} is not defined!"
        ))
    })?;
    let mut dirs = vec![env::current_dir().unwrap_or_default()];
    dirs.extend(jupyter_config_path());
    let mut disk = Map::new();
    for dir in dirs.iter().rev() {
        if let Some(config) = load_config_file(dir)? {
            recursive_update(&mut disk, &config, include_none)?;
        }
    }
    let mut config = Map::new();
    for (class, traits) in chain {
        let defaults: Map = traits.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
        recursive_update(&mut config, &defaults, include_none)?;
        match disk.get(class) {
            Some(Value::Object(section)) => recursive_update(&mut config, section, include_none)?,
            Some(_) => {
                return Err(ConfigError::Fatal(format!(
                    "the config section {class} is not a dict"
                )))
            }
            None => {}
        }
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nbdime_core::json::parse;

    fn map(text: &str) -> Map {
        match parse(text).unwrap() {
            Value::Object(m) => m,
            _ => panic!("not an object"),
        }
    }

    #[test]
    fn recursive_update_merges_and_deletes() {
        let mut target = map(r#"{"a": 1, "b": {"x": 1, "y": 2}, "c": 3}"#);
        let new = map(r#"{"a": null, "b": {"x": null, "z": 5}, "d": {"e": null}}"#);
        recursive_update(&mut target, &new, false).unwrap();
        assert_eq!(
            json::dumps(&Value::Object(target), &json::DumpOptions::compact()),
            r#"{"b":{"y":2,"z":5},"c":3}"#
        );
    }

    #[test]
    fn include_none_keeps_everything() {
        let mut target = Map::new();
        recursive_update(&mut target, &map(r#"{"a": null, "b": {}}"#), true).unwrap();
        assert_eq!(target.len(), 2);
    }

    #[test]
    fn dict_over_scalar_is_fatal() {
        let mut target = map(r#"{"a": 1}"#);
        assert!(matches!(
            recursive_update(&mut target, &map(r#"{"a": {"b": 1}}"#), false),
            Err(ConfigError::Fatal(_))
        ));
    }

    #[test]
    fn defaults_of_the_entry_points() {
        let diff = classes("nbdiff").unwrap();
        assert_eq!(diff.last().unwrap().0, "NbDiff");
        let merge = classes("nbmerge").unwrap();
        assert!(merge.iter().any(|(name, _)| *name == "Merge"));
        assert!(classes("nbdime").is_none());
        assert!(matches!(
            build_config("nbdime", false),
            Err(ConfigError::Ignored(_))
        ));
    }

    #[test]
    fn envset_follows_jupyter_core() {
        env::set_var("NBDIME_RS_TEST_ENVSET", "OFF");
        assert_eq!(envset("NBDIME_RS_TEST_ENVSET"), Some(false));
        env::set_var("NBDIME_RS_TEST_ENVSET", "1");
        assert_eq!(envset("NBDIME_RS_TEST_ENVSET"), Some(true));
        env::remove_var("NBDIME_RS_TEST_ENVSET");
        assert_eq!(envset("NBDIME_RS_TEST_ENVSET"), None);
    }
}
