//! Diffing of notebooks: the differs for outputs and attachments, the configuration that
//! selects them, and the entry point.

use std::collections::BTreeSet;

use crate::diff_format::{Diff, MappingDiffBuilder};
use crate::value::{Map, Value};

use super::config::{DiffConfig, Differ, Predicate};
use super::generic::{diff_values, diff_with_config};
use super::DiffError;

pub(super) const OUTPUTS_PATH: &str = "/cells/*/outputs";
pub(super) const OUTPUT_PATH: &str = "/cells/*/outputs/*";
const CELLS_PATH: &str = "/cells";
const SOURCE_PATH: &str = "/cells/*/source";
const ATTACHMENTS_PATH: &str = "/cells/*/attachments";

/// MIME types whose values are diffed recursively instead of replaced.
const SPLIT_MIMES: [&str; 4] = [
    "text/",
    "image/svg+xml",
    "application/javascript",
    "application/json",
];

/// Which kinds of change the diff reports (`set_notebook_diff_targets`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Targets {
    pub sources: bool,
    pub outputs: bool,
    pub attachments: bool,
    pub metadata: bool,
    pub identifier: bool,
    pub details: bool,
}

impl Default for Targets {
    fn default() -> Self {
        Targets {
            sources: true,
            outputs: true,
            attachments: true,
            metadata: true,
            identifier: true,
            details: true,
        }
    }
}

/// What `set_notebook_diff_ignores` does with one path.
#[derive(Clone)]
enum Ignore {
    All,
    Reset,
    Keys(Vec<Value>),
}

/// The configuration `nbdime.diff_notebooks` runs with: cell and output predicates, the
/// differ of each notebook path, and the ignore flags applied to them.
#[derive(Clone, Debug)]
pub struct NotebookDiffConfig {
    config: DiffConfig,
}

impl Default for NotebookDiffConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// The differ a path has when nothing was set for it.
fn default_differ(path: &str) -> Differ {
    match path {
        CELLS_PATH | OUTPUTS_PATH => Differ::SequenceMultilevel,
        SOURCE_PATH => Differ::StringLines,
        OUTPUT_PATH => Differ::SingleOutputs,
        ATTACHMENTS_PATH => Differ::Attachments,
        _ => Differ::Generic,
    }
}

impl NotebookDiffConfig {
    /// The configuration after `reset_notebook_differ()`.
    pub fn new() -> Self {
        let mut config = DiffConfig::new(vec![Predicate::Eq], Differ::Generic);
        config.set_predicates(
            CELLS_PATH,
            vec![
                Predicate::CellApproximate,
                Predicate::CellModerate,
                Predicate::CellStrict,
                Predicate::CellById,
            ],
        );
        config.set_predicates(
            OUTPUTS_PATH,
            vec![Predicate::OutputApproximate, Predicate::OutputStrict],
        );
        for path in [
            CELLS_PATH,
            SOURCE_PATH,
            OUTPUTS_PATH,
            OUTPUT_PATH,
            ATTACHMENTS_PATH,
        ] {
            config.set_differ(path, default_differ(path));
        }
        // The id is replaced as a whole, so ignoring it has no effect.
        config.set_atomic("/cells/*/id", true);
        NotebookDiffConfig { config }
    }

    pub fn config(&self) -> &DiffConfig {
        &self.config
    }

    /// `set_notebook_diff_ignores`, applied in the given order. `true` ignores the path,
    /// `false` restores its default, and a list ignores those keys of the path's diff.
    /// Any other value is an error.
    pub fn set_ignores(&mut self, ignores: &[(String, Value)]) -> Result<(), DiffError> {
        for (path, rule) in ignores {
            let rule = match rule {
                Value::Bool(true) => Ignore::All,
                Value::Bool(false) => Ignore::Reset,
                Value::List(keys) => Ignore::Keys(keys.clone()),
                other => return Err(DiffError::InvalidIgnore(format!("{path}: {other:?}"))),
            };
            self.apply_ignore(path, rule);
        }
        Ok(())
    }

