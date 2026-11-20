//! Reading and writing v4 notebooks the way `nbformat` does.
//!
//! [`reads`] returns what `nbformat.reads(text, as_version=4)` returns, [`writes`] what
//! `nbformat.write` writes. Where upstream raises (a missing `cells` key, a cell that is not an
//! object, ...) these functions return an error, and where upstream copes with odd input they
//! produce the same result.
//!
//! Deliberate deviations (see `docs/findings.md`): missing and duplicate cell ids are repaired
//! deterministically (F-003), a leading BOM is ignored (Q-002), notebooks that are not nbformat 4
//! are rejected (F-012), and schema violations are neither detected nor logged (F-013).

use std::collections::{HashMap, HashSet};

use crate::json::{self, DumpOptions, ParseError};
use crate::py::splitlines_keepends;
use crate::value::{Int, Map, Value};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum NbformatError {
    #[error("notebook is not valid JSON: {0}")]
    Json(ParseError),
    #[error("unsupported notebook format ({found}): {hint}")]
    UnsupportedVersion { found: String, hint: &'static str },
    #[error("invalid notebook: {0}")]
    Invalid(String),
}

impl From<ParseError> for NbformatError {
    fn from(e: ParseError) -> Self {
        NbformatError::Json(e)
    }
}

fn invalid(message: impl Into<String>) -> NbformatError {
    NbformatError::Invalid(message.into())
}

/// Adds the cell index to an error from deeper in the cell.
fn in_cell(index: usize) -> impl Fn(NbformatError) -> NbformatError {
    move |error| match error {
        NbformatError::Invalid(message) => invalid(format!("cell {index}: {message}")),
        other => other,
    }
}

/// `nbformat.reads(text, as_version=4)`.
pub fn reads(text: &str) -> Result<Value, NbformatError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut nb = json::parse(text)?;
    let minor = version_minor(&nb)?;
    let map = object_mut(&mut nb, "notebook")?;
    rejoin_lines(map)?;
    strip_transient(map)?;
    validate(map, minor, false)?;
    Ok(nb)
}

/// The text `nbformat.write(nb, fp)` writes for a v4 notebook.
pub fn writes(nb: &Value) -> Result<String, NbformatError> {
    let minor = version_minor(nb)?;
    let mut nb = nb.clone();
    let map = object_mut(&mut nb, "notebook")?;
    validate(map, minor, true)?;
    split_lines(map)?;
    strip_transient(map)?;
    let mut text = json::dumps(&nb, &DumpOptions::nbformat_write());
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

/// `nbformat.v4.new_notebook()`.
pub fn new_notebook() -> Value {
    let mut nb = Map::new();
    nb.insert("nbformat".into(), Value::from(4));
    nb.insert("nbformat_minor".into(), Value::from(5));
    nb.insert("metadata".into(), Value::Object(Map::new()));
    nb.insert("cells".into(), Value::List(Vec::new()));
    Value::Object(nb)
}

/// Checks `nbformat == 4` and returns `nbformat_minor` (saturated to `i64`).
///
/// Upstream accepts only an `int` minor version at validation, and `bool` is an `int`.
fn version_minor(nb: &Value) -> Result<i64, NbformatError> {
    let map = nb
        .as_object()
        .ok_or_else(|| invalid("a notebook must be a JSON object"))?;
    match map.get("nbformat") {
        Some(Value::Int(Int::Small(4))) => {}
        Some(other) => {
            let hint = match other {
                Value::Int(Int::Small(1..=3)) => {
                    "only nbformat 4 notebooks are supported; convert older notebooks first, \
                     for example with `jupyter nbconvert --to notebook`"
                }
                Value::Int(Int::Small(5..)) => {
                    "only nbformat 4 notebooks are supported; this notebook is newer"
                }
                _ => "\"nbformat\" must be the integer 4",
            };
            let shown = json::dumps(other, &DumpOptions::compact());
            return Err(NbformatError::UnsupportedVersion {
                found: format!("nbformat {shown}"),
                hint,
            });
        }
        None => {
            return Err(NbformatError::UnsupportedVersion {
                found: "no nbformat version".into(),
                hint: "\"nbformat\" must be the integer 4",
            })
        }
    }
    match map.get("nbformat_minor") {
        None => Ok(0),
        Some(Value::Bool(b)) => Ok(i64::from(*b)),
        Some(Value::Int(Int::Small(v))) => Ok(*v),
        Some(Value::Int(Int::Big(digits))) => Ok(if digits.starts_with('-') {
            i64::MIN
        } else {
            i64::MAX
        }),
        Some(_) => Err(invalid("nbformat_minor must be an integer")),
    }
}

fn object_mut<'a>(value: &'a mut Value, what: &str) -> Result<&'a mut Map, NbformatError> {
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(invalid(format!("{what} must be a JSON object"))),
    }
}

