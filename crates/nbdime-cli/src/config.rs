//! `nbdime_config.json` files: where they are, how they combine, and the defaults they give the
//! command line parsers (`nbdime.config`).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use nbdime_core::json;
use nbdime_core::{Map, Value};

use crate::osstr;
use crate::pathexpand::{expand_path, expanduser, Env, ProcessEnv};

/// The process a command runs in, as far as configuration is concerned: its environment
/// variables and working directory. Tests supply their own.
pub struct Context {
    pub env: Box<dyn Env>,
    pub cwd: PathBuf,
}

impl Context {
    pub fn process() -> Self {
        Context {
            env: Box::new(ProcessEnv),
            cwd: std::env::current_dir().unwrap_or_default(),
        }
    }

    /// A context without any configuration files: `JUPYTER_NO_CONFIG` is set, and `cwd` is where
    /// the files of the test live.
    pub fn without_config(cwd: &Path) -> Self {
        let vars: HashMap<String, String> = [("JUPYTER_NO_CONFIG", "1"), ("HOME", "/nonexistent")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        Context {
            env: Box::new(vars),
            cwd: cwd.to_path_buf(),
        }
    }
}

/// Why a configuration could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// A `ValueError` upstream: the command line parsers ignore it and use no configuration.
    Ignored(String),
    /// Any other exception upstream.
    Fatal(String),
}

type Traits = Vec<(&'static str, Value)>;

/// The configurable traits a class defines itself (`class_own_traits(config=True)`).
fn own_traits(class: &str, cwd: &Path) -> Traits {
    let boolean = |name: &'static str, default: bool| (name, Value::Bool(default));
    match class {
        "_Ignorables" => vec![
            ("sources", Value::Null),
            ("outputs", Value::Null),
            ("metadata", Value::Null),
            ("id", Value::Null),
            ("attachments", Value::Null),
            ("details", Value::Null),
            ("Ignore", Value::Object(Map::new())),
        ],
        "_Diffing" => vec![boolean("color_words", false)],
        "Merge" => vec![
            ("merge_strategy", Value::from("inline")),
            ("input_strategy", Value::Null),
            ("output_strategy", Value::Null),
            boolean("ignore_transients", true),
        ],
        "Web" => vec![
            ("port", Value::from(0)),
            ("ip", Value::from("127.0.0.1")),
            ("base_url", Value::from("/")),
            ("browser", Value::Null),
            boolean("persist", false),
            (
                "workdirectory",
                Value::from(osstr::from_os(cwd.as_os_str())),
            ),
        ],
        "Server" => vec![("port", Value::from(8888))],
        "NbMergeWeb" => vec![boolean("show_base", true)],
        _ => vec![],
    }
}

/// The classes of an entry point's configurable, base class first (reversed MRO), with the
/// class name the configuration file addresses each by.
fn chain_names(entrypoint: &str) -> Option<&'static [&'static str]> {
    const DIFF: [&str; 4] = ["NbdimeConfigurable", "_Ignorables", "_Diffing", "Diff"];
    Some(match entrypoint {
        "nbdiff" => &[DIFF[0], DIFF[1], DIFF[2], DIFF[3], "GitDiff", "NbDiff"],
        "nbdiff-web" => &[
            DIFF[0],
            DIFF[1],
            DIFF[2],
            DIFF[3],
            "GitDiff",
            "Web",
            "NbDiffWeb",
        ],
        "nbmerge" => &[
            "NbdimeConfigurable",
            "_Ignorables",
            "_Diffing",
            "Merge",
            "NbMerge",
        ],
        "nbmerge-web" => &[
            "NbdimeConfigurable",
            "_Ignorables",
            "_Diffing",
            "Merge",
            "Web",
            "NbMergeWeb",
        ],
        "nbshow" => &["NbdimeConfigurable", "_Ignorables", "Show", "NbShow"],
        "server" => &["NbdimeConfigurable", "Web", "Server"],
        "extension" => &[DIFF[0], DIFF[1], DIFF[2], DIFF[3], "GitDiff", "Extension"],
        "git-nbdiffdriver" => &[
            DIFF[0],
            DIFF[1],
            DIFF[2],
            DIFF[3],
            "GitDiff",
            "NbDiffDriver",
        ],
        "git-nbdifftool" => &[
            "NbdimeConfigurable",
            "Web",
            "WebTool",
            "_Ignorables",
            "_Diffing",
            "Diff",
            "GitDiff",
            "NbDiffTool",
        ],
        "git-nbmergedriver" => &[
            "NbdimeConfigurable",
            "_Ignorables",
            "_Diffing",
            "Merge",
            "GitMerge",
            "NbMergeDriver",
        ],
        "git-nbmergetool" => &[
            "NbdimeConfigurable",
            "Web",
            "WebTool",
            "_Ignorables",
            "_Diffing",
            "Merge",
            "GitMerge",
            "NbMergeTool",
        ],
        _ => return None,
    })
}

