//! The `pretty_print_*` functions of upstream's `prettyprint.py`.

use crate::diff_format::{DiffEntry, Key, Op};
use crate::merging::decisions::{diffs_eq, Action, MergeDecision};
use crate::patch::patch;
use crate::path::{join_path, star_path_str};
use crate::py;
use crate::value::{Map, PyType, Value};

use super::highlight::colorize_markdown;
use super::pformat::pformat;
use super::render::diff_render;
use super::{
    file_timestamp, format_value, invalid, trim_base64, PrettyPrintConfig, PrettyPrintError, IND,
    MAXWIDTH, MAX_DEPTH,
};

type Result<T> = std::result::Result<T, PrettyPrintError>;

/// Writes pretty-printed notebook diffs, merge decisions and values to `out`.
///
/// Nothing is written that upstream would not write, and when printing fails what was written
/// before the failure stays in `out`, like the stream upstream writes to.
pub struct Printer<'a> {
    config: &'a PrettyPrintConfig,
    out: &'a mut String,
}

const NOTEBOOK_DIFF_ENTRY_END: &str = "\n";

impl<'a> Printer<'a> {
    pub fn new(config: &'a PrettyPrintConfig, out: &'a mut String) -> Self {
        Printer { config, out }
    }

    fn write(&mut self, text: &str) {
        self.out.push_str(text);
    }

    pub fn key(&mut self, k: &str, prefix: &str) {
        self.write(&format!("{prefix}{k}:\n"));
    }

    pub fn key_value(&mut self, k: &str, v: &str, prefix: &str) {
        self.write(&format!("{prefix}{k}: {v}\n"));
    }

    pub fn diff_action(&mut self, msg: &str, path: &str) {
        let config = self.config;
        self.write(&format!(
            "{}{msg} {path}:{}\n",
            config.info(),
            config.reset()
        ));
    }

    /// `pretty_print_value`: a possibly complex value with all lines prefixed.
    pub fn value(&mut self, value: &Value, prefix: &str) -> Result<()> {
        match value {
            Value::Object(map) => self.dict(map, &[], prefix),
            Value::List(items) if !items.is_empty() => self.list(value, prefix),
            _ => {
                let text = format_value(value)?;
                self.multiline(&text, prefix);
                Ok(())
            }
        }
    }

    /// `pretty_print_value_at`: a value that sits at `path`, with the formatter for the cells,
    /// outputs and attachments that the path names.
    pub fn value_at(&mut self, value: &Value, path: Option<&str>, prefix: &str) -> Result<()> {
        let starred = match path {
            None => None,
            Some(path) => {
                let trail = if path.starts_with('/') {
                    path
                } else {
                    match path.split_once('/') {
                        Some((_, trail)) => trail,
                        None => return invalid(format!("path {path:?} has no '/' to split at")),
                    }
                };
                Some(star_path_str(trail))
            }
        };
        match starred.as_deref() {
            Some("/cells/*") => self.cell(None, value, prefix, true),
            Some("/cells") => {
                for cell in iterate(value)? {
                    self.cell(None, cell, prefix, true)?;
                }
                Ok(())
            }
            Some("/cells/*/outputs/*") => self.output(None, value, prefix),
            Some("/cells/*/outputs") => {
                for output in iterate(value)? {
                    self.output(None, output, prefix)?;
                }
                Ok(())
            }
            Some("/cells/*/attachments") => self.attachments(value, prefix),
            _ => self.value(value, prefix),
        }
    }

    pub fn item(&mut self, k: &str, v: &Value, prefix: &str) -> Result<()> {
        let nested = format!("{prefix}{IND}");
        match v {
            Value::Object(map) => {
                self.key(k, prefix);
                self.dict(map, &[], &nested)
            }
            Value::List(_) => {
                self.key(k, prefix);
                self.list(v, &nested)
            }
            _ => {
                let vstr = format_value(v)?;
                if vstr.contains('\n') {
                    self.key(k, prefix);
                    for line in py::splitlines(&vstr) {
                        self.write(&format!("{nested}{line}\n"));
                    }
                } else {
                    self.key_value(k, &vstr, prefix);
                }
                Ok(())
            }
        }
    }

    /// `pretty_print_multiline`: `text` with every line prefixed, ending in a newline.
    pub fn multiline(&mut self, text: &str, prefix: &str) {
        for line in py::splitlines_keepends(text) {
            self.write(prefix);
            self.write(line);
        }
        if !text.ends_with('\n') {
            self.write("\n");
        }
    }