    fn apply_ignore(&mut self, path: &str, rule: Ignore) {
        let differ = match rule {
            Ignore::All => Differ::Ignore,
            Ignore::Reset => default_differ(path),
            Ignore::Keys(keys) => Differ::IgnoreKeys {
                inner: Box::new(self.config.differ(path).clone()),
                keys,
            },
        };
        self.config.set_differ(path, differ);
    }

    /// `set_notebook_diff_targets`.
    pub fn set_targets(&mut self, targets: &Targets) {
        let details = if targets.details {
            Ignore::Reset
        } else {
            Ignore::Keys(vec![Value::from("execution_count")])
        };
        let flag = |on: bool| if on { Ignore::Reset } else { Ignore::All };
        let rules = [
            (SOURCE_PATH, flag(targets.sources)),
            (OUTPUTS_PATH, flag(targets.outputs)),
            (ATTACHMENTS_PATH, flag(targets.attachments)),
            ("/metadata", flag(targets.metadata)),
            ("/cells/*/id", flag(targets.identifier)),
            ("/cells/*/metadata", flag(targets.metadata)),
            ("/cells/*/outputs/*/metadata", flag(targets.metadata)),
            ("/cells/*", details.clone()),
            (OUTPUT_PATH, details),
        ];
        for (path, rule) in rules {
            self.apply_ignore(path, rule);
        }
    }
}

/// The diff of two notebooks (JSON objects in nbformat 4 form).
pub fn diff_notebooks(
    a: &Value,
    b: &Value,
    config: &NotebookDiffConfig,
) -> Result<Diff, DiffError> {
    if !(a.is_object() && b.is_object()) {
        return Err(DiffError::NotDicts);
    }
    diff_with_config(a, b, "", &config.config)
}

/// Diff of a pair of outputs. The output is diffed without its `data` by the default generic
/// differ and the data as a MIME bundle; outputs of other types are diffed generically with the
/// notebook configuration, starting at the root path.
pub(super) fn diff_single_outputs(
    a: &Value,
    b: &Value,
    path: &str,
    config: &DiffConfig,
) -> Result<Diff, DiffError> {
    if path != OUTPUT_PATH {
        return Err(DiffError::Type(format!(
            "Invalid path for output: {path:?}"
        )));
    }
    let (Some(am), Some(bm)) = (a.as_object(), b.as_object()) else {
        return Err(DiffError::Type("outputs must be objects".into()));
    };
    let (Some(at), Some(bt)) = (am.get("output_type"), bm.get("output_type")) else {
        return Err(DiffError::MissingKey("output_type".into()));
    };
    if !at.py_eq(bt) {
        return Err(DiffError::Type(
            "cannot diff outputs of different types".into(),
        ));
    }
    if !matches!(at.as_str(), Some("execute_result" | "display_data")) {
        return diff_values(a, b, "", config);
    }

    let (Some(adata), Some(bdata)) = (am.get("data"), bm.get("data")) else {
        return Err(DiffError::MissingKey("data".into()));
    };
    let without_data = |m: &Map| {
        let mut m = m.clone();
        m.shift_remove("data");
        Value::Object(m)
    };
    let mut di = MappingDiffBuilder::new();
    for e in diff_values(
        &without_data(am),
        &without_data(bm),
        "",
        &DiffConfig::default(),
    )? {
        di.append(e);
    }
    di.patch("data", diff_mime_bundle(adata, bdata)?);
    Ok(di.validated())
}