/// Entry points in the order of `entrypoint_configurables`.
pub const ENTRYPOINTS: [&str; 11] = [
    "nbdiff",
    "nbdiff-web",
    "nbmerge",
    "nbmerge-web",
    "nbshow",
    "server",
    "extension",
    "git-nbdiffdriver",
    "git-nbdifftool",
    "git-nbmergedriver",
    "git-nbmergetool",
];

/// The class name of an entry point's configurable (the key `--config` prints it under).
pub fn configurable_name(entrypoint: &str) -> Option<&'static str> {
    chain_names(entrypoint)?.last().copied()
}

fn envset(env: &dyn Env, name: &str) -> Option<bool> {
    env.get(name)
        .map(|v| !["no", "n", "false", "off", "0", "0.0"].contains(&v.to_lowercase().as_str()))
}

fn non_empty(env: &dyn Env, name: &str) -> Option<String> {
    env.get(name).filter(|v| !v.is_empty())
}

/// `get_home_dir()`: `Path("~").expanduser().resolve()`.
fn home_dir(env: &dyn Env) -> Result<String, ConfigError> {
    let home = expanduser("~", env);
    if home.starts_with('~') {
        return Err(ConfigError::Fatal(
            "Could not determine home directory.".into(),
        ));
    }
    Ok(osstr::from_os(resolve(&osstr::to_path(&home)).as_os_str()))
}

/// `Path.resolve()` (non-strict): symlinks of the part that exists are resolved, the rest is
/// kept, and the result is absolute with `.` and `..` removed.
fn resolve(path: &Path) -> PathBuf {
    use std::path::Component;
    let Ok(absolute) = std::path::absolute(path) else {
        return path.to_path_buf();
    };
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            Component::Normal(name) => {
                resolved.push(name);
                if fs::symlink_metadata(&resolved).is_ok() {
                    if let Ok(canonical) = fs::canonicalize(&resolved) {
                        resolved = canonical;
                    }
                }
            }
            other => resolved.push(other),
        }
    }
    resolved
}

/// `jupyter_core.paths.jupyter_config_path()`, in descending priority. The directory of a Python
/// prefix is replaced by `$VIRTUAL_ENV` or `$CONDA_PREFIX`, and Python's user site directory is
/// not searched (it is off in virtual environments; findings F-029, F-031).
pub fn jupyter_config_path(env: &dyn Env) -> Result<Vec<String>, ConfigError> {
    if non_empty(env, "JUPYTER_NO_CONFIG").is_some() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    if let Some(list) = non_empty(env, "JUPYTER_CONFIG_PATH") {
        paths.extend(list.split(':').map(|p| p.trim_end_matches('/').to_owned()));
    }
    let user = match non_empty(env, "JUPYTER_CONFIG_DIR") {
        Some(dir) => dir,
        None => format!("{}/.jupyter", home_dir(env)?.trim_end_matches('/')),
    };
    let virtual_env = non_empty(env, "VIRTUAL_ENV");
    let prefix = virtual_env
        .clone()
        .or_else(|| non_empty(env, "CONDA_PREFIX"));
    let system = ["/usr/local/etc/jupyter", "/etc/jupyter"];
    let env_dirs: Vec<String> = prefix
        .map(|p| format!("{p}/etc/jupyter"))
        .into_iter()
        .filter(|p| !system.contains(&p.as_str()))
        .collect();
    let prefer_env = envset(env, "JUPYTER_PREFER_ENV_PATH").unwrap_or_else(|| {
        virtual_env.is_some()
            || (env.get("CONDA_PREFIX").is_some()
                && env.get("CONDA_DEFAULT_ENV").is_some_and(|e| e != "base"))
    });
    if prefer_env {
        paths.extend(env_dirs);
        paths.push(user);
    } else {
        paths.push(user);
        paths.extend(env_dirs);
    }
    paths.extend(system.map(str::to_owned));
    Ok(paths)
}