    /// `pretty_print_list` of the list `li`.
    pub fn list(&mut self, li: &Value, prefix: &str) -> Result<()> {
        let Value::List(items) = li else {
            return invalid("expected a list");
        };
        if li.nesting_exceeds(MAX_DEPTH) {
            return Err(PrettyPrintError::TooDeep);
        }
        let formatted = pformat(li);
        let room = MAXWIDTH as i64 - py::py_len(prefix) as i64;
        if (py::py_len(&formatted) as i64) < room && !formatted.contains("\\n") {
            self.write(&format!("{prefix}{formatted}\n"));
        } else {
            for (k, v) in items.iter().enumerate() {
                self.item(&format!("item[{k}]"), v, prefix)?;
            }
        }
        Ok(())
    }

    /// `pretty_print_dict`: the items of `d` without the keys in `exclude`, in key order, as
    ///
    /// ```text
    /// key: value
    /// key:
    ///   long
    ///   value
    /// ```
    pub fn dict(&mut self, d: &Map, exclude: &[&str], prefix: &str) -> Result<()> {
        let entries = d
            .iter()
            .filter(|(k, _)| !exclude.contains(&k.as_str()))
            .collect();
        self.entries(entries, prefix)
    }

    fn entries(&mut self, mut entries: Vec<(&String, &Value)>, prefix: &str) -> Result<()> {
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (k, v) in entries {
            self.item(k, v, prefix)?;
        }
        Ok(())
    }

    /// `pretty_print_metadata`: the keys in `known_keys` first, then the others.
    pub fn metadata(&mut self, md: &Value, known_keys: &[&str], prefix: &str) -> Result<()> {
        let Value::Object(map) = md else {
            return invalid("metadata must be a dict");
        };
        let (known, unknown): (Vec<_>, Vec<_>) = map
            .iter()
            .partition(|(k, _)| known_keys.contains(&k.as_str()));
        let nested = format!("{prefix}{IND}");
        if !known.is_empty() {
            self.key("metadata (known keys)", prefix);
            self.entries(known, &nested)?;
        }
        if !unknown.is_empty() {
            self.key("metadata (unknown keys)", prefix);
            self.entries(unknown, &nested)?;
        }
        Ok(())
    }

    /// `pretty_print_output`: output number `i`, or just "output" without one.
    pub fn output(&mut self, i: Option<usize>, output: &Value, prefix: &str) -> Result<()> {
        let Value::Object(map) = output else {
            return invalid("an output must be a dict");
        };
        let nested = format!("{prefix}{IND}");
        let name = match i {
            None => "output".to_owned(),
            Some(i) => format!("output {i}"),
        };
        self.key(&name, prefix);

        const ITEM_KEYS: [&str; 8] = [
            "output_type",
            "execution_count",
            "name",
            "text",
            "data",
            "ename",
            "evalue",
            "traceback",
        ];
        for k in ITEM_KEYS {
            if let Some(v) = map.get(k).filter(|v| v.truthy()) {
                self.item(k, v, &nested)?;
            }
        }
        if let Some(metadata) = map.get("metadata").filter(|v| v.truthy()) {
            self.metadata(metadata, &["isolated"], &nested)?;
        }
        let exclude: Vec<&str> = ["metadata"].into_iter().chain(ITEM_KEYS).collect();
        self.dict(map, &exclude, &nested)
    }

    pub fn outputs(&mut self, outputs: &[Value], prefix: &str) -> Result<()> {
        self.key("outputs", prefix);
        let nested = format!("{prefix}{IND}");
        for (i, output) in outputs.iter().enumerate() {
            self.output(Some(i), output, &nested)?;
        }
        Ok(())
    }

    pub fn attachments(&mut self, attachments: &Value, prefix: &str) -> Result<()> {
        self.key("attachments", prefix);
        let nested = format!("{prefix}{IND}");
        match attachments {
            Value::Object(map) => {
                let mut names: Vec<_> = map.iter().collect();
                names.sort_by(|a, b| a.0.cmp(b.0));
                for (name, attachment) in names {
                    self.item(name, attachment, &nested)?;
                }
                Ok(())
            }
            other if !other.truthy() && matches!(other, Value::List(_) | Value::Str(_)) => Ok(()),
            _ => invalid("attachments must be a dict"),
        }
    }

    fn source(&mut self, source: &Value, prefix: &str, is_markdown: bool) -> Result<()> {
        self.key("source", prefix);
        let Value::Str(text) = source else {
            return invalid("expected string argument");
        };
        let nested = format!("{prefix}{IND}");
        if is_markdown && prefix.chars().all(py::is_space) {
            let highlighted = colorize_markdown(text)?;
            self.multiline(&highlighted, &nested);
        } else {
            self.multiline(text, &nested);
        }
        Ok(())
    }