fn add_mime_diff(
    key: &str,
    avalue: &Value,
    bvalue: &Value,
    di: &mut MappingDiffBuilder,
) -> Result<(), DiffError> {
    let diffable = matches!(
        (avalue, bvalue),
        (Value::List(_), Value::List(_))
            | (Value::Object(_), Value::Object(_))
            | (Value::Str(_), Value::Str(_))
    );
    // Upstream's generic diff raises for any other pair, even equal ones (F-019).
    if !diffable {
        if !avalue.py_eq(bvalue) {
            di.replace(key, bvalue.clone());
        }
        return Ok(());
    }
    if let (Value::Str(a), Value::Str(b)) = (avalue, bvalue) {
        if a == b {
            return Ok(());
        }
    }
    let mimetype = key.to_lowercase();
    if SPLIT_MIMES.iter().any(|m| mimetype.starts_with(m)) {
        di.patch(
            key,
            diff_values(avalue, bvalue, "", &DiffConfig::default())?,
        );
    } else if !avalue.py_eq(bvalue) {
        di.replace(key, bvalue.clone());
    }
    Ok(())
}

/// Diff of two collections keyed by name: removed keys, changed values (`on_common`) and added
/// keys, each in sorted order.
fn diff_keyed(
    a: &Value,
    b: &Value,
    mut on_common: impl FnMut(&str, &Value, &Value, &mut MappingDiffBuilder) -> Result<(), DiffError>,
) -> Result<Diff, DiffError> {
    let (Some(a), Some(b)) = (a.as_object(), b.as_object()) else {
        return Err(DiffError::NotBundles);
    };
    fn keys(m: &Map) -> BTreeSet<&str> {
        m.keys().map(String::as_str).collect()
    }
    let (akeys, bkeys) = (keys(a), keys(b));
    let mut di = MappingDiffBuilder::new();
    for key in akeys.difference(&bkeys) {
        di.remove(key);
    }
    for key in akeys.intersection(&bkeys) {
        on_common(key, &a[*key], &b[*key], &mut di)?;
    }
    for key in bkeys.difference(&akeys) {
        di.add(key, b[*key].clone());
    }
    Ok(di.validated())
}

/// Diff of two MIME bundles: mimetypes are the keys and `text/*`-like values are diffed
/// recursively with the default configuration.
pub(super) fn diff_mime_bundle(a: &Value, b: &Value) -> Result<Diff, DiffError> {
    diff_keyed(a, b, add_mime_diff)
}

