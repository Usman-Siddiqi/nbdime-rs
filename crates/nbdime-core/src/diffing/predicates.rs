//! The notebook predicates that decide which cells and outputs are "the same" when aligning
//! two notebooks.
//!
//! Predicates run on [`Item`]s: a value together with what the predicates derive from it (code
//! point counts and indexes of its strings, lowercased MIME types, base64 detection), computed
//! the first time a predicate needs it and kept for every later pair. Predicates read values in
//! the order upstream does, so that a missing key or an operand of the wrong type is an error
//! exactly where upstream raises.

use std::cell::OnceCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::diff_format::Op;
use crate::value::{Map, Value};

use super::approximate::{compare_values_approximate, PreparedStr};
use super::config::{index_listed, retain_unlisted_keys, DiffConfig, Differ};
use super::generic::multilevel_snakes;
use super::notebooks::OUTPUTS_PATH;
use super::snakes::Snake;
use super::DiffError;

const TEXT_MIMEDATA_MAX_COMPARE_LENGTH: usize = 10_000;
const STREAM_MAX_COMPARE_LENGTH: usize = 1_000;

/// Texts shorter than this (both of them) always align under the approximate text predicate.
const SHORT_TEXT: usize = 10;
const APPROXIMATE: f64 = 0.7;
const STRICT: f64 = 0.95;

/// A sequence item and the data derived from it for predicate evaluation.
pub(super) struct Item<'a> {
    value: &'a Value,
    /// `value["id"]` of an object that has one.
    id: Option<&'a Value>,
    /// A hash of `id` when it is a string, so that most pairs of different ids are told apart
    /// without comparing them.
    id_hash: Option<u64>,
    cell: OnceCell<PreparedCell<'a>>,
    output: OnceCell<PreparedOutput<'a>>,
}

impl<'a> Item<'a> {
    pub(super) fn new(value: &'a Value) -> Self {
        let id = value.get("id");
        Item {
            value,
            id,
            id_hash: id.and_then(Value::as_str).map(|id| {
                let mut hasher = DefaultHasher::new();
                id.hash(&mut hasher);
                hasher.finish()
            }),
            cell: OnceCell::new(),
            output: OnceCell::new(),
        }
    }

    pub(super) fn value(&self) -> &'a Value {
        self.value
    }

    fn cell(&self) -> &PreparedCell<'a> {
        self.cell.get_or_init(|| PreparedCell::new(self.value))
    }

    fn output(&self) -> &PreparedOutput<'a> {
        self.output.get_or_init(|| PreparedOutput::new(self.value))
    }
}

struct PreparedCell<'a> {
    cell_type: Option<&'a Value>,
    source: Option<&'a Value>,
    source_text: Option<PreparedStr<'a>>,
    outputs_value: Option<&'a Value>,
    /// The items of `outputs`, when it is a list.
    outputs: Option<Vec<Item<'a>>>,
}

impl<'a> PreparedCell<'a> {
    fn new(cell: &'a Value) -> Self {
        let source = cell.get("source");
        let outputs_value = cell.get("outputs");
        PreparedCell {
            cell_type: cell.get("cell_type"),
            source,
            source_text: source.and_then(Value::as_str).map(PreparedStr::new),
            outputs_value,
            outputs: outputs_value
                .and_then(Value::as_list)
                .map(|items| items.iter().map(Item::new).collect()),
        }
    }

    fn cell_type(&self) -> Result<&'a Value, DiffError> {
        missing(self.cell_type, "cell_type")
    }

    fn source(&self) -> Result<Text<'a, '_>, DiffError> {
        Ok(Text {
            value: missing(self.source, "source")?,
            prepared: self.source_text.as_ref(),
        })
    }

    fn is_code(&self) -> Result<bool, DiffError> {
        Ok(self.cell_type()?.as_str() == Some("code"))
    }

    /// `cell["outputs"] or ()`.
    fn outputs(&self) -> Result<&[Item<'a>], DiffError> {
        if !missing(self.outputs_value, "outputs")?.truthy() {
            return Ok(&[]);
        }
        self.outputs
            .as_deref()
            .ok_or_else(|| type_error("outputs must be a list"))
    }
}

struct PreparedOutput<'a> {
    /// `output["text"]` of a stream.
    text: Option<PreparedStr<'a>>,
    /// One entry per member of `output["data"]`, in its order.
    bundle: Vec<MimeEntry<'a>>,
}

impl<'a> PreparedOutput<'a> {
    fn new(output: &'a Value) -> Self {
        PreparedOutput {
            text: output
                .get("text")
                .and_then(Value::as_str)
                .map(PreparedStr::new),
            bundle: output
                .get("data")
                .and_then(Value::as_object)
                .map(|data| data.iter().map(|(k, v)| MimeEntry::new(k, v)).collect())
                .unwrap_or_default(),
        }
    }
}

struct MimeEntry<'a> {
    mimetype: String,
    text: Option<PreparedStr<'a>>,
    base64: OnceCell<bool>,
    /// The text split around pointer values; `None` if it has several lines or is no string.
    pointer_pieces: OnceCell<Option<Vec<&'a str>>>,
}