    /// `pretty_print_cell`: the parts of cell `i` that the config shows. The header with the
    /// cell type comes first, but only if something follows it or `force_header` is set.
    pub fn cell(
        &mut self,
        i: Option<usize>,
        cell: &Value,
        prefix: &str,
        force_header: bool,
    ) -> Result<()> {
        let Value::Object(map) = cell else {
            return invalid("a cell must be a dict");
        };
        let config = self.config;
        let key_prefix = format!("{prefix}{IND}");
        let mut header_written = false;
        let mut header = |this: &mut Self| {
            if !header_written {
                let cell_type = map
                    .get("cell_type")
                    .map_or("None".into(), |v| crate::prettyprint::py_str(v));
                let name = match i {
                    None => format!("{cell_type} cell"),
                    Some(i) => format!("{cell_type} cell {i}"),
                };
                this.key(&name, prefix);
                header_written = true;
            }
        };

        if force_header {
            header(self);
        }
        if let Some(id) = map.get("id").filter(|v| v.truthy()) {
            if config.details {
                header(self);
                self.item("id", id, &key_prefix)?;
            }
        }
        if let Some(count) = map.get("execution_count").filter(|v| v.truthy()) {
            if config.details {
                header(self);
                self.item("execution_count", count, &key_prefix)?;
            }
        }
        if let Some(metadata) = map.get("metadata").filter(|v| v.truthy()) {
            if config.metadata {
                header(self);
                const KNOWN: [&str; 6] = [
                    "collapsed",
                    "autoscroll",
                    "deletable",
                    "format",
                    "name",
                    "tags",
                ];
                self.metadata(metadata, &KNOWN, &key_prefix)?;
            }
        }
        if let Some(source) = map.get("source").filter(|v| v.truthy()) {
            if config.sources {
                let is_markdown = map.get("cell_type") == Some(&Value::from("markdown"));
                header(self);
                self.source(source, &key_prefix, is_markdown)?;
            }
        }
        if let Some(attachments) = map.get("attachments").filter(|v| v.truthy()) {
            if config.attachments {
                header(self);
                self.attachments(attachments, &key_prefix)?;
            }
        }
        if let Some(outputs) = map.get("outputs").filter(|v| v.truthy()) {
            if config.outputs {
                header(self);
                let Value::List(outputs) = outputs else {
                    return invalid("outputs must be a list");
                };
                self.outputs(outputs, &key_prefix)?;
            }
        }
        // 'attachment' (sic): upstream's exclusion list misspells the key, so the attachments of
        // a cell are listed once more with the other keys.
        const EXCLUDE: [&str; 7] = [
            "cell_type",
            "source",
            "execution_count",
            "outputs",
            "metadata",
            "id",
            "attachment",
        ];
        if config.details && map.keys().any(|k| !EXCLUDE.contains(&k.as_str())) {
            header(self);
            self.dict(map, &EXCLUDE, &key_prefix)?;
        }
        Ok(())
    }

    /// `pretty_print_diff_entry`: one entry of the diff of `a` at `path`.
    pub fn diff_entry(&mut self, a: &Value, e: &DiffEntry, path: &str) -> Result<()> {
        let config = self.config;
        if config.should_ignore_path(path) {
            return Ok(());
        }
        let nextpath = format!("{path}/{}", e.key);
        match &e.op {
            Op::Patch { diff } => {
                return self.diff(item_at(a, &e.key)?, diff, &nextpath);
            }
            Op::AddRange { valuelist } => {
                self.diff_action("inserted before", &nextpath);
                self.value_at(valuelist, Some(path), config.add())?;
            }
            Op::RemoveRange { length } => {
                let Key::Index(start) = e.key else {
                    return invalid("removerange needs an integer key");
                };
                let keyrange = if *length > 1 {
                    format!("{nextpath}-{}", start + length - 1)
                } else {
                    nextpath
                };
                self.diff_action("deleted", &keyrange);
                let Value::List(items) = a else {
                    return invalid("only a list can be sliced");
                };
                let end = start.saturating_add(*length).min(items.len());
                let removed = Value::List(items[start.min(end)..end].to_vec());
                self.value_at(&removed, Some(path), config.remove())?;
            }
            Op::Remove => {
                if config.should_ignore_path(&nextpath) {
                    return Ok(());
                }
                self.diff_action("deleted", &nextpath);
                self.value_at(item_at(a, &e.key)?, Some(&nextpath), config.remove())?;
            }
            Op::Add { value } => {
                if config.should_ignore_path(&nextpath) {
                    return Ok(());
                }
                self.diff_action("added", &nextpath);
                self.value_at(value, Some(&nextpath), config.add())?;
            }
            Op::Replace { value: bval } => {
                if config.should_ignore_path(&nextpath) {
                    return Ok(());
                }
                let aval = item_at(a, &e.key)?;
                let typechange = if aval.same_type(bval) {
                    String::new()
                } else {
                    format!(
                        " (type changed from {} to {})",
                        type_name(aval),
                        type_name(bval)
                    )
                };
                self.diff_action(&format!("replaced{typechange}"), &nextpath);
                self.value_at(aval, Some(&nextpath), config.remove())?;
                self.value_at(bval, Some(&nextpath), config.add())?;
            }
            Op::ParentDeleted => {
                return invalid(format!("Unknown list diff op {}", e.op.name()));
            }
        }
        self.write(NOTEBOOK_DIFF_ENTRY_END);
        self.write(config.reset());
        Ok(())
    }