/// `recursive_update`: dicts merge, `None` deletes its key unless `include_none`. A dict meeting
/// a value that is not one fails like the assignments into it do upstream, unless the dict is
/// empty.
fn recursive_update(target: &mut Map, new: &Map, include_none: bool) -> Result<(), ConfigError> {
    for (key, value) in new {
        match value {
            Value::Object(sub) => {
                if !target.contains_key(key) {
                    target.insert(key.clone(), Value::Object(Map::new()));
                }
                match target.get_mut(key) {
                    Some(Value::Object(inner)) => {
                        recursive_update(inner, sub, include_none)?;
                        if !include_none && inner.is_empty() {
                            target.shift_remove(key);
                        }
                    }
                    Some(other) => {
                        if !sub.is_empty() {
                            return Err(ConfigError::Fatal(format!(
                                "cannot merge a dict into the non-dict value of {key:?}"
                            )));
                        }
                        if !include_none && !other.truthy() {
                            target.shift_remove(key);
                        }
                    }
                    None => unreachable!("the key was inserted above"),
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

fn value_error(file: &Path, what: &str) -> ConfigError {
    ConfigError::Ignored(format!("{}: {what}", file.display()))
}

fn type_error(file: &Path, what: &str) -> ConfigError {
    ConfigError::Fatal(format!("{}: {what}", file.display()))
}

/// `Config(dict_like)` for a list: `dict(pairs)` followed by `_ensure_subconfig`.
fn config_from_pairs(file: &Path, items: &[Value]) -> Result<Map, ConfigError> {
    let mut map = Map::new();
    let mut truthy_key = false;
    for (n, item) in items.iter().enumerate() {
        let pair: Vec<Value> = match item {
            Value::List(pair) if pair.len() == 2 => pair.clone(),
            Value::Str(text) if text.chars().count() == 2 => {
                text.chars().map(|c| Value::from(c.to_string())).collect()
            }
            Value::Object(entries) if entries.len() == 2 => {
                entries.keys().map(|k| Value::from(k.as_str())).collect()
            }
            Value::List(_) | Value::Str(_) | Value::Object(_) => {
                return Err(value_error(
                    file,
                    &format!("dictionary update sequence element #{n} has the wrong length"),
                ))
            }
            _ => {
                return Err(type_error(
                    file,
                    &format!(
                        "cannot convert dictionary update sequence element #{n} to a sequence"
                    ),
                ))
            }
        };
        let [key, value] = <[Value; 2]>::try_from(pair).expect("a pair");
        match key {
            Value::Str(key) => {
                map.insert(key, value);
            }
            Value::List(_) | Value::Object(_) => {
                return Err(type_error(file, "unhashable type"));
            }
            other => truthy_key |= other.truthy(),
        }
    }
    // `_is_section_key` indexes a key that is not a string.
    if truthy_key {
        return Err(type_error(file, "object is not subscriptable"));
    }
    Ok(map)
}

/// `JSONFileConfigLoader._convert_to_config`: the configuration a file's JSON stands for.
fn config_from_json(file: &Path, parsed: Value) -> Result<Map, ConfigError> {
    let has_version = match &parsed {
        Value::Object(map) => map.contains_key("version"),
        Value::List(items) => items
            .iter()
            .any(|v| matches!(v, Value::Str(s) if s == "version")),
        Value::Str(text) => text.contains("version"),
        _ => return Err(type_error(file, "argument is not iterable")),
    };
    let mut parsed = parsed;
    let mut version = Value::from(1);
    if has_version {
        match &mut parsed {
            Value::Object(map) => version = map.shift_remove("version").unwrap_or(version),
            _ => return Err(type_error(file, "no pop method")),
        }
    }
    if !version.py_eq(&Value::from(1)) {
        return Err(value_error(
            file,
            &format!("Unknown version of JSON config file: {version:?}"),
        ));
    }
    match parsed {
        Value::Object(map) => Ok(map),
        Value::List(items) => config_from_pairs(file, &items),
        Value::Str(text) if text.is_empty() => Ok(Map::new()),
        Value::Str(_) => Err(value_error(
            file,
            "dictionary update sequence element has length 1",
        )),
        _ => unreachable!("rejected above"),
    }
}

/// The `nbdime_config.json` that `filefind` finds in `dir`: both are expanded like a shell would.
fn find_config_file(env: &dyn Env, dir: &str) -> Option<PathBuf> {
    let joined = if dir.is_empty() || dir.ends_with('/') {
        format!("{dir}nbdime_config.json")
    } else {
        format!("{dir}/nbdime_config.json")
    };
    let path = osstr::to_path(&expand_path(&joined, env));
    path.is_file().then_some(path)
}

/// The contents of `nbdime_config.json` in `dir`, if there is such a file and it holds a
/// non-empty configuration.
fn load_config_file(env: &dyn Env, dir: &str) -> Result<Option<Map>, ConfigError> {
    let Some(file) = find_config_file(env, dir) else {
        return Ok(None);
    };
    let bytes = fs::read(&file).map_err(|e| type_error(&file, &e.to_string()))?;
    let text = String::from_utf8(bytes).map_err(|e| value_error(&file, &e.to_string()))?;
    let parsed = json::parse(&text).map_err(|e| value_error(&file, &e.to_string()))?;
    let config = config_from_json(&file, parsed)?;
    Ok((!config.is_empty()).then_some(config))
}

/// `build_config(entrypoint, include_none)`: the trait defaults of the entry point's classes with
/// the values from the config files on top, as a flat map (sections already merged).
pub fn build_config(
    ctx: &Context,
    entrypoint: &str,
    include_none: bool,
) -> Result<Map, ConfigError> {
    let chain = chain_names(entrypoint).ok_or_else(|| {
        ConfigError::Ignored(format!(
            "Config for entrypoint name {entrypoint:?} is not defined!"
        ))
    })?;
    let mut dirs = vec![osstr::from_os(ctx.cwd.as_os_str())];
    dirs.extend(jupyter_config_path(ctx.env.as_ref())?);
    let mut disk = Map::new();
    for dir in dirs.iter().rev() {
        if let Some(config) = load_config_file(ctx.env.as_ref(), dir)? {
            recursive_update(&mut disk, &config, include_none)?;
        }
    }
    let mut config = Map::new();
    for class in chain {
        let defaults: Map = own_traits(class, &ctx.cwd)
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
        recursive_update(&mut config, &defaults, include_none)?;
        match disk.get(*class) {
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
    use crate::testutil::TempDir;
    use nbdime_core::json::parse;

    #[cfg(unix)]
    #[test]
    fn resolve_is_absolute_even_where_the_path_does_not_exist() {
        let dir = TempDir::new("resolve");
        let root = fs::canonicalize(&dir.0).unwrap();
        std::os::unix::fs::symlink(&root, root.join("link")).unwrap();
        let wanted = root.join("missing/deeper");
        assert_eq!(resolve(&root.join("link/missing/./deeper")), wanted);
        assert_eq!(resolve(&root.join("link/missing/x/../deeper")), wanted);
        assert!(resolve(Path::new("no/such/dir")).is_absolute());
    }

    fn map(text: &str) -> Map {
        match parse(text).unwrap() {
            Value::Object(m) => m,
            _ => panic!("not an object"),
        }
    }

    fn context(cwd: &Path, vars: &[(&str, &str)]) -> Context {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        Context {
            env: Box::new(vars),
            cwd: cwd.to_path_buf(),
        }
    }

    /// A context whose only configuration file is `nbdime_config.json` in `dir`.
    fn with_config(dir: &TempDir, text: &str) -> Context {
        dir.write("nbdime_config.json", text);
        context(
            &dir.0,
            &[("JUPYTER_NO_CONFIG", "1"), ("HOME", "/nonexistent")],
        )
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
    fn dict_over_scalar_is_fatal_unless_it_is_empty() {
        let mut target = map(r#"{"a": 1}"#);
        assert!(matches!(
            recursive_update(&mut target, &map(r#"{"a": {"b": 1}}"#), false),
            Err(ConfigError::Fatal(_))
        ));
        // An empty dict assigns nothing; the falsy value it met is pruned unless None is kept.
        let mut target = map(r#"{"truthy": 1, "falsy": false}"#);
        recursive_update(&mut target, &map(r#"{"truthy": {}, "falsy": {}}"#), false).unwrap();
        assert_eq!(target.keys().collect::<Vec<_>>(), ["truthy"]);
        let mut target = map(r#"{"falsy": false}"#);
        recursive_update(&mut target, &map(r#"{"falsy": {}}"#), true).unwrap();
        assert_eq!(target.len(), 1);
    }

    #[test]
    fn entry_points_have_their_classes() {
        assert_eq!(configurable_name("nbdiff"), Some("NbDiff"));
        assert_eq!(configurable_name("git-nbmergetool"), Some("NbMergeTool"));
        assert_eq!(configurable_name("nbdime"), None);
        assert!(ENTRYPOINTS.iter().all(|e| chain_names(e).is_some()));
        let dir = TempDir::new("config-chains");
        let ctx = context(&dir.0, &[("JUPYTER_NO_CONFIG", "1")]);
        let server = build_config(&ctx, "server", false).unwrap();
        assert_eq!(server.get("port"), Some(&Value::from(8888)));
        let merge = build_config(&ctx, "nbmerge-web", false).unwrap();
        assert_eq!(merge.get("show_base"), Some(&Value::Bool(true)));
        assert_eq!(merge.get("merge_strategy"), Some(&Value::from("inline")));
        assert!(matches!(
            build_config(&ctx, "nbdime", false),
            Err(ConfigError::Ignored(_))
        ));
    }

    #[test]
    fn config_path_order_and_expansion() {
        let env: HashMap<String, String> = [
            ("HOME", "/home/u"),
            ("JUPYTER_CONFIG_PATH", "/a/:/b"),
            ("VIRTUAL_ENV", "/venv"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        assert_eq!(
            jupyter_config_path(&env).unwrap(),
            [
                "/a",
                "/b",
                "/venv/etc/jupyter",
                "/home/u/.jupyter",
                "/usr/local/etc/jupyter",
                "/etc/jupyter"
            ]
        );
        let plain: HashMap<String, String> = [("HOME", "/home/u"), ("JUPYTER_CONFIG_DIR", "~/c")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        // The directory is expanded when the file is looked up, not when the path is built.
        assert_eq!(jupyter_config_path(&plain).unwrap()[0], "~/c");
        let none: HashMap<String, String> = [("JUPYTER_NO_CONFIG", "1".to_owned())]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
        assert!(jupyter_config_path(&none).unwrap().is_empty());
    }

    #[test]
    fn the_user_site_directory_is_not_searched() {
        let env: HashMap<String, String> = [("HOME", "/home/u")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        let path = jupyter_config_path(&env).unwrap();
        assert!(path.iter().all(|p| !p.contains(".local")), "{path:?}");
        let with_base: HashMap<String, String> = [("HOME", "/home/u"), ("PYTHONUSERBASE", "/ub")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        assert!(jupyter_config_path(&with_base)
            .unwrap()
            .iter()
            .all(|p| !p.starts_with("/ub")));
    }

    #[test]
    fn home_unset_uses_the_passwd_entry() {
        let none: HashMap<String, String> = HashMap::new();
        let path = jupyter_config_path(&none).unwrap();
        let home = expanduser("~", &none);
        assert!(!home.starts_with('~'));
        assert!(path[0].ends_with("/.jupyter"), "{path:?}");
    }

    #[test]
    fn config_dirs_are_expanded_like_a_shell() {
        let dir = TempDir::new("config-expand");
        std::fs::create_dir_all(dir.0.join("cfg")).unwrap();
        dir.write(
            "cfg/nbdime_config.json",
            r#"{"NbDiff": {"outputs": false}}"#,
        );
        let home = dir.0.to_str().unwrap().to_owned();
        let vars = [("HOME", home.as_str()), ("JUPYTER_CONFIG_DIR", "~/cfg")];
        let ctx = context(Path::new("/nonexistent"), &vars);
        let config = build_config(&ctx, "nbdiff", false).unwrap();
        assert_eq!(config.get("outputs"), Some(&Value::Bool(false)));
        let vars = [
            ("HOME", home.as_str()),
            ("JUPYTER_CONFIG_PATH", "$HOME/cfg"),
        ];
        let ctx = context(Path::new("/nonexistent"), &vars);
        let config = build_config(&ctx, "nbdiff", false).unwrap();
        assert_eq!(config.get("outputs"), Some(&Value::Bool(false)));
    }

    #[test]
    fn the_working_directory_wins_over_the_config_path() {
        let dir = TempDir::new("config-priority");
        std::fs::create_dir_all(dir.0.join("cfg")).unwrap();
        dir.write(
            "cfg/nbdime_config.json",
            r#"{"NbDiff": {"outputs": true, "sources": false}}"#,
        );
        dir.write("nbdime_config.json", r#"{"NbDiff": {"outputs": false}}"#);
        let cfg = dir.0.join("cfg");
        let ctx = context(&dir.0, &[("JUPYTER_CONFIG_DIR", cfg.to_str().unwrap())]);
        let config = build_config(&ctx, "nbdiff", false).unwrap();
        assert_eq!(config.get("outputs"), Some(&Value::Bool(false)));
        assert_eq!(config.get("sources"), Some(&Value::Bool(false)));
    }

    #[test]
    fn odd_top_level_values() {
        let dir = TempDir::new("config-top");
        let build = |text: &str| build_config(&with_config(&dir, text), "nbdiff", false);
        // An empty string is an empty configuration, a list of pairs is a configuration.
        assert!(build("\"\"").is_ok());
        let pairs = build(r#"[["NbDiff", {"outputs": false}]]"#).unwrap();
        assert_eq!(pairs.get("outputs"), Some(&Value::Bool(false)));
        let chars = build(r#"["ab"]"#).unwrap();
        assert!(!chars.contains_key("a"));
        // `dict(...)` raises a ValueError (ignored) for a wrong length, a TypeError otherwise.
        assert!(matches!(build("[[1]]"), Err(ConfigError::Ignored(_))));
        assert!(matches!(build("\"ab\""), Err(ConfigError::Ignored(_))));
        assert!(matches!(build("[1]"), Err(ConfigError::Fatal(_))));
        assert!(matches!(build("[[[1], 2]]"), Err(ConfigError::Fatal(_))));
        assert!(matches!(build("[[1, 2]]"), Err(ConfigError::Fatal(_))));
        assert!(build("[[0, 2]]").is_ok());
        // `"version" in dictionary` and then `.pop`.
        assert!(matches!(build("\"version\""), Err(ConfigError::Fatal(_))));
        assert!(matches!(
            build(r#"["version"]"#),
            Err(ConfigError::Fatal(_))
        ));
        assert!(matches!(build("5"), Err(ConfigError::Fatal(_))));
        assert!(matches!(build("null"), Err(ConfigError::Fatal(_))));
        assert!(matches!(
            build(r#"{"version": 2}"#),
            Err(ConfigError::Ignored(_))
        ));
        assert!(build(r#"{"version": true, "NbDiff": {"outputs": false}}"#).is_ok());
        assert!(matches!(build("{not json"), Err(ConfigError::Ignored(_))));
    }

    #[test]
    fn sections_must_be_dicts() {
        let dir = TempDir::new("config-sections");
        let ctx = with_config(&dir, r#"{"NbDiff": 5}"#);
        assert!(matches!(
            build_config(&ctx, "nbdiff", false),
            Err(ConfigError::Fatal(_))
        ));
        // An empty dict where a boolean is changes nothing, with or without `None` values.
        let ctx = with_config(&dir, r#"{"NbDiff": {"color_words": {}}}"#);
        for include_none in [false, true] {
            assert_eq!(
                build_config(&ctx, "nbdiff", include_none)
                    .unwrap()
                    .get("color_words"),
                Some(&Value::Bool(false))
            );
        }
        let ctx = with_config(&dir, r#"{"NbDiff": {"color_words": {"a": 1}}}"#);
        assert!(matches!(
            build_config(&ctx, "nbdiff", false),
            Err(ConfigError::Fatal(_))
        ));
    }

    #[test]
    fn a_directory_called_nbdime_config_json_is_not_a_config_file() {
        let dir = TempDir::new("config-dir");
        std::fs::create_dir_all(dir.0.join("nbdime_config.json")).unwrap();
        let ctx = context(&dir.0, &[("JUPYTER_NO_CONFIG", "1")]);
        assert!(build_config(&ctx, "nbdiff", false).is_ok());
    }

    #[test]
    fn envset_follows_jupyter_core() {
        let mut vars: HashMap<String, String> = HashMap::new();
        assert_eq!(envset(&vars, "X"), None);
        vars.insert("X".into(), "OFF".into());
        assert_eq!(envset(&vars, "X"), Some(false));
        vars.insert("X".into(), "1".into());
        assert_eq!(envset(&vars, "X"), Some(true));
    }
}