impl<'a> MimeEntry<'a> {
    fn new(mimetype: &str, value: &'a Value) -> Self {
        MimeEntry {
            mimetype: mimetype.to_lowercase(),
            text: value.as_str().map(PreparedStr::new),
            base64: OnceCell::new(),
            pointer_pieces: OnceCell::new(),
        }
    }

    fn is_base64(&self) -> bool {
        *self.base64.get_or_init(|| {
            self.text
                .as_ref()
                .is_some_and(|text| is_base64(text.text(), text.len()))
        })
    }

    fn pointer_pieces(&self) -> Option<&[&'a str]> {
        self.pointer_pieces
            .get_or_init(|| {
                let text = self.text.as_ref()?.text();
                (!text.contains('\n')).then(|| split_pointers(text))
            })
            .as_deref()
    }
}

/// A value and, when it is a string, its prepared form.
#[derive(Clone, Copy)]
struct Text<'a, 'p> {
    value: &'a Value,
    prepared: Option<&'p PreparedStr<'a>>,
}

fn missing<T>(found: Option<T>, key: &str) -> Result<T, DiffError> {
    found.ok_or_else(|| DiffError::MissingKey(key.to_owned()))
}

fn type_error(message: &str) -> DiffError {
    DiffError::Type(message.to_owned())
}

/// `value[key]`.
fn field<'a>(map: &'a Map, key: &str) -> Result<&'a Value, DiffError> {
    missing(map.get(key), key)
}

fn object(value: &Value) -> Result<&Map, DiffError> {
    value
        .as_object()
        .ok_or_else(|| type_error("expected an output object"))
}

/// `set(x) == set(y)` for the keys of two dicts.
fn same_keys(x: &Map, y: &Map) -> bool {
    x.len() == y.len() && x.keys().all(|k| y.contains_key(k))
}

/// `compare_text_approximate`, which upstream also caches (and thereby hashes its arguments).
fn compare_text_approximate(
    x: Text<'_, '_>,
    y: Text<'_, '_>,
    maxlen: Option<usize>,
) -> Result<bool, DiffError> {
    let (Some(px), Some(py)) = (x.prepared, y.prepared) else {
        let unhashable = |v: &Value| matches!(v, Value::List(_) | Value::Object(_));
        if unhashable(x.value) || unhashable(y.value) {
            return Err(type_error("unhashable text operand"));
        }
        if x.value.truthy() != y.value.truthy() {
            return Ok(false);
        }
        return Err(type_error("text operands must be strings"));
    };
    if px.is_empty() != py.is_empty() {
        return Ok(false);
    }
    // Aligns tiny strings, such as a single number in an output, without comparing them.
    if px.len() < SHORT_TEXT && py.len() < SHORT_TEXT {
        return Ok(true);
    }
    Ok(px.is_similar(py, APPROXIMATE, maxlen))
}

fn compare_text_strict(
    x: Text<'_, '_>,
    y: Text<'_, '_>,
    maxlen: Option<usize>,
) -> Result<bool, DiffError> {
    match (x.prepared, y.prepared) {
        (Some(px), Some(py)) => Ok(px.is_similar(py, STRICT, maxlen)),
        _ => Err(type_error("text operands must be strings")),
    }
}

/// Whether `s` (of `len` code points) is base64 data, possibly broken into lines.
fn is_base64(s: &str, len: usize) -> bool {
    const MIN_LEN: usize = 64;
    if len < MIN_LEN {
        return false;
    }
    let mut scan = Base64Scan::default();
    for (at, &byte) in s.as_bytes().iter().enumerate() {
        if !byte.is_ascii() {
            return s[at..].chars().all(|c| scan.step(c)) && scan.is_complete();
        }
        if !scan.step(char::from(byte)) {
            return false;
        }
    }
    scan.is_complete()
}

/// The state of matching `^(?:[a-z0-9+/]{4})*(?:[a-z0-9+/]{2}==|[a-z0-9+/]{3}=)?$` (case
/// insensitive) against `''.join(s.splitlines())`, one character of `s` at a time.
#[derive(Default)]
struct Base64Scan {
    digits: usize,
    padding: usize,
}

impl Base64Scan {
    /// Takes the next character; false once the text can no longer match.
    #[inline]
    fn step(&mut self, c: char) -> bool {
        // The pattern is Unicode aware, so `[a-z]` also takes the characters that fold to ASCII
        // letters: dotted capital I, dotless i, long s and the Kelvin sign.
        let is_digit = c.is_ascii_alphanumeric()
            || matches!(
                c,
                '+' | '/' | '\u{130}' | '\u{131}' | '\u{17f}' | '\u{212a}'
            );
        if self.padding == 0 && is_digit {
            self.digits += 1;
        } else if c == '=' && self.padding < 2 {
            self.padding += 1;
        } else if !is_line_end(c) {
            return false;
        }
        true
    }