    /// The diff of a dict: entries in key order.
    pub fn dict_diff(&mut self, a: &Value, di: &[DiffEntry], path: &str) -> Result<()> {
        let mut entries: Vec<&DiffEntry> = di.iter().collect();
        if entries.iter().any(|e| matches!(e.key, Key::Index(_)))
            && entries.iter().any(|e| matches!(e.key, Key::Name(_)))
        {
            return invalid("cannot order integer and string keys");
        }
        entries.sort_by(|x, y| match (&x.key, &y.key) {
            (Key::Name(x), Key::Name(y)) => x.cmp(y),
            (Key::Index(x), Key::Index(y)) => x.cmp(y),
            _ => std::cmp::Ordering::Equal,
        });
        for e in entries {
            self.diff_entry(a, e, path)?;
        }
        Ok(())
    }

    /// The diff of a list: entries in the order they come.
    pub fn list_diff(&mut self, a: &Value, di: &[DiffEntry], path: &str) -> Result<()> {
        for e in di {
            self.diff_entry(a, e, path)?;
        }
        Ok(())
    }

    /// The diff of a string: the new text is computed, and the change is shown with the external
    /// diff renderer when the text has several lines.
    pub fn string_diff(&mut self, a: &str, di: &[DiffEntry], path: &str) -> Result<()> {
        let config = self.config;
        self.diff_action("modified", path);
        let Value::Str(b) = patch(&Value::Str(a.to_owned()), di)? else {
            return invalid("patching a string must give a string");
        };

        let ta = trim_base64(a);
        let tb = trim_base64(&b);
        let a_trimmed = matches!(ta, std::borrow::Cow::Owned(_));
        let b_trimmed = matches!(tb, std::borrow::Cow::Owned(_));
        if a_trimmed || b_trimmed {
            if a_trimmed {
                self.write(&format!("{}{ta}\n", config.remove()));
            } else {
                self.value_at(&Value::from(a), Some(path), config.remove())?;
            }
            if b_trimmed {
                self.write(&format!("{}{tb}\n", config.add()));
            } else {
                self.value_at(&Value::from(b.as_str()), Some(path), config.add())?;
            }
        } else if a.contains('\n') || b.contains('\n') {
            let diff = diff_render(a, &b, config)?;
            self.write(&diff);
        } else {
            self.write(&format!("{}{a}\n", config.remove()));
            self.write(&format!("{}{b}\n", config.add()));
        }
        self.write(NOTEBOOK_DIFF_ENTRY_END);
        self.write(config.reset());
        Ok(())
    }

    /// `pretty_print_diff`: the diff `di` of `a`.
    pub fn diff(&mut self, a: &Value, di: &[DiffEntry], path: &str) -> Result<()> {
        match a {
            Value::Object(_) => self.dict_diff(a, di, path),
            Value::List(_) => self.list_diff(a, di, path),
            Value::Str(s) => self.string_diff(s, di, path),
            other => invalid(format!(
                "Invalid type {} for diff presentation.",
                type_name(other)
            )),
        }
    }

    /// `pretty_print_notebook_diff`: the header naming both files and then the diff, unless the
    /// diff is empty.
    pub fn notebook_diff(
        &mut self,
        afn: &str,
        bfn: &str,
        a: &Value,
        di: &[DiffEntry],
    ) -> Result<()> {
        if di.is_empty() {
            return Ok(());
        }
        if a.nesting_exceeds(MAX_DEPTH) {
            return Err(PrettyPrintError::TooDeep);
        }
        let atime = file_timestamp(afn)?;
        let btime = file_timestamp(bfn)?;
        self.write(&format!(
            "nbdiff {afn} {bfn}\n--- {afn}  {atime}\n+++ {bfn}  {btime}\n"
        ));
        self.diff(a, di, "")
    }