/// Diff of two attachment collections: filenames map to MIME bundles.
pub(super) fn diff_attachments(a: &Value, b: &Value, path: &str) -> Result<Diff, DiffError> {
    if path != ATTACHMENTS_PATH {
        return Err(DiffError::Type(format!(
            "Invalid path for attachment: {path:?}"
        )));
    }
    diff_keyed(a, b, |key, av, bv, di| {
        di.patch(key, diff_mime_bundle(av, bv)?);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_format::diff_to_value;
    use crate::json::{dumps, parse, DumpOptions};

    fn run(a: &str, b: &str, config: &NotebookDiffConfig) -> Result<String, DiffError> {
        let d = diff_notebooks(&parse(a).unwrap(), &parse(b).unwrap(), config)?;
        Ok(dumps(&diff_to_value(&d), &DumpOptions::compact()))
    }

    fn notebook(cells: &str) -> String {
        format!(r#"{{"cells":[{cells}],"metadata":{{"m":1}},"nbformat":4,"nbformat_minor":4}}"#)
    }

    fn code(source: &str, outputs: &str) -> String {
        format!(
            r#"{{"cell_type":"code","execution_count":1,"metadata":{{}},"outputs":[{outputs}],"source":{source:?}}}"#
        )
    }

    #[test]
    fn identical_notebooks_have_no_diff() {
        let nb = notebook(&code("x = 1\n", ""));
        assert_eq!(
            run(&nb, &nb, &NotebookDiffConfig::new()).as_deref(),
            Ok("[]")
        );
    }

    #[test]
    fn inputs_must_be_objects() {
        let nb = parse(&notebook("")).unwrap();
        let list = Value::List(vec![]);
        let config = NotebookDiffConfig::new();
        assert_eq!(
            diff_notebooks(&list, &nb, &config),
            Err(DiffError::NotDicts)
        );
        assert_eq!(
            diff_notebooks(&nb, &list, &config),
            Err(DiffError::NotDicts)
        );
    }

    #[test]
    fn edited_and_inserted_cells_align() {
        let a = notebook(&code("x = 1\ny = 2\n", ""));
        let b = notebook(&format!(
            "{},{}",
            code("new\n", ""),
            code("x = 1\ny = 3\n", "")
        ));
        let d = run(&a, &b, &NotebookDiffConfig::new()).unwrap();
        assert!(d.contains(r#""op":"addrange","key":0"#), "{d}");
        assert!(
            d.contains(r#""op":"patch","key":0,"diff":[{"op":"patch","key":"source""#),
            "{d}"
        );
    }

    #[test]
    fn ignore_rules_apply_in_order() {
        let a = notebook(&code("x\n", ""));
        let b = a.replace(r#""m":1"#, r#""m":2"#);
        let mut config = NotebookDiffConfig::new();
        assert!(run(&a, &b, &config).unwrap().contains("metadata"));
        let rule = |path: &str, v: Value| vec![(path.to_owned(), v)];
        config
            .set_ignores(&rule("/metadata", Value::Bool(true)))
            .unwrap();
        assert_eq!(run(&a, &b, &config).as_deref(), Ok("[]"));
        config
            .set_ignores(&rule("/metadata", Value::Bool(false)))
            .unwrap();
        assert!(run(&a, &b, &config).unwrap().contains("metadata"));
        config
            .set_ignores(&rule("/metadata", Value::List(vec![Value::from("m")])))
            .unwrap();
        assert_eq!(run(&a, &b, &config).as_deref(), Ok("[]"));
        // Wrapping nests: each list filters what the wrapped differ returned.
        let mut nested = NotebookDiffConfig::new();
        let key = |k: &str| Value::List(vec![Value::from(k)]);
        nested
            .set_ignores(&[
                ("/metadata".into(), key("other")),
                ("/metadata".into(), key("m")),
            ])
            .unwrap();
        assert_eq!(run(&a, &b, &nested).as_deref(), Ok("[]"));
        assert_eq!(
            config.set_ignores(&rule("/cells", Value::from("yes"))),
            Err(DiffError::InvalidIgnore("/cells: Str(\"yes\")".into()))
        );
    }

    #[test]
    fn targets_map_to_ignore_rules() {
        let a = notebook(&code("x\n", ""));
        let b = a.replace(r#""execution_count":1"#, r#""execution_count":2"#);
        let mut config = NotebookDiffConfig::new();
        assert!(run(&a, &b, &config).unwrap().contains("execution_count"));
        config.set_targets(&Targets {
            details: false,
            ..Targets::default()
        });
        assert_eq!(run(&a, &b, &config).as_deref(), Ok("[]"));
        // Targets reset what earlier ignores set on the same paths.
        let mut config = NotebookDiffConfig::new();
        config
            .set_ignores(&[("/cells/*/source".into(), Value::Bool(true))])
            .unwrap();
        config.set_targets(&Targets::default());
        let c = a.replace(r#""source":"x\n""#, r#""source":"y\n""#);
        assert!(run(&a, &c, &config).unwrap().contains("source"));
    }

    #[test]
    fn identifier_target_does_not_ignore_cell_ids() {
        let cell = |id: &str| code("x\n", "").replacen('{', &format!(r#"{{"id":"{id}","#), 1);
        let (a, b) = (notebook(&cell("a")), notebook(&cell("b")));
        let mut config = NotebookDiffConfig::new();
        config.set_targets(&Targets {
            identifier: false,
            ..Targets::default()
        });
        assert!(run(&a, &b, &config).unwrap().contains(r#""key":"id""#));
    }

    #[test]
    fn mime_bundle_diff() {
        let d = |a: &str, b: &str| {
            let d = diff_mime_bundle(&parse(a).unwrap(), &parse(b).unwrap())?;
            Ok::<_, DiffError>(dumps(&diff_to_value(&d), &DumpOptions::compact()))
        };
        assert_eq!(
            d(
                r#"{"a/x":1,"image/png":"p","text/plain":"l1\nl2\n"}"#,
                r#"{"image/png":"q","text/plain":"l1\nl3\n","z/y":2}"#
            )
            .unwrap(),
            concat!(
                r#"[{"op":"remove","key":"a/x"},{"op":"replace","key":"image/png","value":"q"},"#,
                r#"{"op":"patch","key":"text/plain","diff":[{"op":"addrange","key":1,"valuelist":["l3\n"]},{"op":"removerange","key":1,"length":1}]},"#,
                r#"{"op":"add","key":"z/y","value":2}]"#
            )
        );
        // F-019: values the generic diff cannot recurse into are replaced when they differ.
        let scalar = |a: &str, b: &str| {
            d(
                &format!(r#"{{"text/plain":{a}}}"#),
                &format!(r#"{{"text/plain":{b}}}"#),
            )
        };
        assert_eq!(scalar("1", "1").unwrap(), "[]");
        assert_eq!(scalar("1", "1.0").unwrap(), "[]");
        assert_eq!(scalar("true", "1").unwrap(), "[]");
        assert_eq!(scalar("null", "null").unwrap(), "[]");
        assert_eq!(
            scalar("1", "2").unwrap(),
            r#"[{"op":"replace","key":"text/plain","value":2}]"#
        );
        assert_eq!(
            scalar(r#""a""#, r#"["a"]"#).unwrap(),
            r#"[{"op":"replace","key":"text/plain","value":["a"]}]"#
        );
        assert_eq!(scalar(r#"["a"]"#, r#"["a"]"#).unwrap(), "[]");
        assert_eq!(
            scalar(r#"{"a":1}"#, r#"["a"]"#)
                .unwrap()
                .matches("replace")
                .count(),
            1
        );
        assert_eq!(scalar("{}", "{}").unwrap(), "[]");
        assert_eq!(
            d(r#"{"Text/Plain":"a"}"#, r#"{"Text/Plain":"a"}"#).unwrap(),
            "[]"
        );
        assert_eq!(d("[]", "{}"), Err(DiffError::NotBundles));
    }

    #[test]
    fn single_outputs_use_the_default_config_for_the_part_without_data() {
        let config = NotebookDiffConfig::new();
        let out = |md: &str, data: &str| {
            parse(&format!(
                r#"{{"output_type":"display_data","metadata":{md},"data":{data}}}"#
            ))
            .unwrap()
        };
        let d = diff_single_outputs(
            &out(r#"{"k":1}"#, r#"{"text/plain":"a"}"#),
            &out(r#"{"k":2}"#, r#"{"text/plain":"b"}"#),
            OUTPUT_PATH,
            config.config(),
        )
        .unwrap();
        let text = dumps(&diff_to_value(&d), &DumpOptions::compact());
        assert!(text.starts_with(r#"[{"op":"patch","key":"data""#), "{text}");
        assert!(text.contains(r#"{"op":"patch","key":"metadata""#), "{text}");
        let stream = parse(r#"{"output_type":"stream"}"#).unwrap();
        assert!(matches!(
            diff_single_outputs(&stream, &out("{}", "{}"), OUTPUT_PATH, config.config()),
            Err(DiffError::Type(_))
        ));
        assert!(diff_single_outputs(&stream, &stream, "/x", config.config()).is_err());
    }

    #[test]
    fn attachments_diff_by_filename() {
        let a = parse(r#"{"a.png":{"image/png":"x"},"b.png":{"image/png":"y"}}"#).unwrap();
        let b = parse(r#"{"b.png":{"image/png":"z"},"c.png":{"image/png":"y"}}"#).unwrap();
        let d = diff_attachments(&a, &b, ATTACHMENTS_PATH).unwrap();
        assert_eq!(
            dumps(&diff_to_value(&d), &DumpOptions::compact()),
            concat!(
                r#"[{"op":"remove","key":"a.png"},"#,
                r#"{"op":"patch","key":"b.png","diff":[{"op":"replace","key":"image/png","value":"z"}]},"#,
                r#"{"op":"add","key":"c.png","value":{"image/png":"y"}}]"#
            )
        );
    }
}