/// The items of `value` as upstream iterates them with `for x in value`.
///
/// An empty dict or string iterates nothing, so upstream accepts it; a non-empty one yields
/// strings, which upstream then fails to treat as cells or outputs.
fn items_mut<'a>(value: &'a mut Value, what: &str) -> Result<&'a mut [Value], NbformatError> {
    match value {
        Value::List(items) => Ok(items),
        Value::Object(map) if map.is_empty() => Ok(&mut []),
        Value::Str(s) if s.is_empty() => Ok(&mut []),
        _ => Err(invalid(format!("{what} must be a list"))),
    }
}

fn cells_mut(nb: &mut Map) -> Result<&mut [Value], NbformatError> {
    let cells = nb
        .get_mut("cells")
        .ok_or_else(|| invalid("missing \"cells\""))?;
    items_mut(cells, "cells")
}

/// `"".join(items)` if every item is a string.
fn join_strings(items: &[Value]) -> Option<String> {
    let mut joined = String::new();
    for item in items {
        joined.push_str(item.as_str()?);
    }
    Some(joined)
}

fn is_json_mime(mime: &str) -> bool {
    mime == "application/json" || (mime.starts_with("application/") && mime.ends_with("+json"))
}

fn rejoin_mimebundle(data: &mut Map) {
    for (key, value) in data.iter_mut() {
        if is_json_mime(key) {
            continue;
        }
        if let Value::List(items) = value {
            if let Some(joined) = join_strings(items) {
                *value = Value::Str(joined);
            }
        }
    }
}

fn rejoin_lines(nb: &mut Map) -> Result<(), NbformatError> {
    for (index, cell) in cells_mut(nb)?.iter_mut().enumerate() {
        rejoin_cell(object_mut(cell, &format!("cell {index}"))?, index)?;
    }
    Ok(())
}

fn rejoin_cell(cell: &mut Map, index: usize) -> Result<(), NbformatError> {
    let bad = |what: &str| invalid(format!("cell {index}: {what}"));

    if let Some(source) = cell.get_mut("source") {
        if let Value::List(lines) = source {
            let joined = join_strings(lines).ok_or_else(|| bad("source lines must be strings"))?;
            *source = Value::Str(joined);
        }
    }
    match cell.get_mut("attachments") {
        None => {}
        Some(Value::Object(attachments)) => {
            for attachment in attachments.values_mut() {
                let attachment = object_mut(attachment, "attachment").map_err(in_cell(index))?;
                rejoin_mimebundle(attachment);
            }
        }
        Some(_) => return Err(bad("attachments must be an object")),
    }
    // Only code cells have outputs that are touched.
    if cell.get("cell_type").and_then(Value::as_str) != Some("code") {
        return Ok(());
    }
    let Some(outputs) = cell.get_mut("outputs") else {
        return Ok(());
    };
    for output in items_mut(outputs, "outputs").map_err(in_cell(index))? {
        let output = object_mut(output, "output").map_err(in_cell(index))?;
        rejoin_output(output).map_err(in_cell(index))?;
    }
    Ok(())
}