    /// `pretty_print_merge_decision`.
    pub fn merge_decision(&mut self, base: &Value, decision: &MergeDecision) -> Result<()> {
        let config = self.config;
        let path = join_path(&decision.common_path);
        let confnote = if decision.conflict { "conflicted " } else { "" };
        self.write(&format!(
            "{}{confnote}decision at {path}:{}\n",
            config.info().replace("##", "===="),
            config.reset()
        ));

        const DIFF_KEYS: [&str; 5] = [
            "diff",
            "local_diff",
            "remote_diff",
            "custom_diff",
            "similar_insert",
        ];
        let exclude: Vec<&str> = DIFF_KEYS
            .into_iter()
            .chain(["common_path", "action", "conflict"])
            .collect();
        let Value::Object(fields) = decision.to_value() else {
            return invalid("a decision serializes as a dict");
        };
        self.dict(&fields, &exclude, IND)?;

        let action = decision.action.as_str();
        for dkey in DIFF_KEYS {
            let diff = match dkey {
                "local_diff" => decision.local_diff.as_ref(),
                "remote_diff" => decision.remote_diff.as_ref(),
                "custom_diff" => decision.custom_diff.as_ref(),
                "similar_insert" => decision.similar_insert.as_ref(),
                _ => None,
            };
            let same_as = |other: Option<&Vec<DiffEntry>>| match (diff, other) {
                (None, None) => true,
                (Some(x), Some(y)) => diffs_eq(x, y),
                _ => false,
            };
            let note = if dkey == "remote_diff"
                && decision.action == Action::Either
                && same_as(decision.local_diff.as_ref())
            {
                continue;
            } else if dkey == "local_diff"
                && decision.action == Action::Either
                && same_as(decision.remote_diff.as_ref())
            {
                " (same as remote_diff)"
            } else if dkey.starts_with(action) {
                " (selected)"
            } else {
                ""
            };

            let Some(diff) = diff.filter(|d| !d.is_empty()) else {
                continue;
            };
            self.write(&format!(
                "{}{dkey}{note}:{}\n",
                config.info().replace("##", "---"),
                config.reset()
            ));
            let mut value = base;
            let mut diff = diff.clone();
            for (i, k) in decision.common_path.iter().enumerate() {
                if matches!(value, Value::Str(_)) {
                    // Diffs of strings are line based, except when `common_path` points at a
                    // line within one (it is then the last key): wrap the character diff in a
                    // patch of that line.
                    if i != decision.common_path.len() - 1 {
                        return invalid(format!(
                            "invalid decision common path, tries to subindex string: {path}"
                        ));
                    }
                    diff = vec![DiffEntry {
                        key: k.clone(),
                        op: Op::Patch { diff },
                    }];
                    break;
                }
                value = item_at(value, k)?;
            }
            self.diff(value, &diff, &path)?;
        }
        Ok(())
    }

    /// `pretty_print_merge_decisions`: the number of conflicts, then every decision.
    pub fn merge_decisions(&mut self, base: &Value, decisions: &[MergeDecision]) -> Result<()> {
        if base.nesting_exceeds(MAX_DEPTH) {
            return Err(PrettyPrintError::TooDeep);
        }
        let conflicted = decisions.iter().filter(|d| d.conflict).count();
        self.write(&format!(
            "{conflicted} conflicted decisions of {} total:\n",
            decisions.len()
        ));
        for decision in decisions {
            self.merge_decision(base, decision)?;
        }
        Ok(())
    }
}

/// `value[key]` for a dict with a string key or a list with an integer index.
fn item_at<'v>(value: &'v Value, key: &Key) -> Result<&'v Value> {
    let item = match (value, key) {
        (Value::Object(map), Key::Name(name)) => map.get(name),
        (Value::List(items), Key::Index(i)) => items.get(*i),
        _ => return invalid(format!("cannot index a {} with {key:?}", type_name(value))),
    };
    match item {
        Some(item) => Ok(item),
        None => invalid(format!("key {key} not found")),
    }
}

/// The items `for x in value` visits, where upstream only gets anywhere with lists: strings and
/// dicts hand out strings, which none of the callers can use, and nothing else is iterable.
fn iterate(value: &Value) -> Result<Vec<&Value>> {
    match value {
        Value::List(items) => Ok(items.iter().collect()),
        Value::Str(s) if s.is_empty() => Ok(Vec::new()),
        Value::Object(m) if m.is_empty() => Ok(Vec::new()),
        other => invalid(format!(
            "cannot use a {} as a list of items",
            type_name(other)
        )),
    }
}