    fn is_complete(&self) -> bool {
        match self.padding {
            0 => self.digits % 4 == 0,
            1 => self.digits % 4 == 3,
            _ => self.digits % 4 == 2,
        }
    }
}

fn is_line_end(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r'
            | '\u{b}'
            | '\u{c}'
            | '\u{1c}'
            | '\u{1d}'
            | '\u{1e}'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// `re.compile(r"0x[a-f0-9]{8,16}", re.I).split(s)`.
fn split_pointers(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let (mut pieces, mut start, mut i) = (Vec::new(), 0, 0);
    while i + 1 < bytes.len() {
        if bytes[i] == b'0' && matches!(bytes[i + 1], b'x' | b'X') {
            let digits = bytes[i + 2..]
                .iter()
                .take(16)
                .take_while(|b| b.is_ascii_hexdigit())
                .count();
            if digits >= 8 {
                pieces.push(&s[start..i]);
                i += 2 + digits;
                start = i;
                continue;
            }
        }
        i += 1;
    }
    pieces.push(&s[start..]);
    pieces
}

/// `"\n" not in value`.
fn lacks_newline(value: &Value) -> Result<bool, DiffError> {
    match value {
        Value::Str(s) => Ok(!s.contains('\n')),
        Value::List(items) => Ok(!items.iter().any(|i| i.as_str() == Some("\n"))),
        Value::Object(map) => Ok(!map.contains_key("\n")),
        _ => Err(type_error("argument is not iterable")),
    }
}

/// One member of two MIME bundles, compared with the approximate or the strict text predicate.
fn compare_mimedata(
    x: (&Value, &MimeEntry<'_>),
    y: (&Value, &MimeEntry<'_>),
    strict: bool,
) -> Result<bool, DiffError> {
    let ((xv, xe), (yv, ye)) = (x, y);
    let mimetype = xe.mimetype.as_str();
    let (xt, yt) = (
        Text {
            value: xv,
            prepared: xe.text.as_ref(),
        },
        Text {
            value: yv,
            prepared: ye.text.as_ref(),
        },
    );
    let compare_text = |maxlen| {
        if strict {
            compare_text_strict(xt, yt, maxlen)
        } else {
            compare_text_approximate(xt, yt, maxlen)
        }
    };
    let maxlen = Some(TEXT_MIMEDATA_MAX_COMPARE_LENGTH);

    // Short one-line texts may differ in pointer values only.
    if mimetype == "text/plain" {
        match (xe.pointer_pieces(), ye.pointer_pieces()) {
            (Some(px), Some(py)) => {
                if px == py {
                    return Ok(true);
                }
            }
            _ if lacks_newline(xv)? && lacks_newline(yv)? => {
                return Err(type_error("expected string or bytes-like object"));
            }
            _ => {}
        }
    }
    if mimetype.starts_with("text/") {
        return compare_text(maxlen);
    }
    if let (Some(px), Some(py)) = (xe.text.as_ref(), ye.text.as_ref()) {
        // Both the base64 and the text comparison accept equal strings.
        if px.text() == py.text() {
            return Ok(true);
        }
        if xe.is_base64() {
            return Ok(px.len() == py.len() && px.text() == py.text());
        }
        return compare_text(maxlen);
    }
    Ok(xv.py_eq(yv))
}

/// `compare_mimebundle_approximate` and `compare_mimebundle_strict`; `None` and JSON null are
/// both Python `None`.
fn compare_mimebundle(
    x: Option<(&Value, &PreparedOutput<'_>)>,
    y: Option<(&Value, &PreparedOutput<'_>)>,
    strict: bool,
) -> Result<bool, DiffError> {
    let (x, y) = match (x, y) {
        (None, None) => return Ok(true),
        (None, _) | (_, None) => return Ok(false),
        (Some(x), Some(y)) => (x, y),
    };
    let (Some(xm), Some(ym)) = (x.0.as_object(), y.0.as_object()) else {
        return Err(type_error("MIME bundles should be dictionaries"));
    };
    if !same_keys(xm, ym) {
        return Ok(false);
    }
    for (i, (key, xv)) in xm.iter().enumerate() {
        let (j, _, yv) = ym.get_full(key).expect("same keys");
        if !compare_mimedata((xv, &x.1.bundle[i]), (yv, &y.1.bundle[j]), strict)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn bundle<'v, 'a>(
    map: &'v Map,
    output: &'v PreparedOutput<'a>,
    required: bool,
) -> Result<Option<(&'v Value, &'v PreparedOutput<'a>)>, DiffError> {
    let data = if required {
        Some(field(map, "data")?)
    } else {
        map.get("data")
    };
    Ok(data
        .filter(|d| !matches!(d, Value::Null))
        .map(|d| (d, output)))
}

fn compare_tracebacks(x: &Value, y: &Value) -> Result<bool, DiffError> {
    let (Some(x), Some(y)) = (x.as_list(), y.as_list()) else {
        return Err(type_error("tracebacks must be lists"));
    };
    if x.len() != y.len() {
        return Ok(false);
    }
    for (a, b) in x.iter().zip(y) {
        if !compare_values_approximate(a, b, None)? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn compare_output_approximate(x: &Item<'_>, y: &Item<'_>) -> Result<bool, DiffError> {
    let (xm, ym) = (object(x.value)?, object(y.value)?);
    let output_type = field(xm, "output_type")?;
    if !output_type.py_eq(field(ym, "output_type")?) {
        return Ok(false);
    }
    if !same_keys(xm, ym) {
        return Ok(false);
    }

    // Metadata and execution counts are deliberately not compared.
    let mut handled = vec!["output_type", "metadata", "execution_count"];
    match output_type.as_str() {
        Some("stream") => {
            if !field(xm, "name")?.py_eq(field(ym, "name")?) {
                return Ok(false);
            }
            let (xt, yt) = (field(xm, "text")?, field(ym, "text")?);
            let similar = match (&x.output().text, &y.output().text) {
                (Some(px), Some(py)) => {
                    px.is_similar(py, APPROXIMATE, Some(STREAM_MAX_COMPARE_LENGTH))
                }
                _ => compare_values_approximate(xt, yt, Some(STREAM_MAX_COMPARE_LENGTH))?,
            };
            if !similar {
                return Ok(false);
            }
            handled.extend(["name", "text"]);
        }
        Some("error") => {
            if !field(xm, "ename")?.py_eq(field(ym, "ename")?)
                || !field(xm, "evalue")?.py_eq(field(ym, "evalue")?)
                || !compare_tracebacks(field(xm, "traceback")?, field(ym, "traceback")?)?
            {
                return Ok(false);
            }
            handled.extend(["ename", "evalue", "traceback"]);
        }
        Some("display_data" | "execute_result") => {
            let xd = bundle(xm, x.output(), true)?;
            let yd = bundle(ym, y.output(), true)?;
            if !compare_mimebundle(xd, yd, false)? {
                return Ok(false);
            }
            handled.push("data");
        }
        _ => {}
    }
    Ok(other_keys_equal(xm, ym, &handled))
}

/// The "play safe on unknown keys" loop: every key not in `handled` has equal values.
fn other_keys_equal(x: &Map, y: &Map, handled: &[&str]) -> bool {
    x.iter()
        .filter(|(k, _)| !handled.contains(&k.as_str()))
        .all(|(k, xv)| xv.py_eq(&y[k]))
}

pub(super) fn compare_output_strict(x: &Item<'_>, y: &Item<'_>) -> Result<bool, DiffError> {
    let (xm, ym) = (object(x.value)?, object(y.value)?);
    if !field(xm, "output_type")?.py_eq(field(ym, "output_type")?) {
        return Ok(false);
    }
    if !same_keys(xm, ym) {
        return Ok(false);
    }
    if !other_keys_equal(xm, ym, &["output_type", "data"]) {
        return Ok(false);
    }
    compare_mimebundle(
        bundle(xm, x.output(), false)?,
        bundle(ym, y.output(), false)?,
        true,
    )
}

/// `x["cell_type"] != y["cell_type"]` negated.
fn same_cell_type(x: &PreparedCell<'_>, y: &PreparedCell<'_>) -> Result<bool, DiffError> {
    Ok(x.cell_type()?.py_eq(y.cell_type()?))
}

pub(super) fn compare_cell_approximate(x: &Item<'_>, y: &Item<'_>) -> Result<bool, DiffError> {
    let (xc, yc) = (x.cell(), y.cell());
    Ok(same_cell_type(xc, yc)? && compare_text_approximate(xc.source()?, yc.source()?, None)?)
}

pub(super) fn compare_cell_moderate<'a>(
    x: &Item<'a>,
    y: &Item<'a>,
    config: &DiffConfig,
) -> Result<bool, DiffError> {
    let (xc, yc) = (x.cell(), y.cell());
    if !same_cell_type(xc, yc)? || !compare_text_approximate(xc.source()?, yc.source()?, None)? {
        return Ok(false);
    }
    if xc.is_code()? {
        let (xo, yo) = (xc.outputs()?, yc.outputs()?);
        if xo.is_empty() != yo.is_empty() {
            return Ok(false);
        }
        return compare_outputs_approximate(xo, yo, config);
    }
    Ok(true)
}

pub(super) fn compare_cell_strict(x: &Item<'_>, y: &Item<'_>) -> Result<bool, DiffError> {
    let (xc, yc) = (x.cell(), y.cell());
    if !same_cell_type(xc, yc)? || !compare_text_strict(xc.source()?, yc.source()?, None)? {
        return Ok(false);
    }
    if xc.is_code()? {
        let (xo, yo) = (xc.outputs()?, yc.outputs()?);
        if xo.len() != yo.len() {
            return Ok(false);
        }
        for (xi, yi) in xo.iter().zip(yo) {
            if !compare_output_strict(xi, yi)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

#[inline]
pub(super) fn compare_cell_by_ids(x: &Item<'_>, y: &Item<'_>) -> Result<bool, DiffError> {
    if x.value.is_object() && y.value.is_object() {
        if let (Some(hx), Some(hy)) = (x.id_hash, y.id_hash) {
            if hx != hy {
                return Ok(false);
            }
        }
        return Ok(matches!((x.id, y.id), (Some(a), Some(b)) if a.py_eq(b)));
    }
    // Cells that are no objects: `'id' in x and 'id' in y and x['id'] == y['id']`.
    let has_id = |v: &Value| -> Result<bool, DiffError> {
        match v {
            Value::Object(m) => Ok(m.contains_key("id")),
            Value::List(items) => Ok(items.iter().any(|i| i.as_str() == Some("id"))),
            Value::Str(s) => Ok(s.contains("id")),
            _ => Err(type_error("argument is not iterable")),
        }
    };
    if !has_id(x.value)? || !has_id(y.value)? {
        return Ok(false);
    }
    // Only objects can be subscripted by a string, and they were handled above.
    Err(type_error("indices must be integers or slices"))
}

/// Whether two output lists align without any output being added or removed, under the
/// differ currently configured for `/cells/*/outputs` (so ignoring outputs makes them align).
///
/// With the default differ (possibly wrapped in key filters) the answer follows from the
/// alignment alone: only additions and removals count, so the diffs of aligned pairs are not
/// computed and added outputs are not copied.
fn compare_outputs_approximate<'a>(
    x: &[Item<'a>],
    y: &[Item<'a>],
    config: &DiffConfig,
) -> Result<bool, DiffError> {
    let mut filters = Vec::new();
    let mut differ = config.differ(OUTPUTS_PATH);
    while let Differ::IgnoreKeys { inner, keys } = differ {
        filters.push(keys.as_slice());
        differ = inner;
    }
    let survives = |key: usize| !filters.iter().any(|keys| index_listed(key, keys));
    match differ {
        Differ::Ignore => Ok(true),
        Differ::SequenceMultilevel => {
            let snakes = multilevel_snakes(x, y, OUTPUTS_PATH, config)?;
            Ok(!unaligned_keys(&snakes, x.len(), y.len())
                .into_iter()
                .any(survives))
        }
        other => {
            let values =
                |items: &[Item<'_>]| Value::List(items.iter().map(|i| i.value().clone()).collect());
            let mut diff = other.apply(&values(x), &values(y), OUTPUTS_PATH, config)?;
            for keys in filters {
                retain_unlisted_keys(&mut diff, keys);
            }
            Ok(diff.iter().all(|e| matches!(e.op, Op::Patch { .. })))
        }
    }
}

/// The keys of the `removerange` and `addrange` entries that the snakes yield: the start of
/// every run of items outside them, in either list.
fn unaligned_keys(snakes: &[Snake], len_x: usize, len_y: usize) -> Vec<usize> {
    let closing = Snake {
        i: len_x,
        j: len_y,
        n: 0,
    };
    let (mut i0, mut j0) = (0, 0);
    let mut keys = Vec::new();
    for snake in snakes.iter().chain([&closing]) {
        if snake.i > i0 || snake.j > j0 {
            keys.push(i0);
        }
        i0 = snake.i + snake.n;
        j0 = snake.j + snake.n;
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::super::config::Predicate;
    use super::super::generic::diff_sequence_multilevel_items;
    use super::super::MAX_DIFF_DEPTH;
    use super::*;
    use crate::json::parse;

    /// The default configuration with the notebook outputs predicates and differ.
    fn config() -> DiffConfig {
        let mut config = DiffConfig::default();
        config.set_predicates(
            OUTPUTS_PATH,
            vec![Predicate::OutputApproximate, Predicate::OutputStrict],
        );
        config.set_differ(OUTPUTS_PATH, Differ::SequenceMultilevel);
        config
    }

    fn check(
        pred: fn(&Item<'_>, &Item<'_>) -> Result<bool, DiffError>,
        x: &str,
        y: &str,
    ) -> Result<bool, DiffError> {
        let (x, y) = (parse(x).unwrap(), parse(y).unwrap());
        pred(&Item::new(&x), &Item::new(&y))
    }

    fn cell(kind: &str, source: &str, extra: &str) -> String {
        format!(r#"{{"cell_type":"{kind}","source":{source:?}{extra}}}"#)
    }

    #[test]
    fn base64_detection() {
        let data = "QUJD".repeat(16);
        assert!(is_base64(&data, 64));
        assert!(!is_base64(&data[..60], 60), "too short");
        assert!(is_base64(&format!("{data}QQ=="), 68));
        assert!(is_base64(&format!("{data}QUI="), 68));
        assert!(!is_base64(&format!("{data}QUI"), 67), "bad length");
        assert!(!is_base64(&format!("{data}Q=Q="), 68), "padding inside");
        assert!(!is_base64(&format!("{data}QQ==="), 69));
        // Every line boundary is removed first, not only "\n".
        let lined = format!(
            "{}\r\n{}\u{2028}{}",
            &data[..20],
            &data[20..40],
            &data[40..]
        );
        assert!(is_base64(&lined, lined.chars().count()));
        // Case-insensitive Unicode matching also admits characters folding to ASCII letters.
        let folded = format!("{}\u{17f}\u{212a}\u{130}\u{131}", &data[..60]);
        assert!(is_base64(&folded, 64));
        assert!(!is_base64(&format!("{}\u{e9}", &data[..63]), 64));
    }

    #[test]
    fn pointer_splitting() {
        assert_eq!(split_pointers("a 0x7f3a2b1c4d5e b"), ["a ", " b"]);
        assert_eq!(split_pointers("0x12345678"), ["", ""]);
        // Seven digits are too few; more than sixteen leave the rest in the next piece.
        assert_eq!(split_pointers("0x1234567"), ["0x1234567"]);
        assert_eq!(split_pointers("0X0123456789ABCDEF00"), ["", "00"]);
        assert_eq!(split_pointers("0x0x12345678"), ["0x", ""]);
        assert_eq!(split_pointers("no pointer"), ["no pointer"]);
    }

    #[test]
    fn text_predicates_short_strings_and_truthiness() {
        let (one, two, empty) = (Value::from("1"), Value::from("2"), Value::from(""));
        let (p1, p2, pe) = (
            PreparedStr::new("1"),
            PreparedStr::new("2"),
            PreparedStr::new(""),
        );
        let t = |value, prepared| Text {
            value,
            prepared: Some(prepared),
        };
        let (a, b, e) = (t(&one, &p1), t(&two, &p2), t(&empty, &pe));
        assert_eq!(compare_text_approximate(a, b, None), Ok(true));
        assert_eq!(compare_text_approximate(a, e, None), Ok(false));
        assert_eq!(compare_text_strict(a, b, None), Ok(false));
        assert_eq!(compare_text_strict(a, a, None), Ok(true));
        // Unprepared operands: unhashable ones fail first, then truthiness, then `len()`.
        let n = |value| Text {
            value,
            prepared: None,
        };
        let (zero, list) = (Value::from(0), Value::List(vec![]));
        assert_eq!(compare_text_approximate(n(&zero), a, None), Ok(false));
        assert!(compare_text_approximate(n(&zero), n(&zero), None).is_err());
        assert!(compare_text_approximate(n(&list), n(&zero), None).is_err());
    }

    #[test]
    fn cell_predicates() {
        let code =
            |source: &str, outputs: &str| cell("code", source, &format!(r#","outputs":{outputs}"#));
        let (a, b) = (code("x = 1\ny = 2\n", "[]"), code("x = 1\ny = 3\n", "[]"));
        assert_eq!(check(compare_cell_approximate, &a, &b), Ok(true));
        // Strict needs a ratio above 0.95.
        assert_eq!(check(compare_cell_strict, &a, &b), Ok(false));
        let long = "import numpy as np\nvalues = np.arange(100)\nprint(values.sum())\n";
        let (c, d) = (code(long, "[]"), code(&long.replace("100", "101"), "[]"));
        assert_eq!(check(compare_cell_strict, &c, &d), Ok(true));
        let (a, b) = (
            code("alpha beta gamma", "[]"),
            code("zzzzzz yyyyyy xxxxx", "[]"),
        );
        assert_eq!(check(compare_cell_approximate, &a, &b), Ok(false));
        let md = cell("markdown", "alpha beta gamma", "");
        assert_eq!(check(compare_cell_approximate, &a, &md), Ok(false));
        // Outputs are not looked at for non-code cells, nor by the approximate predicate.
        let (m1, m2) = (
            cell("markdown", "same text here", ""),
            cell("markdown", "same text here", r#","outputs":1"#),
        );
        assert_eq!(check(compare_cell_strict, &m1, &m2), Ok(true));
    }

    #[test]
    fn missing_keys_are_errors_where_upstream_raises() {
        let ok = cell("code", "x", r#","outputs":[]"#);
        let no_source = r#"{"cell_type":"code","outputs":[]}"#;
        assert!(matches!(
            check(compare_cell_strict, &ok, no_source),
            Err(DiffError::MissingKey(_))
        ));
        // Different cell types decide before the source is read.
        let md = r#"{"cell_type":"markdown"}"#;
        assert_eq!(check(compare_cell_approximate, no_source, md), Ok(false));
        assert!(check(compare_cell_approximate, "[]", md).is_err());
    }

    #[test]
    fn cell_ids_need_both_cells_to_have_one() {
        let with = |id: &str| format!(r#"{{"id":"{id}"}}"#);
        assert_eq!(check(compare_cell_by_ids, &with("a"), &with("a")), Ok(true));
        assert_eq!(
            check(compare_cell_by_ids, &with("a"), &with("b")),
            Ok(false)
        );
        assert_eq!(check(compare_cell_by_ids, "{}", "{}"), Ok(false));
        assert_eq!(check(compare_cell_by_ids, "[]", "[]"), Ok(false));
        assert!(check(compare_cell_by_ids, "1", "{}").is_err());
        assert!(check(compare_cell_by_ids, r#""valid""#, r#""valid""#).is_err());
    }

    fn stream(text: &str, extra: &str) -> String {
        format!(r#"{{"output_type":"stream","name":"stdout","text":{text:?}{extra}}}"#)
    }

    #[test]
    fn output_predicates_on_streams() {
        let (a, b) = (
            stream("hello world\nline two\n", ""),
            stream("hello world\nline 2\n", ""),
        );
        assert_eq!(check(compare_output_approximate, &a, &b), Ok(true));
        assert_eq!(check(compare_output_strict, &a, &b), Ok(false));
        assert_eq!(check(compare_output_strict, &a, &a), Ok(true));
        // Approximate ignores metadata and execution counts, strict does not.
        let (m1, m2) = (
            stream("x", r#","metadata":{"a":1}"#),
            stream("x", r#","metadata":{"a":2}"#),
        );
        assert_eq!(check(compare_output_approximate, &m1, &m2), Ok(true));
        assert_eq!(check(compare_output_strict, &m1, &m2), Ok(false));
        // Other unknown keys are compared by both, and different key sets never align.
        let (k1, k2) = (stream("x", r#","k":1"#), stream("x", r#","k":2"#));
        assert_eq!(check(compare_output_approximate, &k1, &k2), Ok(false));
        assert_eq!(
            check(compare_output_approximate, &k1, &stream("x", "")),
            Ok(false)
        );
    }

    #[test]
    fn output_predicates_on_mime_bundles() {
        let display = |data: &str| {
            format!(r#"{{"output_type":"display_data","metadata":{{}},"data":{data}}}"#)
        };
        let image = "QUJD".repeat(20);
        let (a, b) = (
            display(&format!(
                r#"{{"text/plain":"<Figure>","image/png":"{image}"}}"#
            )),
            display(&format!(
                r#"{{"text/plain":"<Figure>","image/png":"{}"}}"#,
                "QUJE".repeat(20)
            )),
        );
        // Base64 data must be equal, not similar.
        assert_eq!(check(compare_output_approximate, &a, &b), Ok(false));
        assert_eq!(check(compare_output_approximate, &a, &a), Ok(true));
        let ptr = |p: &str| display(&format!(r#"{{"text/plain":"<Foo at 0x{p}>"}}"#));
        assert_eq!(
            check(
                compare_output_strict,
                &ptr("7f3a2b1c4d5e"),
                &ptr("7f00deadbeef")
            ),
            Ok(true)
        );
        assert_eq!(
            check(compare_output_approximate, &display("{}"), &display("{}")),
            Ok(true)
        );
        assert_eq!(
            check(
                compare_output_approximate,
                &display(r#"{"a":1}"#),
                &display(r#"{"b":1}"#)
            ),
            Ok(false)
        );
        assert!(matches!(
            check(
                compare_output_approximate,
                r#"{"output_type":"display_data"}"#,
                r#"{"output_type":"display_data"}"#
            ),
            Err(DiffError::MissingKey(_))
        ));
        // Strict reads `data` with `get`, so a missing bundle equals another missing bundle.
        assert_eq!(
            check(
                compare_output_strict,
                r#"{"output_type":"display_data"}"#,
                r#"{"output_type":"display_data"}"#
            ),
            Ok(true)
        );
    }

    #[test]
    fn error_outputs_compare_tracebacks() {
        let error = |tb: &str| {
            format!(r#"{{"output_type":"error","ename":"E","evalue":"v","traceback":{tb}}}"#)
        };
        let (a, b) = (
            error(r#"["abcdefgh","ijklmnop"]"#),
            error(r#"["abcdefgx","ijklmnop"]"#),
        );
        assert_eq!(check(compare_output_approximate, &a, &b), Ok(true));
        assert_eq!(
            check(compare_output_approximate, &a, &error(r#"["abcdefgh"]"#)),
            Ok(false)
        );
    }

    #[test]
    fn outputs_align_under_the_configured_outputs_differ() {
        let a = parse(&stream("hello world", "")).unwrap();
        let b = parse(&stream("completely different text", "")).unwrap();
        let (xi, yi) = ([Item::new(&a)], [Item::new(&b)]);
        let mut config = config();
        assert_eq!(compare_outputs_approximate(&xi, &yi, &config), Ok(false));
        assert_eq!(compare_outputs_approximate(&xi, &[], &config), Ok(false));
        assert_eq!(compare_outputs_approximate(&[], &[], &config), Ok(true));
        config.set_differ(OUTPUTS_PATH, Differ::Ignore);
        assert_eq!(compare_outputs_approximate(&xi, &yi, &config), Ok(true));
    }

    fn ignoring(keys: &[i64], inner: Differ) -> Differ {
        Differ::IgnoreKeys {
            inner: Box::new(inner),
            keys: keys.iter().map(|&k| Value::from(k)).collect(),
        }
    }

    #[test]
    fn ignored_keys_hide_unaligned_outputs_at_those_positions() {
        let texts = ["alpha one", "beta two two", "gamma three three three"];
        let values: Vec<Value> = texts
            .iter()
            .map(|t| parse(&stream(t, "")).unwrap())
            .collect();
        let items: Vec<_> = values.iter().map(Item::new).collect();
        // The second output was removed: one `removerange` at key 1.
        let (x, y) = (
            &items[..],
            &[items[0].value(), items[2].value()].map(Item::new),
        );
        let compare = |differ: Differ| {
            let mut config = config();
            config.set_differ(OUTPUTS_PATH, differ);
            compare_outputs_approximate(x, y, &config)
        };
        assert_eq!(compare(Differ::SequenceMultilevel), Ok(false));
        assert_eq!(
            compare(ignoring(&[1], Differ::SequenceMultilevel)),
            Ok(true)
        );
        assert_eq!(
            compare(ignoring(&[0], Differ::SequenceMultilevel)),
            Ok(false)
        );
        // `True == 1`, and listing the key in any one wrapper is enough to drop the entry.
        let by_bool = Differ::IgnoreKeys {
            inner: Box::new(Differ::SequenceMultilevel),
            keys: vec![Value::Bool(true)],
        };
        assert_eq!(compare(by_bool), Ok(true));
        let nested = ignoring(&[0], ignoring(&[1], Differ::SequenceMultilevel));
        assert_eq!(compare(nested), Ok(true));
        assert_eq!(compare(ignoring(&[0], Differ::Ignore)), Ok(true));
        assert_eq!(
            compare(ignoring(&[], ignoring(&[], Differ::Ignore))),
            Ok(true)
        );
    }

    #[test]
    fn alignment_alone_decides_like_the_full_output_diff() {
        let outputs = [
            stream("hello world", ""),
            stream("hello there, world", ""),
            stream("something else entirely", ""),
            r#"{"output_type":"display_data","metadata":{},"data":{"text/plain":"<Figure size 640x480>"}}"#
                .to_owned(),
            r#"{"output_type":"error","ename":"E","evalue":"v","traceback":["tb line one"]}"#
                .to_owned(),
        ];
        let values: Vec<Value> = outputs.iter().map(|o| parse(o).unwrap()).collect();
        let config = config();
        // Every pair of sublists of up to three outputs, including the empty one.
        let lists: Vec<Vec<usize>> = (0..(1 << 7))
            .map(|m: usize| (0..5).filter(|i| m & (1 << i) != 0).collect::<Vec<_>>())
            .filter(|l| l.len() <= 3)
            .collect();
        for xs in &lists {
            for ys in &lists {
                let x: Vec<_> = xs.iter().map(|&i| Item::new(&values[i])).collect();
                let y: Vec<_> = ys.iter().map(|&i| Item::new(&values[i])).collect();
                let full = diff_sequence_multilevel_items(&x, &y, OUTPUTS_PATH, &config).unwrap();
                let expected = full.iter().all(|e| matches!(e.op, Op::Patch { .. }));
                assert_eq!(
                    compare_outputs_approximate(&x, &y, &config),
                    Ok(expected),
                    "{xs:?} against {ys:?}"
                );
            }
        }
    }

    #[test]
    fn predicates_refuse_inputs_nested_beyond_the_diff_limit_on_a_small_stack() {
        // Stream outputs that carry cells make the predicates recurse through the value, so the
        // public entry has to bound the nesting before it starts.
        fn nested_cell(cycles: usize) -> Value {
            let stream =
                |extra: &str| format!(r#"{{"output_type":"stream","name":"o","text":"t"{extra}}}"#);
            let mut output = parse(&stream("")).unwrap();
            for _ in 0..cycles {
                let cell = Value::Object(Map::from_iter([
                    ("cell_type".to_owned(), Value::from("code")),
                    ("source".to_owned(), Value::from("x")),
                    ("outputs".to_owned(), Value::List(vec![output])),
                ]));
                let mut with_cells = parse(&stream("")).unwrap();
                if let Value::Object(map) = &mut with_cells {
                    map.insert("cells".to_owned(), Value::List(vec![cell]));
                }
                output = with_cells;
            }
            Value::Object(Map::from_iter([
                ("cell_type".to_owned(), Value::from("code")),
                ("source".to_owned(), Value::from("x")),
                ("outputs".to_owned(), Value::List(vec![output])),
            ]))
        }
        let handle = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(|| {
                let config = DiffConfig::default();
                let deep = nested_cell(1000);
                let results = [
                    Predicate::CellModerate.compare(&deep, &deep, &config),
                    Predicate::CellStrict.compare(&deep, &deep, &config),
                    Predicate::Eq.compare(&deep, &deep, &config),
                ];
                // Dropping runs on this thread too; the nesting is shallow enough for that.
                results
            })
            .unwrap();
        for result in handle.join().unwrap() {
            assert_eq!(result, Err(DiffError::TooDeep(MAX_DIFF_DEPTH)));
        }
    }
}
