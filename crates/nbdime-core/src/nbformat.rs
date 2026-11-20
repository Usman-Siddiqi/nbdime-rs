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

use std::collections::HashSet;

use crate::json::{self, DumpOptions, ParseError};
use crate::py::splitlines_keepends;
use crate::value::{Int, Map, Value};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum NbformatError {
    #[error("notebook is not valid JSON: {0}")]
    Json(ParseError),
    #[error(
        "unsupported notebook format ({found}): only nbformat 4 notebooks are supported; \
         upgrade older notebooks first, for example with `jupyter nbconvert --to notebook`"
    )]
    UnsupportedVersion { found: String },
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

/// `nbformat.reads(text, as_version=4)`.
pub fn reads(text: &str) -> Result<Value, NbformatError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut nb = json::parse(text)?;
    let minor = version_minor(&nb)?;
    let map = object_mut(&mut nb, "notebook")?;
    rejoin_lines(map)?;
    strip_transient(map)?;
    validate(map, minor)?;
    Ok(nb)
}

/// The text `nbformat.write(nb, fp)` writes for a v4 notebook.
pub fn writes(nb: &Value) -> Result<String, NbformatError> {
    let minor = version_minor(nb)?;
    let mut nb = nb.clone();
    let map = object_mut(&mut nb, "notebook")?;
    validate(map, minor)?;
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
            let shown = json::dumps(other, &DumpOptions::compact());
            return Err(NbformatError::UnsupportedVersion {
                found: format!("nbformat {shown}"),
            });
        }
        None => {
            return Err(NbformatError::UnsupportedVersion {
                found: "no nbformat version".into(),
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
                rejoin_mimebundle(object_mut(attachment, "attachment")?);
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
    for output in items_mut(outputs, "outputs").map_err(|_| bad("outputs must be a list"))? {
        let output = object_mut(output, "output").map_err(|_| bad("output must be an object"))?;
        rejoin_output(output).map_err(|_| bad("malformed output"))?;
    }
    Ok(())
}

fn rejoin_output(output: &mut Map) -> Result<(), NbformatError> {
    enum Kind {
        Rich,
        Other { present: bool },
    }
    let kind = match output.get("output_type") {
        Some(Value::Str(t)) if t == "execute_result" || t == "display_data" => Kind::Rich,
        // Upstream tests membership in a set, which cannot hold a list or dict.
        Some(Value::List(_) | Value::Object(_)) => return Err(invalid("unhashable output_type")),
        Some(t) => Kind::Other {
            present: t.truthy(),
        },
        None => Kind::Other { present: false },
    };
    match kind {
        Kind::Rich => {
            if let Some(data) = output.get_mut("data") {
                rejoin_mimebundle(object_mut(data, "output data")?);
            }
        }
        Kind::Other { present: true } => {
            if let Some(text @ Value::List(_)) = output.get_mut("text") {
                let Value::List(lines) = &*text else {
                    unreachable!("matched a list")
                };
                let joined =
                    join_strings(lines).ok_or_else(|| invalid("text lines must be strings"))?;
                *text = Value::Str(joined);
            }
        }
        Kind::Other { present: false } => {}
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
        if let Some(source @ Value::Str(_)) = cell.get_mut("source") {
            let Value::Str(text) = &*source else {
                unreachable!("matched a string")
            };
            *source = split_lines_value(text);
        }
        match cell.get_mut("attachments") {
            None => {}
            Some(Value::Object(attachments)) => {
                for attachment in attachments.values_mut() {
                    split_mimebundle(object_mut(attachment, "attachment")?);
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
        for output in items_mut(outputs, "outputs").map_err(|_| bad("outputs must be a list"))? {
            let output =
                object_mut(output, "output").map_err(|_| bad("output must be an object"))?;
            split_output(output).map_err(|_| bad("malformed output"))?;
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
                Some(text @ Value::Str(_)) => {
                    let Value::Str(s) = &*text else {
                        unreachable!("matched a string")
                    };
                    *text = split_lines_value(s);
                }
                Some(_) => {}
            }
            Ok(())
        }
        Some(_) => Ok(()),
    }
}

/// The part of `nbformat.validate` that changes the notebook: id repair for nbformat >= 4.5.
fn validate(nb: &mut Map, minor: i64) -> Result<(), NbformatError> {
    if minor >= 5 {
        repair_ids(cells_mut(nb)?)?;
    }
    Ok(())
}

/// Gives cells without an id one, and cells whose id repeats an earlier one a new one.
///
/// Upstream draws random ids; these are derived from the cell's content (findings F-003), so an
/// unchanged cell keeps its id wherever it sits and diffs of two such notebooks show no
/// spurious id changes.
fn repair_ids(cells: &mut [Value]) -> Result<(), NbformatError> {
    let mut taken: HashSet<String> = HashSet::new();
    let mut missing = Vec::new();
    for (index, cell) in cells.iter().enumerate() {
        let cell = cell
            .as_object()
            .ok_or_else(|| invalid(format!("cell {index} must be a JSON object")))?;
        match cell.get("id") {
            None => missing.push(index),
            Some(Value::Str(id)) => {
                taken.insert(id.clone());
            }
            // Upstream keeps ids in a set, which cannot hold a list or dict.
            Some(Value::List(_) | Value::Object(_)) => {
                return Err(invalid(format!("cell {index}: unhashable id")))
            }
            Some(_) => {}
        }
    }

    for index in missing {
        let id = generate_id(&canonical_json(&cells[index]), &mut taken);
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
        let id = generate_id(&canonical_json(&cells[index]), &mut taken);
        if let Value::Object(cell) = &mut cells[index] {
            cell.shift_insert(position, "id".into(), Value::Str(id));
        }
    }
    Ok(())
}

/// Compact JSON with sorted keys and ASCII escapes: the hash input for generated ids.
fn canonical_json(cell: &Value) -> String {
    let options = DumpOptions {
        sort_keys: true,
        ..DumpOptions::compact()
    };
    json::dumps(cell, &options)
}

/// The first 8 hex digits of the 64-bit FNV-1a hash of `content`, extended with `:1`, `:2`, ...
/// until the id is not already taken. The chosen id is added to `taken`.
fn generate_id(content: &str, taken: &mut HashSet<String>) -> String {
    let mut attempt = 0u32;
    loop {
        let hash = if attempt == 0 {
            fnv1a64(content)
        } else {
            fnv1a64(&format!("{content}:{attempt}"))
        };
        let id = format!("{:08x}", hash >> 32);
        if taken.insert(id.clone()) {
            return id;
        }
        attempt += 1;
    }
}

fn fnv1a64(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

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
                Err(NbformatError::UnsupportedVersion { found }) => {
                    assert!(found.contains(expected), "{found}")
                }
                other => panic!("{text}: {other:?}"),
            }
        }
        let message = reads(r#"{"nbformat": 2}"#).unwrap_err().to_string();
        assert!(
            message.contains("jupyter nbconvert --to notebook"),
            "{message}"
        );
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
    fn writes_splits_sorts_and_indents() {
        let nb = parse(
            r#"{"nbformat": 4, "nbformat_minor": 4, "metadata": {"signature": "x", "k": {"b": 1, "a": 2}},
                "cells": [{"cell_type": "code", "metadata": {"trusted": true}, "source": "a\nb\r\nc d",
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
    "c ",
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
        // ` ` is written raw: ensure_ascii is off.
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