/// `type(value).__name__` of a value as nbformat reads it, where every dict is a `NotebookNode`.
fn type_name(value: &Value) -> &'static str {
    match value.py_type() {
        PyType::None => "NoneType",
        PyType::Bool => "bool",
        PyType::Int => "int",
        PyType::Float => "float",
        PyType::Str => "str",
        PyType::List => "list",
        PyType::Dict => "NotebookNode",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_format::{diff_from_value, Diff};
    use crate::json::parse;
    use crate::merging::decisions::MergeDecision;

    fn plain() -> PrettyPrintConfig {
        PrettyPrintConfig {
            use_color: false,
            use_git: false,
            use_diff: false,
            ..PrettyPrintConfig::default()
        }
    }

    fn value_at(json: &str, path: Option<&str>, prefix: &str) -> Result<String> {
        let config = plain();
        let mut out = String::new();
        Printer::new(&config, &mut out).value_at(&parse(json).unwrap(), path, prefix)?;
        Ok(out)
    }

    fn notebook_diff(config: &PrettyPrintConfig, nb: &str, diff: &str) -> Result<String> {
        let di: Diff = diff_from_value(&parse(diff).unwrap()).unwrap();
        let mut out = String::new();
        Printer::new(config, &mut out).notebook_diff(
            "a.ipynb",
            "b.ipynb",
            &parse(nb).unwrap(),
            &di,
        )?;
        Ok(out)
    }

    #[test]
    fn multiline_always_ends_with_a_newline() {
        let config = plain();
        let mut out = String::new();
        let mut printer = Printer::new(&config, &mut out);
        printer.multiline("", "p");
        printer.multiline("a\nb", "> ");
        printer.multiline("c\r", "> ");
        assert_eq!(out, "\n> a\n> b\n> c\r\n");
    }

    #[test]
    fn items_nest_and_wrap() {
        assert_eq!(
            value_at(r#"[1, "a\nb"]"#, None, "> ").unwrap(),
            "> item[0]: 1\n> item[1]:\n>   a\n>   b\n"
        );
        assert_eq!(
            value_at(r#"{"a": {"b": [1, {"c": "x\ny"}]}, "z": []}"#, None, "").unwrap(),
            "a:\n  b:\n    item[0]: 1\n    item[1]:\n      c:\n        x\n        y\nz:\n  []\n"
        );
    }

    #[test]
    fn short_lists_are_one_line_and_long_ones_are_itemized() {
        assert_eq!(value_at("[1, 2, 3]", None, "").unwrap(), "[1, 2, 3]\n");
        let long = format!(
            "[{}]",
            (0..30)
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let out = value_at(&long, None, "").unwrap();
        assert!(out.starts_with("item[0]: 0\nitem[1]: 1\n"), "{out}");
        // A newline escape in the repr forces itemizing as well.
        assert_eq!(
            value_at(r#"["a\nb"]"#, None, "").unwrap(),
            "item[0]:\n  a\n  b\n"
        );
    }

    #[test]
    fn empty_values_print_as_python_does() {
        assert_eq!(value_at("{}", None, "").unwrap(), "");
        assert_eq!(value_at("[]", None, "").unwrap(), "[]\n");
        assert_eq!(value_at(r#""""#, None, "").unwrap(), "\n");
        assert_eq!(value_at("null", None, "- ").unwrap(), "- None\n");
    }

    #[test]
    fn paths_must_be_splittable() {
        assert!(value_at("1", Some(""), "").is_err());
        assert!(value_at("1", Some("abc"), "").is_err());
        assert_eq!(value_at("1", Some("abc/def"), "").unwrap(), "1\n");
        assert_eq!(value_at("1", Some("/x"), "").unwrap(), "1\n");
    }

    #[test]
    fn cells_list_attachments_twice_and_hide_what_is_not_shown() {
        let cell = r#"{"cell_type": "markdown", "metadata": {"tags": ["a"], "zz": 1},
            "source": "hi\nthere", "id": "abc", "attachments": {"x.png": {"image/png": "QUJD"}},
            "extra": [1, 2]}"#;
        assert_eq!(
            value_at(cell, Some("/cells/1"), "+  ").unwrap(),
            "+  markdown cell:\n+    id: abc\n+    metadata (known keys):\n+      tags:\n+        ['a']\n\
             +    metadata (unknown keys):\n+      zz: 1\n+    source:\n+      hi\n+      there\n\
             +    attachments:\n+      x.png:\n+        image/png: QUJD\n\
             +    attachments:\n+      x.png:\n+        image/png: QUJD\n+    extra:\n+      [1, 2]\n"
        );
    }

    #[test]
    fn a_cell_header_needs_a_reason_unless_forced() {
        let config = PrettyPrintConfig {
            sources: false,
            ..plain()
        };
        let cell = parse(r#"{"cell_type": "raw", "source": "x"}"#).unwrap();
        let mut out = String::new();
        Printer::new(&config, &mut out)
            .cell(Some(3), &cell, "", false)
            .unwrap();
        assert_eq!(out, "");
        Printer::new(&config, &mut out)
            .cell(Some(3), &cell, "", true)
            .unwrap();
        assert_eq!(out, "raw cell 3:\n");
    }

    #[test]
    fn outputs_print_known_keys_first_then_the_rest() {
        let output = r#"{"output_type": "display_data", "metadata": {"isolated": true, "w": 1},
            "data": {"text/plain": "x"}, "transient": {"a": 1}}"#;
        assert_eq!(
            value_at(output, Some("/cells/0/outputs/1"), "").unwrap(),
            "output:\n  output_type: display_data\n  data:\n    text/plain: x\n\
             \x20 metadata (known keys):\n    isolated: True\n  metadata (unknown keys):\n    w: 1\n\
             \x20 transient:\n    a: 1\n"
        );
    }

    #[test]
    fn markdown_with_a_blank_prefix_is_highlighted() {
        let cell = r##"{"cell_type": "markdown", "source": "# Title"}"##;
        assert_eq!(
            value_at(cell, Some("/cells/0"), "").unwrap(),
            "markdown cell:\n  source:\n    \x1b[38;5;18;01m# Title\x1b[39;00m\n"
        );
        assert_eq!(
            value_at(cell, Some("/cells/0"), "+  ").unwrap(),
            "+  markdown cell:\n+    source:\n+      # Title\n"
        );
    }

    #[test]
    fn values_that_are_no_cells_are_errors() {
        assert!(value_at("1", Some("/cells/0"), "").is_err());
        assert!(value_at(r#"["a"]"#, Some("/cells"), "").is_err());
        assert!(value_at("[]", Some("/cells"), "").unwrap().is_empty());
        assert!(value_at(r#"{"output_type": 1}"#, Some("/cells/0/attachments"), "").is_ok());
        assert!(value_at(r#"[1]"#, Some("/cells/0/attachments"), "").is_err());
    }

    const NOTEBOOK: &str = r#"{"nbformat": 4, "nbformat_minor": 5, "metadata": {"x": "old", "y": 1},
        "cells": [{"cell_type": "code", "source": "a = 1\nb = 2\nc = 3\n", "id": "1"}]}"#;

    #[test]
    fn string_diffs_use_the_builtin_renderer_without_git_and_diff() {
        let diff = r#"[{"op": "patch", "key": "cells", "diff": [{"op": "patch", "key": 0, "diff":
            [{"op": "patch", "key": "source", "diff": [{"op": "patch", "key": 1, "diff":
            [{"op": "addrange", "key": 2, "valuelist": "X"}]}]}]}]},
            {"op": "patch", "key": "metadata", "diff": [{"op": "replace", "key": "x", "value": "new"}]}]"#;
        assert_eq!(
            notebook_diff(&plain(), NOTEBOOK, diff).unwrap(),
            "nbdiff a.ipynb b.ipynb\n--- a.ipynb  (no timestamp)\n+++ b.ipynb  (no timestamp)\n\
             ## modified /cells/0/source:\n@@ -1,3 +1,3 @@\n   a = 1\n-  b = 2\n+  b X= 2\n   c = 3\n\
             ## replaced /metadata/x:\n-  old\n+  new\n\n"
        );
        let colored = PrettyPrintConfig {
            use_color: true,
            ..plain()
        };
        let out = notebook_diff(&colored, NOTEBOOK, diff).unwrap();
        assert!(out.contains("\x1b[34m\x1b[1m## modified /cells/0/source:\x1b[0m\n@@ -1,3"));
        assert!(out.ends_with("\x1b[31m-  old\n\x1b[32m+  new\n\n\x1b[0m"));
    }

    #[test]
    fn an_empty_diff_prints_nothing_and_ignored_paths_are_skipped() {
        assert_eq!(notebook_diff(&plain(), NOTEBOOK, "[]").unwrap(), "");
        let hide = PrettyPrintConfig {
            metadata: false,
            ..plain()
        };
        let diff =
            r#"[{"op": "patch", "key": "metadata", "diff": [{"op": "remove", "key": "y"}]}]"#;
        assert_eq!(
            notebook_diff(&hide, NOTEBOOK, diff).unwrap(),
            "nbdiff a.ipynb b.ipynb\n--- a.ipynb  (no timestamp)\n+++ b.ipynb  (no timestamp)\n"
        );
    }

    #[test]
    fn type_changes_are_named_like_python() {
        let diff = r#"[{"op": "patch", "key": "metadata", "diff": [{"op": "replace", "key": "y", "value": null},
            {"op": "replace", "key": "x", "value": {"a": 1}}]}]"#;
        let out = notebook_diff(&plain(), NOTEBOOK, diff).unwrap();
        assert!(out.contains(
            "## replaced (type changed from str to NotebookNode) /metadata/x:\n-  old\n+  a: 1\n"
        ));
        assert!(out.contains(
            "## replaced (type changed from int to NoneType) /metadata/y:\n-  1\n+  None\n"
        ));
    }

    #[test]
    fn removed_ranges_are_clamped_and_named_by_their_extent() {
        let nb = r#"{"metadata": {"l": [1, 2, 3]}}"#;
        let diff = r#"[{"op": "patch", "key": "metadata", "diff": [{"op": "patch", "key": "l", "diff":
            [{"op": "removerange", "key": 1, "length": 5}]}]}]"#;
        let out = notebook_diff(&plain(), nb, diff).unwrap();
        assert!(out.contains("## deleted /metadata/l/1-5:\n"), "{out}");
        assert!(out.ends_with("-  [2, 3]\n\n"), "{out}");
    }

    #[test]
    fn diffs_that_do_not_fit_are_errors() {
        let missing = r#"[{"op": "remove", "key": "nope"}]"#;
        assert!(notebook_diff(&plain(), NOTEBOOK, missing).is_err());
        let wrong_type = r#"[{"op": "patch", "key": "nbformat", "diff": []}]"#;
        assert!(notebook_diff(&plain(), NOTEBOOK, wrong_type).is_err());
        let addrange_on_dict = r#"[{"op": "addrange", "key": 0, "valuelist": [1]}]"#;
        assert!(notebook_diff(&plain(), NOTEBOOK, addrange_on_dict).is_err());
    }

    const BASE: &str = r#"{"nbformat": 4, "nbformat_minor": 5, "metadata": {}, "cells":
        [{"cell_type": "code", "source": "a\nb\n", "metadata": {}, "outputs": [], "id": "1"}]}"#;

    const DECISIONS: &str = r#"[
        {"common_path": ["cells", 0, "source"], "conflict": true, "action": "base",
         "local_diff": [{"op": "patch", "key": 0, "diff": [{"op": "addrange", "key": 1, "valuelist": "X"}]}],
         "remote_diff": [{"op": "removerange", "key": 0, "length": 1}], "strategy": "mixed"},
        {"common_path": ["cells", 0, "source", 1], "conflict": false, "action": "either",
         "local_diff": [{"op": "addrange", "key": 0, "valuelist": "Q"}],
         "remote_diff": [{"op": "addrange", "key": 0, "valuelist": "Q"}]},
        {"common_path": [], "conflict": false, "action": "local",
         "local_diff": [{"op": "add", "key": "new", "value": 1}], "remote_diff": null}]"#;

    fn merge_decisions(config: &PrettyPrintConfig, decisions: &str) -> Result<String> {
        let Value::List(items) = parse(decisions).unwrap() else {
            panic!("decisions are a list");
        };
        let decisions: Vec<MergeDecision> = items
            .iter()
            .map(|d| MergeDecision::from_value(d).unwrap())
            .collect();
        let mut out = String::new();
        Printer::new(config, &mut out).merge_decisions(&parse(BASE).unwrap(), &decisions)?;
        Ok(out)
    }

    #[test]
    fn merge_decisions_name_conflicts_selected_sides_and_equal_sides() {
        assert_eq!(
            merge_decisions(&plain(), DECISIONS).unwrap(),
            "1 conflicted decisions of 3 total:\n\
             ==== conflicted decision at /cells/0/source:\n  strategy: mixed\n\
             --- local_diff:\n## modified /cells/0/source:\n@@ -1,2 +1,2 @@\n-  a\n+  aX\n   b\n\
             --- remote_diff:\n## modified /cells/0/source:\n@@ -1,2 +1 @@\n-  a\n   b\n\
             ==== decision at /cells/0/source/1:\n\
             --- local_diff (same as remote_diff):\n## modified /cells/0/source/1:\n\
             @@ -1,2 +1,2 @@\n   a\n-  b\n+  Qb\n\
             ==== decision at /:\n--- local_diff (selected):\n## added //new:\n+  1\n\n"
        );
    }

    #[test]
    fn merge_decisions_are_colored_with_their_own_markers() {
        let colored = PrettyPrintConfig {
            use_color: true,
            ..plain()
        };
        let out = merge_decisions(&colored, DECISIONS).unwrap();
        assert!(
            out.contains("\x1b[34m\x1b[1m==== conflicted decision at /cells/0/source:\x1b[0m\n")
        );
        assert!(out.contains("\x1b[34m\x1b[1m--- remote_diff:\x1b[0m\n"));
    }

    #[test]
    fn decisions_that_do_not_fit_the_base_are_errors() {
        let bad_path = r#"[{"common_path": ["cells", 5], "conflict": false, "action": "local",
            "local_diff": [{"op": "remove", "key": "k"}], "remote_diff": null}]"#;
        assert!(merge_decisions(&plain(), bad_path).is_err());
        let through_string = r#"[{"common_path": ["cells", 0, "source", 1, 0], "conflict": false,
            "action": "local", "local_diff": [{"op": "addrange", "key": 0, "valuelist": "x"}],
            "remote_diff": null}]"#;
        assert!(merge_decisions(&plain(), through_string).is_err());
    }
}