fn rejoin_output(output: &mut Map) -> Result<(), NbformatError> {
    match output.get("output_type") {
        Some(Value::Str(t)) if t == "execute_result" || t == "display_data" => {
            if let Some(data) = output.get_mut("data") {
                rejoin_mimebundle(object_mut(data, "output data")?);
            }
        }
        // Upstream tests membership in a set, which cannot hold a list or dict.
        Some(Value::List(_) | Value::Object(_)) => return Err(invalid("unhashable output_type")),
        // Any other truthy output type has its `text` lines joined.
        Some(t) if t.truthy() => {
            if let Some(text) = output.get_mut("text") {
                if let Value::List(lines) = text {
                    let joined =
                        join_strings(lines).ok_or_else(|| invalid("text lines must be strings"))?;
                    *text = Value::Str(joined);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn strip_transient(nb: &mut Map) -> Result<(), NbformatError> {
    let metadata = nb
        .get_mut("metadata")
        .ok_or_else(|| invalid("missing \"metadata\""))?;
    let metadata = object_mut(metadata, "metadata")?;
    for key in ["orig_nbformat", "orig_nbformat_minor", "signature"] {
        metadata.shift_remove(key);
    }
    for (index, cell) in cells_mut(nb)?.iter_mut().enumerate() {
        let metadata = object_mut(cell, &format!("cell {index}"))?
            .get_mut("metadata")
            .ok_or_else(|| invalid(format!("cell {index}: missing \"metadata\"")))?;
        object_mut(metadata, "cell metadata")?.shift_remove("trusted");
    }
    Ok(())
}

fn is_split_mime(mime: &str) -> bool {
    mime.starts_with("text/") || mime == "application/javascript" || mime == "image/svg+xml"
}

fn split_lines_value(text: &str) -> Value {
    Value::List(
        splitlines_keepends(text)
            .into_iter()
            .map(Value::from)
            .collect(),
    )
}

fn split_mimebundle(data: &mut Map) {
    for (key, value) in data.iter_mut() {
        if let Value::Str(text) = value {
            if is_split_mime(key) {
                *value = split_lines_value(text);
            }
        }
    }
}

fn split_lines(nb: &mut Map) -> Result<(), NbformatError> {
    for (index, cell) in cells_mut(nb)?.iter_mut().enumerate() {
        let cell = object_mut(cell, &format!("cell {index}"))?;
        let bad = |what: &str| invalid(format!("cell {index}: {what}"));
        if let Some(source) = cell.get_mut("source") {
            if let Value::Str(text) = source {
                *source = split_lines_value(text);
            }
        }
        match cell.get_mut("attachments") {
            None => {}
            Some(Value::Object(attachments)) => {
                for attachment in attachments.values_mut() {
                    let attachment =
                        object_mut(attachment, "attachment").map_err(in_cell(index))?;
                    split_mimebundle(attachment);
                }
            }
            Some(_) => return Err(bad("attachments must be an object")),
        }
        // Unlike on reading, a missing `cell_type` or `outputs` is an error here.
        match cell.get("cell_type") {
            None => return Err(bad("missing \"cell_type\"")),
            Some(Value::Str(t)) if t == "code" => {}
            Some(_) => continue,
        }
        let outputs = cell
            .get_mut("outputs")
            .ok_or_else(|| bad("missing \"outputs\""))?;
        for output in items_mut(outputs, "outputs").map_err(in_cell(index))? {
            let output = object_mut(output, "output").map_err(in_cell(index))?;
            split_output(output).map_err(in_cell(index))?;
        }
    }
    Ok(())
}

fn split_output(output: &mut Map) -> Result<(), NbformatError> {
    match output.get("output_type") {
        None => Err(invalid("missing output_type")),
        Some(Value::List(_) | Value::Object(_)) => Err(invalid("unhashable output_type")),
        Some(Value::Str(t)) if t == "execute_result" || t == "display_data" => {
            if let Some(data) = output.get_mut("data") {
                split_mimebundle(object_mut(data, "output data")?);
            }
            Ok(())
        }
        Some(Value::Str(t)) if t == "stream" => {
            match output.get_mut("text") {
                None => return Err(invalid("stream output without text")),
                Some(text) => {
                    if let Value::Str(s) = text {
                        *text = split_lines_value(s);
                    }
                }
            }
            Ok(())
        }
        Some(_) => Ok(()),
    }
}

/// The part of `nbformat.validate` that changes the notebook: id repair for nbformat >= 4.5.
///
/// `writing` is true when the cells are still in their in-memory form (see [`repair_ids`]).
fn validate(nb: &mut Map, minor: i64, writing: bool) -> Result<(), NbformatError> {
    if minor >= 5 {
        repair_ids(cells_mut(nb)?, writing)?;
    }
    Ok(())
}

/// Gives cells without an id one, and cells whose id repeats an earlier one a new one.
///
/// Upstream draws random ids; these are derived from the cell's content (findings F-003), so an
/// unchanged cell keeps its id wherever it sits and diffs of two such notebooks show no
/// spurious id changes.
///
/// The hash input is the cell as `reads` returns it. When `writing`, the cells have not been
/// rejoined and stripped yet, so that is done on a copy first; the id then does not depend on how
/// the caller spelled `source` or whether `trusted` was set, and equals the id `reads` would
/// generate for the written file.
fn repair_ids(cells: &mut [Value], writing: bool) -> Result<(), NbformatError> {
    let mut ids = IdGenerator::new(writing);
    let mut missing = Vec::new();
    for (index, cell) in cells.iter().enumerate() {
        let cell = cell
            .as_object()
            .ok_or_else(|| invalid(format!("cell {index} must be a JSON object")))?;
        match cell.get("id") {
            None => missing.push(index),
            Some(Value::Str(id)) => {
                ids.taken.insert(id.clone());
            }
            // Upstream keeps ids in a set, which cannot hold a list or dict.
            Some(Value::List(_) | Value::Object(_)) => {
                return Err(invalid(format!("cell {index}: unhashable id")))
            }
            Some(_) => {}
        }
    }

    for index in missing {
        let id = ids.generate(&cells[index]);
        if let Value::Object(cell) = &mut cells[index] {
            cell.insert("id".into(), Value::Str(id));
        }
    }

    let mut seen = HashSet::new();
    let mut repeated = Vec::new();
    for (index, cell) in cells.iter().enumerate() {
        // Python set semantics: `1`, `1.0` and `true` are the same id.
        if let Some(id) = cell.get("id").and_then(Value::as_hashable) {
            if !seen.insert(id) {
                repeated.push(index);
            }
        }
    }
    drop(seen);

    for index in repeated {
        let Value::Object(cell) = &mut cells[index] else {
            continue;
        };
        let Some((position, _, _)) = cell.shift_remove_full("id") else {
            continue;
        };
        let id = ids.generate(&cells[index]);
        if let Value::Object(cell) = &mut cells[index] {
            cell.shift_insert(position, "id".into(), Value::Str(id));
        }
    }
    Ok(())
}

/// Derives cell ids from cell content.
///
/// An id is the first 8 hex digits of the 64-bit FNV-1a hash of the cell's canonical JSON
/// (compact, keys sorted, ASCII escapes). When that id is taken, the hash is continued with the
/// bytes `:1`, `:2`, ... until it is not.
struct IdGenerator {
    /// Every string id in the notebook, explicit or generated.
    taken: HashSet<String>,
    /// The next suffix to try for a content hash. The suffix sequence depends on the hash
    /// state alone and `taken` only grows, so suffixes before this one are known to be taken.
    next_suffix: HashMap<u64, u32>,
    writing: bool,
    options: DumpOptions,
    buffer: String,
    #[cfg(test)]
    probes: usize,
}

impl IdGenerator {
    fn new(writing: bool) -> Self {
        IdGenerator {
            taken: HashSet::new(),
            next_suffix: HashMap::new(),
            writing,
            options: DumpOptions {
                sort_keys: true,
                ..DumpOptions::compact()
            },
            buffer: String::new(),
            #[cfg(test)]
            probes: 0,
        }
    }

    /// An id for `cell` that is not taken yet, which is then taken.
    fn generate(&mut self, cell: &Value) -> String {
        let content = self.hash_content(cell);
        let mut suffix = self.next_suffix.get(&content).copied().unwrap_or(0);
        loop {
            #[cfg(test)]
            {
                self.probes += 1;
            }
            let hash = if suffix == 0 {
                content
            } else {
                fnv1a64_extend(fnv1a64_extend(content, b":"), suffix.to_string().as_bytes())
            };
            let id = format!("{:08x}", hash >> 32);
            if self.taken.insert(id.clone()) {
                self.next_suffix.insert(content, suffix + 1);
                return id;
            }
            suffix += 1;
        }
    }

    fn hash_content(&mut self, cell: &Value) -> u64 {
        self.buffer.clear();
        if self.writing {
            json::write_value(&mut self.buffer, &as_read_back(cell), &self.options);
        } else {
            json::write_value(&mut self.buffer, cell, &self.options);
        }
        fnv1a64(&self.buffer)
    }
}

/// The cell as `reads` would return the file `writes` makes from it.
///
/// Best effort: a cell that cannot be rejoined cannot be read back either, and then any
/// deterministic form will do.
fn as_read_back(cell: &Value) -> Value {
    let mut cell = cell.clone();
    if let Value::Object(map) = &mut cell {
        let _ = rejoin_cell(map, 0);
        if let Some(Value::Object(metadata)) = map.get_mut("metadata") {
            metadata.shift_remove("trusted");
        }
    }
    cell
}

fn fnv1a64(text: &str) -> u64 {
    fnv1a64_extend(FNV_OFFSET_BASIS, text.as_bytes())
}

fn fnv1a64_extend(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Value {
        json::parse(text).unwrap()
    }

    fn read(text: &str) -> Value {
        reads(text).unwrap()
    }

    fn notebook(cells: &str) -> String {
        format!(r#"{{"nbformat": 4, "nbformat_minor": 4, "metadata": {{}}, "cells": {cells}}}"#)
    }

    fn cell_ids(nb: &Value) -> Vec<String> {
        nb.get("cells")
            .and_then(Value::as_list)
            .unwrap()
            .iter()
            .map(|c| c.get("id").and_then(Value::as_str).unwrap().to_owned())
            .collect()
    }

    #[test]
    fn reads_rejoins_source_and_outputs() {
        let nb = read(&notebook(
            r#"[{"cell_type": "code", "metadata": {}, "source": ["a\n", "b"], "execution_count": null,
                "outputs": [
                  {"output_type": "stream", "name": "stdout", "text": ["x\n", "y"]},
                  {"output_type": "error", "ename": "E", "evalue": "v", "text": ["t", "u"], "traceback": []},
                  {"output_type": "execute_result", "execution_count": 1, "metadata": {},
                   "data": {"text/plain": ["p\n", "q"], "application/json": ["keep"],
                            "application/x+json": ["keep"], "text/x": [1, "no"], "empty": []}},
                  {"output_type": "display_data", "metadata": {}, "text": ["not\n", "joined"], "data": {}}]},
               {"cell_type": "markdown", "metadata": {}, "source": "s",
                "attachments": {"a.png": {"image/png": ["Zm9v", "YmFy"]}},
                "outputs": [{"output_type": "stream", "text": ["no", "join"]}]}]"#,
        ));
        let cells = nb.get("cells").unwrap().as_list().unwrap();
        assert_eq!(cells[0].get("source"), Some(&Value::from("a\nb")));
        let outputs = cells[0].get("outputs").unwrap().as_list().unwrap();
        assert_eq!(outputs[0].get("text"), Some(&Value::from("x\ny")));
        assert_eq!(outputs[1].get("text"), Some(&Value::from("tu")));
        let data = outputs[2].get("data").unwrap();
        assert_eq!(data.get("text/plain"), Some(&Value::from("p\nq")));
        assert_eq!(data.get("application/json"), Some(&parse(r#"["keep"]"#)));
        assert_eq!(data.get("application/x+json"), Some(&parse(r#"["keep"]"#)));
        assert_eq!(data.get("text/x"), Some(&parse(r#"[1, "no"]"#)));
        assert_eq!(data.get("empty"), Some(&Value::from("")));
        assert_eq!(
            outputs[3].get("text"),
            Some(&parse(r#"["not\n", "joined"]"#))
        );
        let attachment = cells[1].get("attachments").unwrap().get("a.png").unwrap();
        assert_eq!(attachment.get("image/png"), Some(&Value::from("Zm9vYmFy")));
        let markdown_outputs = cells[1].get("outputs").unwrap().as_list().unwrap();
        assert_eq!(
            markdown_outputs[0].get("text"),
            Some(&parse(r#"["no", "join"]"#))
        );
    }

    #[test]
    fn reads_strips_transient_metadata_keeping_order() {
        let nb = read(
            r#"{"nbformat": 4, "nbformat_minor": 4, "cells": [
                {"cell_type": "raw", "metadata": {"a": 1, "trusted": true, "b": 2}, "source": ""}],
                "metadata": {"z": 1, "signature": "s", "orig_nbformat": 3, "orig_nbformat_minor": 0, "a": 2}}"#,
        );
        assert_eq!(nb.get("metadata"), Some(&parse(r#"{"z": 1, "a": 2}"#)));
        let cell = &nb.get("cells").unwrap().as_list().unwrap()[0];
        assert_eq!(cell.get("metadata"), Some(&parse(r#"{"a": 1, "b": 2}"#)));
    }

    #[test]
    fn reads_ignores_one_bom() {
        let text = notebook("[]");
        assert_eq!(read(&format!("\u{feff}{text}")), read(&text));
        assert!(reads(&format!("\u{feff}\u{feff}{text}")).is_err());
    }

    #[test]
    fn reads_rejects_other_versions() {
        for (text, expected) in [
            (r#"{"nbformat": 3, "metadata": {}}"#, "nbformat 3"),
            (r#"{"nbformat": 5, "nbformat_minor": 0}"#, "nbformat 5"),
            (
                r#"{"nbformat": 4.0, "metadata": {}, "cells": []}"#,
                "nbformat 4.0",
            ),
            (r#"{"nbformat": true}"#, "nbformat true"),
            (r#"{"metadata": {}, "cells": []}"#, "no nbformat"),
        ] {
            match reads(text) {
                Err(NbformatError::UnsupportedVersion { found, .. }) => {
                    assert!(found.contains(expected), "{found}")
                }
                other => panic!("{text}: {other:?}"),
            }
        }
        let message = |text: &str| reads(text).unwrap_err().to_string();
        let convert = "jupyter nbconvert --to notebook";
        for older in [
            r#"{"nbformat": 1}"#,
            r#"{"nbformat": 2}"#,
            r#"{"nbformat": 3}"#,
        ] {
            assert!(message(older).contains(convert), "{older}");
        }
        for other in [
            r#"{"nbformat": 5, "nbformat_minor": 0}"#,
            r#"{"nbformat": 4.0}"#,
            r#"{"nbformat": "4"}"#,
            r#"{"nbformat": true}"#,
            r#"{"nbformat": 0}"#,
            r#"{}"#,
        ] {
            let message = message(other);
            assert!(!message.contains(convert), "{other}: {message}");
            assert!(!message.contains("older"), "{other}: {message}");
        }
        assert!(message(r#"{"nbformat": 4.0}"#).contains("must be the integer 4"));
        assert!(message(r#"{"nbformat": 5}"#).contains("newer"));
    }

    #[test]
    fn reads_rejects_malformed_notebooks() {
        let missing_cells = r#"{"nbformat": 4, "nbformat_minor": 4, "metadata": {}}"#;
        let missing_metadata = r#"{"nbformat": 4, "nbformat_minor": 4, "cells": []}"#;
        for text in [
            "",
            "[]",
            "null",
            "{",
            missing_cells,
            missing_metadata,
            &notebook(r#"[{"cell_type": "raw", "source": ""}]"#),
            &notebook(r#"[1]"#),
            &notebook(r#"[[]]"#),
            &notebook("null"),
            &notebook(r#"{"a": 1}"#),
            &notebook(r#""x""#),
            &notebook(r#"[{"metadata": {}, "source": [1]}]"#),
            &notebook(r#"[{"metadata": [], "source": ""}]"#),
            &notebook(r#"[{"metadata": {}, "attachments": []}]"#),
            &notebook(r#"[{"metadata": {}, "attachments": {"a": []}}]"#),
            &notebook(r#"[{"cell_type": "code", "metadata": {}, "outputs": [1]}]"#),
            &notebook(r#"[{"cell_type": "code", "metadata": {}, "outputs": null}]"#),
            &notebook(
                r#"[{"cell_type": "code", "metadata": {}, "outputs": [{"output_type": []}]}]"#,
            ),
            &notebook(
                r#"[{"cell_type": "code", "metadata": {}, "outputs": [{"output_type": "display_data", "data": null}]}]"#,
            ),
            &notebook(
                r#"[{"cell_type": "code", "metadata": {}, "outputs": [{"output_type": "stream", "text": ["a", 1]}]}]"#,
            ),
            r#"{"nbformat": 4, "nbformat_minor": 4, "metadata": [], "cells": []}"#,
            r#"{"nbformat": 4, "nbformat_minor": 1.5, "metadata": {}, "cells": []}"#,
            r#"{"nbformat": 4, "nbformat_minor": null, "metadata": {}, "cells": []}"#,
        ] {
            assert!(reads(text).is_err(), "{text}");
        }
    }

    #[test]
    fn errors_name_the_cell_and_keep_the_detail() {
        let error = |text: String| reads(&text).unwrap_err().to_string();
        let output = |output: &str| {
            notebook(&format!(
                r#"[{{"cell_type": "raw", "metadata": {{}}}}, {{"cell_type": "code", "metadata": {{}}, "outputs": [{output}]}}]"#
            ))
        };
        let message = error(output(r#"{"output_type": "stream", "text": ["a", 1]}"#));
        assert!(
            message.contains("cell 1") && message.contains("text lines must be strings"),
            "{message}"
        );
        let message = error(output("1"));
        assert!(
            message.contains("cell 1") && message.contains("output must be a JSON object"),
            "{message}"
        );
        let message = error(output(r#"{"output_type": "display_data", "data": []}"#));
        assert!(
            message.contains("cell 1") && message.contains("output data must be a JSON object"),
            "{message}"
        );
        let message = error(notebook(
            r#"[{"cell_type": "raw", "metadata": {}, "attachments": {"a": 1}}]"#,
        ));
        assert!(
            message.contains("cell 0") && message.contains("attachment must be a JSON object"),
            "{message}"
        );
    }

    #[test]
    fn reads_accepts_oddly_shaped_but_iterable_values() {
        for cells in ["{}", r#""""#, "[]"] {
            let nb = read(&notebook(cells));
            assert_eq!(nb.get("cells"), Some(&parse(cells)));
        }
        let nb = read(&notebook(
            r#"[{"cell_type": "code", "metadata": {}, "outputs": {}}, {"cell_type": "x", "metadata": {}, "outputs": 5}]"#,
        ));
        assert_eq!(nb.get("cells").unwrap().as_list().unwrap().len(), 2);
        // Minor versions upstream treats as integers.
        for minor in ["true", "0", "-3", "99999999999999999999999"] {
            let text = format!(
                r#"{{"nbformat": 4, "nbformat_minor": {minor}, "metadata": {{}}, "cells": []}}"#
            );
            assert!(reads(&text).is_ok(), "{minor}");
        }
    }

    #[test]
    fn repairs_missing_and_duplicate_ids_deterministically() {
        let text = r#"{"nbformat": 4, "nbformat_minor": 5, "metadata": {}, "cells": [
            {"cell_type": "markdown", "metadata": {}, "source": "same"},
            {"cell_type": "markdown", "metadata": {}, "id": "dup", "source": "one"},
            {"cell_type": "markdown", "metadata": {}, "source": "same"},
            {"cell_type": "markdown", "metadata": {}, "id": "dup", "source": "two"},
            {"cell_type": "markdown", "metadata": {}, "id": 1, "source": "n"},
            {"cell_type": "markdown", "metadata": {}, "id": 1.0, "source": "m"}]}"#;
        let nb = read(text);
        let cells = nb.get("cells").unwrap().as_list().unwrap();
        let ids: Vec<&Value> = cells.iter().map(|c| c.get("id").unwrap()).collect();
        let is_generated = |v: &Value| {
            v.as_str()
                .is_some_and(|s| s.len() == 8 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        };
        assert!(is_generated(ids[0]) && is_generated(ids[2]));
        assert_ne!(ids[0], ids[2], "identical cells still get distinct ids");
        assert_eq!(ids[1], &Value::from("dup"));
        assert!(is_generated(ids[3]), "the later duplicate is replaced");
        assert_eq!(ids[4], &Value::from(1));
        assert!(is_generated(ids[5]), "1.0 repeats 1");
        // Generated ids are appended to the cell, replaced ones keep their position.
        let keys = |c: &Value| c.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
        assert_eq!(keys(&cells[0]), ["cell_type", "metadata", "source", "id"]);
        assert_eq!(keys(&cells[3]), ["cell_type", "metadata", "id", "source"]);
        assert_eq!(read(text), nb);
    }

    #[test]
    fn generated_ids_depend_only_on_content() {
        let cell = r##"{"cell_type": "markdown", "metadata": {}, "source": "# same"}"##;
        let other = r#"{"cell_type": "markdown", "metadata": {}, "source": "x"}"#;
        let with = |cells: &str| {
            format!(
                r#"{{"nbformat": 4, "nbformat_minor": 5, "metadata": {{}}, "cells": [{cells}]}}"#
            )
        };
        let a = cell_ids(&read(&with(cell)));
        let b = cell_ids(&read(&with(&format!("{other}, {cell}"))));
        assert_eq!(a[0], b[1]);
        assert_ne!(b[0], b[1]);
    }

    #[test]
    fn ids_are_only_repaired_from_4_5() {
        let nb = read(&notebook(
            r#"[{"cell_type": "raw", "metadata": {}, "source": ""}]"#,
        ));
        assert!(nb.get("cells").unwrap().as_list().unwrap()[0]
            .get("id")
            .is_none());
        let unhashable = r#"{"nbformat": 4, "nbformat_minor": 5, "metadata": {}, "cells":
            [{"cell_type": "raw", "metadata": {}, "source": "", "id": []}]}"#;
        assert!(reads(unhashable).is_err());
    }

    #[test]
    fn generated_ids_avoid_ids_anywhere_in_the_notebook() {
        let cell = r#"{"cell_type": "raw", "metadata": {}, "source": ""}"#;
        let probe = |extra: &str| {
            format!(
                r#"{{"nbformat": 4, "nbformat_minor": 5, "metadata": {{}}, "cells": [{cell}{extra}]}}"#
            )
        };
        let natural = cell_ids(&read(&probe("")))[0].clone();
        let explicit = format!(
            r#", {{"cell_type": "raw", "metadata": {{}}, "source": "x", "id": "{natural}"}}"#
        );
        let ids = cell_ids(&read(&probe(&explicit)));
        assert_eq!(ids[1], natural);
        assert_ne!(ids[0], natural);
    }

    #[test]
    fn generated_ids_are_stable() {
        // Computed from the rule in docs/process/u02-nbformat.md, independently of this code.
        let same = r#"{"cell_type": "markdown", "metadata": {}, "source": "same"}"#;
        let code = r#"{"cell_type": "code", "execution_count": null, "metadata": {},
                       "outputs": [], "source": ""}"#;
        let raw = r#"{"cell_type": "raw", "metadata": {}, "source": "é\u2028"}"#;
        let nb = read(&format!(
            r#"{{"nbformat": 4, "nbformat_minor": 5, "metadata": {{}},
                "cells": [{same}, {same}, {same}, {same}, {code}, {raw}]}}"#
        ));
        assert_eq!(
            cell_ids(&nb),
            ["ec9f074f", "d2a9c719", "d2a9c619", "d2a9c519", "729e0f33", "0fb51b3c"]
        );

        let dup = r#"{"cell_type": "raw", "metadata": {}, "source": "q", "id": "x"}"#;
        let nb = read(&format!(
            r#"{{"nbformat": 4, "nbformat_minor": 5, "metadata": {{}}, "cells": [{dup}, {dup}, {dup}]}}"#
        ));
        assert_eq!(cell_ids(&nb), ["x", "0ee374e3", "42b8d166"]);
    }

    #[test]
    fn identical_cells_do_not_rescan_taken_suffixes() {
        let cell = parse(r#"{"cell_type": "raw", "metadata": {}, "source": ""}"#);
        let count = 20_000;
        let mut ids = IdGenerator::new(false);
        let generated: HashSet<String> = (0..count).map(|_| ids.generate(&cell)).collect();
        assert_eq!(generated.len(), count);
        // One probe per id, plus a few retries when two ids collide in their first 32 bits.
        assert!(ids.probes < count + count / 100, "{} probes", ids.probes);
    }

    #[test]
    fn many_identical_cells_get_distinct_ids() {
        let missing = r#"{"cell_type": "raw", "metadata": {}, "source": ""}"#;
        let same = r#"{"cell_type": "raw", "metadata": {}, "source": "", "id": "same"}"#;
        for cell in [missing, same] {
            let cells = vec![cell; 20_000].join(",");
            let nb = read(&format!(
                r#"{{"nbformat": 4, "nbformat_minor": 5, "metadata": {{}}, "cells": [{cells}]}}"#
            ));
            let ids = cell_ids(&nb);
            assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        }
    }

    #[test]
    fn writes_ids_do_not_depend_on_how_the_cell_is_spelled() {
        let written_id = |cell: &str| {
            let nb = parse(&format!(
                r#"{{"nbformat": 4, "nbformat_minor": 5, "metadata": {{}}, "cells": [{cell}]}}"#
            ));
            let text = writes(&nb).unwrap();
            // What the file reads back as, and what the unwritten notebook reads as.
            let written = cell_ids(&reads(&text).unwrap());
            let direct = cell_ids(&reads(&json::dumps(&nb, &DumpOptions::compact())).unwrap());
            assert_eq!(written, direct, "{cell}");
            written.into_iter().next().unwrap()
        };
        let string = written_id(r#"{"cell_type": "raw", "metadata": {}, "source": "s"}"#);
        let list = written_id(r#"{"cell_type": "raw", "metadata": {}, "source": ["s"]}"#);
        let trusted =
            written_id(r#"{"cell_type": "raw", "metadata": {"trusted": true}, "source": "s"}"#);
        assert_eq!(string, list);
        assert_eq!(string, trusted);
    }

    #[test]
    fn reads_of_written_notebooks_keep_the_ids_writes_generated() {
        let cell =
            r#"{"cell_type": "markdown", "metadata": {"trusted": true}, "source": ["a\n", "b"]}"#;
        let dup = r#"{"cell_type": "markdown", "metadata": {}, "source": "a\nb", "id": "d"}"#;
        let nb = parse(&format!(
            r#"{{"nbformat": 4, "nbformat_minor": 5, "metadata": {{}}, "cells": [{cell}, {cell}, {dup}, {dup}]}}"#
        ));
        let text = writes(&nb).unwrap();
        let ids = cell_ids(&reads(&text).unwrap());
        let mut distinct = ids.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), 4, "{ids:?}");
        assert_eq!(ids[2], "d");
        // Writing again changes nothing: the ids are already in the file.
        assert_eq!(writes(&reads(&text).unwrap()).unwrap(), text);
    }

    #[test]
    fn writes_splits_sorts_and_indents() {
        let nb = parse(
            r#"{"nbformat": 4, "nbformat_minor": 4, "metadata": {"signature": "x", "k": {"b": 1, "a": 2}},
                "cells": [{"cell_type": "code", "metadata": {"trusted": true}, "source": "a\nb\r\nc\u2028d",
                  "execution_count": 1, "outputs": [
                    {"output_type": "stream", "name": "stdout", "text": "x\ny"},
                    {"output_type": "display_data", "metadata": {}, "data": {"text/plain": "p\nq",
                      "Text/Plain": "no\nsplit", "image/svg+xml": "<a>\n</a>", "image/png": "a\nb",
                      "application/javascript": "j\nk", "application/json": "j\nk"}}]},
                  {"cell_type": "markdown", "metadata": {}, "source": "é😀", "attachments": {"a": {"text/x": "1\n2"}}}]}"#,
        );
        let text = writes(&nb).unwrap();
        let expected = r#"{
 "cells": [
  {
   "cell_type": "code",
   "execution_count": 1,
   "metadata": {},
   "outputs": [
    {
     "name": "stdout",
     "output_type": "stream",
     "text": [
      "x\n",
      "y"
     ]
    },
    {
     "data": {
      "Text/Plain": "no\nsplit",
      "application/javascript": [
       "j\n",
       "k"
      ],
      "application/json": "j\nk",
      "image/png": "a\nb",
      "image/svg+xml": [
       "<a>\n",
       "</a>"
      ],
      "text/plain": [
       "p\n",
       "q"
      ]
     },
     "metadata": {},
     "output_type": "display_data"
    }
   ],
   "source": [
    "a\n",
    "b\r\n",
    "c\u2028",
    "d"
   ]
  },
  {
   "attachments": {
    "a": {
     "text/x": [
      "1\n",
      "2"
     ]
    }
   },
   "cell_type": "markdown",
   "metadata": {},
   "source": [
    "é😀"
   ]
  }
 ],
 "metadata": {
  "k": {
   "a": 2,
   "b": 1
  }
 },
 "nbformat": 4,
 "nbformat_minor": 4
}
"#;
        // U+2028 is written raw, not as an escape: ensure_ascii is off. `expected` spells it \u2028.
        assert!(!text.contains("\\u2028"));
        assert_eq!(text, expected.replace("\\u2028", "\u{2028}"));
        assert_eq!(
            reads(&text)
                .unwrap()
                .get("cells")
                .unwrap()
                .as_list()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn writes_does_not_touch_its_input_and_repairs_ids() {
        let nb = parse(
            r#"{"nbformat": 4, "nbformat_minor": 5, "metadata": {}, "cells":
               [{"cell_type": "raw", "metadata": {}, "source": "a"}]}"#,
        );
        let before = nb.clone();
        let text = writes(&nb).unwrap();
        assert_eq!(nb, before);
        let id = cell_ids(&reads(&text).unwrap()).remove(0);
        assert_eq!(id.len(), 8);
        assert_eq!(writes(&nb).unwrap(), text);
    }

    #[test]
    fn writes_rejects_what_upstream_cannot_write() {
        let cell = |extra: &str| {
            parse(&notebook(&format!(
                r#"[{{"metadata": {{}}, "source": "", {extra}}}]"#
            )))
        };
        for nb in [
            parse("[]"),
            parse(r#"{"nbformat": 3}"#),
            parse(r#"{"nbformat": 4, "nbformat_minor": 4, "metadata": {}}"#),
            cell(r#""x": 1"#),
            cell(r#""cell_type": "code""#),
            cell(r#""cell_type": "code", "outputs": [{}]"#),
            cell(r#""cell_type": "code", "outputs": [{"output_type": "stream"}]"#),
            cell(r#""cell_type": "markdown", "attachments": 3"#),
        ] {
            assert!(writes(&nb).is_err(), "{nb:?}");
        }
        assert!(writes(&cell(r#""cell_type": "markdown""#)).is_ok());
        assert!(writes(&cell(r#""cell_type": "code", "outputs": []"#)).is_ok());
        assert!(writes(&cell(
            r#""cell_type": "code", "outputs": [{"output_type": "error"}]"#
        ))
        .is_ok());
    }

    #[test]
    fn new_notebook_has_nbformat_key_order() {
        let text = json::dumps(&new_notebook(), &DumpOptions::compact());
        assert_eq!(
            text,
            r#"{"nbformat":4,"nbformat_minor":5,"metadata":{},"cells":[]}"#
        );
    }

    #[test]
    fn fnv1a64_known_values() {
        assert_eq!(fnv1a64(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64("foobar"), 0x8594_4171_f739_67e8);
    }
}
