//! Conversion between Python objects and [`Value`], without a JSON round trip.
//!
//! Both directions walk the data with an explicit stack, so deeply nested input cannot overflow
//! the native stack. Circular references and nesting beyond [`MAX_NESTING`] are errors.

use std::collections::HashSet;
use std::mem;

use nbdime_core::{Int, Map, Value};
use pyo3::exceptions::{PyOverflowError, PyTypeError, PyUnicodeEncodeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyIterator, PyList, PySet, PyString, PyTuple};

/// Deepest nesting of lists and dicts accepted, matching what `json.loads` can produce.
pub const MAX_NESTING: usize = 10_000;

/// A converted Python object. Dropping it releases the nested values without recursion.
pub struct Doc {
    value: Value,
    depth: usize,
}

impl Doc {
    /// How deeply lists and dicts are nested (an empty container is 1, a scalar 0).
    pub fn depth(&self) -> usize {
        self.depth
    }
}

impl std::ops::Deref for Doc {
    type Target = Value;

    fn deref(&self) -> &Value {
        &self.value
    }
}

impl Drop for Doc {
    fn drop(&mut self) {
        dispose(mem::take(&mut self.value));
    }
}

/// Drops `value` iteratively; the derived drop glue recurses once per nesting level.
fn dispose(value: Value) {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::List(items) => pending.extend(items),
            Value::Object(map) => pending.extend(map.into_values()),
            _ => {}
        }
    }
}

enum ListSource<'py> {
    Exact(<Bound<'py, PyList> as IntoIterator>::IntoIter),
    Python(Bound<'py, PyIterator>),
}

struct Frame<'py> {
    // A subclass's iterator may not keep the container alive.
    owner: Bound<'py, PyAny>,
    contents: Contents<'py>,
}

enum Contents<'py> {
    List {
        source: ListSource<'py>,
        items: Vec<Value>,
    },
    Dict {
        source: Bound<'py, PyIterator>,
        entries: Map,
        key: Option<String>,
    },
}

impl Frame<'_> {
    fn dispose(self) {
        match self.contents {
            Contents::List { items, .. } => items.into_iter().for_each(dispose),
            Contents::Dict { entries, .. } => entries.into_values().for_each(dispose),
        }
    }
}

/// Converts `obj` as `json.dumps` would read it: `dict` with `str` keys, `list`, `str`, `int`,
/// `float`, `bool` and `None`, or subclasses of those. Anything else is a `TypeError`.
pub fn from_py(obj: &Bound<'_, PyAny>) -> PyResult<Doc> {
    from_py_with_ignores(obj, false)
}

/// The values in the `ignores` dict can also be tuples or sets of keys.
pub fn from_ignores(obj: &Bound<'_, PyAny>) -> PyResult<Doc> {
    from_py_with_ignores(obj, true)
}

fn from_py_with_ignores(obj: &Bound<'_, PyAny>, ignores: bool) -> PyResult<Doc> {
    let mut stack = Vec::new();
    let mut depth = 0;
    match convert(obj, &mut stack, &mut depth, ignores) {
        Ok(value) => Ok(Doc { value, depth }),
        Err(e) => {
            stack.into_iter().for_each(Frame::dispose);
            Err(e)
        }
    }
}

fn convert<'py>(
    root: &Bound<'py, PyAny>,
    stack: &mut Vec<Frame<'py>>,
    depth: &mut usize,
    ignores: bool,
) -> PyResult<Value> {
    let mut pending = Some(root.clone());
    let mut done: Option<Value> = None;
    let mut active = HashSet::new();
    loop {
        if let Some(obj) = pending.take() {
            if active.contains(&obj.as_ptr()) {
                return Err(PyValueError::new_err("Circular reference detected"));
            }
            let ignore_keys = ignores
                && matches!(
                    stack.as_slice(),
                    [Frame {
                        contents: Contents::Dict { .. },
                        ..
                    }]
                );
            match open(&obj, ignore_keys)? {
                Opened::Scalar(value) => done = Some(value),
                Opened::Container(contents) => {
                    active.insert(obj.as_ptr());
                    stack.push(Frame {
                        owner: obj,
                        contents,
                    });
                    *depth = (*depth).max(stack.len());
                    if stack.len() > MAX_NESTING {
                        return Err(PyValueError::new_err(format!(
                            "nesting deeper than {MAX_NESTING} levels"
                        )));
                    }
                }
            }
        }
        if let Some(value) = done.take() {
            match stack.last_mut().map(|frame| &mut frame.contents) {
                None => return Ok(value),
                Some(Contents::List { items, .. }) => items.push(value),
                Some(Contents::Dict { entries, key, .. }) => {
                    if let Some(old) =
                        entries.insert(key.take().expect("a dict value follows its key"), value)
                    {
                        dispose(old);
                    }
                }
            }
        }
        let top = stack.last_mut().expect("a container is open");
        match next_child(&mut top.contents)? {
            Some(child) => pending = Some(child),
            None => {
                let frame = stack.pop().expect("a container is open");
                active.remove(&frame.owner.as_ptr());
                done = Some(match frame.contents {
                    Contents::List { items, .. } => Value::List(items),
                    Contents::Dict { entries, .. } => Value::Object(entries),
                });
            }
        }
    }
}

enum Opened<'py> {
    Scalar(Value),
    Container(Contents<'py>),
}

fn open<'py>(obj: &Bound<'py, PyAny>, ignore_keys: bool) -> PyResult<Opened<'py>> {
    if obj.is_none() {
        return Ok(Opened::Scalar(Value::Null));
    }
    // `bool` first: it is a subclass of `int`.
    if let Ok(b) = obj.cast::<PyBool>() {
        return Ok(Opened::Scalar(Value::Bool(b.is_true())));
    }
    if let Ok(i) = obj.cast::<PyInt>() {
        return Ok(Opened::Scalar(Value::Int(int(i)?)));
    }
    if let Ok(f) = obj.cast::<PyFloat>() {
        return Ok(Opened::Scalar(Value::Float(f.value())));
    }
    if let Ok(s) = obj.cast::<PyString>() {
        return Ok(Opened::Scalar(Value::Str(string(s)?)));
    }
    if let Ok(list) = obj.cast_exact::<PyList>() {
        return Ok(Opened::Container(Contents::List {
            items: Vec::with_capacity(list.len()),
            source: ListSource::Exact(list.iter()),
        }));
    }
    if obj.cast::<PyList>().is_ok()
        || (ignore_keys && (obj.cast::<PyTuple>().is_ok() || obj.cast::<PySet>().is_ok()))
    {
        return Ok(Opened::Container(Contents::List {
            items: Vec::new(),
            source: ListSource::Python(obj.try_iter()?),
        }));
    }
    if let Ok(dict) = obj.cast::<PyDict>() {
        // A Python items iterator preserves subclass order and raises on mutation.
        return Ok(Opened::Container(Contents::Dict {
            entries: Map::with_capacity(dict.len()),
            source: dict.call_method0("items")?.try_iter()?,
            key: None,
        }));
    }
    Err(PyTypeError::new_err(format!(
        "cannot convert an object of type {} (only dict, list, str, int, float, bool and None)",
        obj.get_type().name()?
    )))
}

fn next_child<'py>(frame: &mut Contents<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
    match frame {
        Contents::List { source, .. } => match source {
            ListSource::Exact(source) => Ok(source.next()),
            ListSource::Python(source) => source.next().transpose(),
        },
        Contents::Dict { source, key, .. } => {
            let Some(pair) = source.next().transpose()? else {
                return Ok(None);
            };
            let pair = pair
                .cast::<PyTuple>()
                .ok()
                .filter(|pair| pair.len() == 2)
                .ok_or_else(|| PyValueError::new_err("items must return 2-tuples"))?;
            let (k, v) = (pair.get_item(0)?, pair.get_item(1)?);
            let Ok(k) = k.cast::<PyString>() else {
                return Err(PyTypeError::new_err(format!(
                    "dict keys must be str, not {}",
                    k.get_type().name()?
                )));
            };
            *key = Some(string(k)?);
            Ok(Some(v))
        }
    }
}

fn string(s: &Bound<'_, PyString>) -> PyResult<String> {
    s.to_cow()
        .map(std::borrow::Cow::into_owned)
        .map_err(|error| {
            if error.is_instance_of::<PyUnicodeEncodeError>(s.py()) {
                PyValueError::new_err(
                    "string contains a lone surrogate, which the engine cannot represent",
                )
            } else {
                error
            }
        })
}

fn int(i: &Bound<'_, PyInt>) -> PyResult<Int> {
    match i.extract::<i64>() {
        Ok(v) => Ok(Int::Small(v)),
        Err(e) if e.is_instance_of::<PyOverflowError>(i.py()) => {
            // `int.__repr__` rather than `str`: a subclass may override `__str__`.
            let digits = i.py().get_type::<PyInt>().call_method1("__repr__", (i,))?;
            Ok(Int::Big(digits.extract()?))
        }
        Err(e) => Err(e),
    }
}

enum Building<'py> {
    List(Bound<'py, PyList>, std::vec::IntoIter<Value>),
    Dict(
        Bound<'py, PyDict>,
        indexmap::map::IntoIter<String, Value>,
        Option<String>,
    ),
}

/// Builds the Python object for `value`: `dict`, `list`, `str`, `int`, `float`, `bool`, `None`.
pub fn to_py<'py>(py: Python<'py>, value: Value) -> PyResult<Bound<'py, PyAny>> {
    let mut stack: Vec<Building<'py>> = Vec::new();
    let mut pending = Some(value);
    let mut done: Option<Bound<'py, PyAny>> = None;
    loop {
        if let Some(value) = pending.take() {
            match value {
                Value::List(items) => {
                    stack.push(Building::List(PyList::empty(py), items.into_iter()));
                }
                Value::Object(entries) => {
                    stack.push(Building::Dict(PyDict::new(py), entries.into_iter(), None));
                }
                scalar => done = Some(scalar_to_py(py, scalar)?),
            }
        }
        if let Some(child) = done.take() {
            match stack.last_mut() {
                None => return Ok(child),
                Some(Building::List(list, _)) => list.append(child)?,
                Some(Building::Dict(dict, _, key)) => {
                    dict.set_item(key.take().expect("a dict value follows its key"), child)?;
                }
            }
        }
        match stack.last_mut().expect("a container is open") {
            Building::List(list, rest) => match rest.next() {
                Some(next) => pending = Some(next),
                None => done = Some(list.clone().into_any()),
            },
            Building::Dict(dict, rest, key) => match rest.next() {
                Some((k, next)) => {
                    *key = Some(k);
                    pending = Some(next);
                }
                None => done = Some(dict.clone().into_any()),
            },
        }
        if done.is_some() {
            stack.pop();
        }
    }
}

fn scalar_to_py<'py>(py: Python<'py>, value: Value) -> PyResult<Bound<'py, PyAny>> {
    Ok(match value {
        Value::Null => py.None().into_bound(py),
        Value::Bool(b) => PyBool::new(py, b).to_owned().into_any(),
        Value::Int(Int::Small(i)) => PyInt::new(py, i).into_any(),
        Value::Int(Int::Big(digits)) => py.get_type::<PyInt>().call1((digits,))?,
        Value::Float(f) => PyFloat::new(py, f).into_any(),
        Value::Str(s) => PyString::new(py, &s).into_any(),
        Value::List(_) | Value::Object(_) => unreachable!("containers are built by the caller"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::PyTuple;

    fn eval<'py>(py: Python<'py>, code: &str) -> Bound<'py, PyAny> {
        py.eval(&std::ffi::CString::new(code).unwrap(), None, None)
            .unwrap()
    }

    #[test]
    fn scalars_keep_their_python_type() {
        Python::attach(|py| {
            let doc = from_py(&eval(py, "[None, True, 1, 1.0, 'é', 2**70]")).unwrap();
            let Value::List(items) = &*doc else { panic!() };
            assert_eq!(items[0], Value::Null);
            assert_eq!(items[1], Value::Bool(true));
            assert_eq!(items[2], Value::Int(Int::Small(1)));
            assert_eq!(items[3], Value::Float(1.0));
            assert_eq!(items[4], Value::from("é"));
            assert_eq!(
                items[5],
                Value::Int(Int::Big("1180591620717411303424".into()))
            );
        });
    }

    #[test]
    fn round_trip_keeps_dict_order_and_types() {
        Python::attach(|py| {
            let src = eval(py, "{'z': [1, {'k': True}], 'a': -(2**64), 'm': {}}");
            let mut doc = from_py(&src).unwrap();
            assert_eq!(doc.depth(), 3);
            let back = to_py(py, mem::take(&mut doc.value)).unwrap();
            assert!(back.eq(&src).unwrap());
            let keys: Vec<String> = back.cast::<PyDict>().unwrap().keys().extract().unwrap();
            assert_eq!(keys, ["z", "a", "m"]);
        });
    }

    #[test]
    fn subclasses_are_accepted() {
        Python::attach(|py| {
            let src = eval(
                py,
                "type('D', (dict,), {})({'a': type('L', (list,), {})([type('S', (str,), {})('x')])})",
            );
            let doc = from_py(&src).unwrap();
            assert_eq!(
                doc.get("a").unwrap().as_list().unwrap()[0],
                Value::from("x")
            );
        });
    }

    #[test]
    fn unsupported_types_are_type_errors() {
        Python::attach(|py| {
            for code in ["(1, 2)", "b'x'", "{1, 2}", "{1: 2}", "[object()]", "1j"] {
                let err = from_py(&eval(py, code)).err().unwrap();
                assert!(err.is_instance_of::<PyTypeError>(py), "{code}");
            }
            let tuple = PyTuple::empty(py);
            assert!(from_py(tuple.as_any()).is_err());
        });
    }

    #[test]
    fn lone_surrogates_are_value_errors() {
        Python::attach(|py| {
            let err = from_py(&eval(py, "['\\ud800']")).err().unwrap();
            assert!(err.is_instance_of::<PyValueError>(py));
        });
    }

    #[test]
    fn circular_and_very_deep_input_is_an_error_not_a_crash() {
        Python::attach(|py| {
            let circular = eval(py, "(lambda a: (a.append(a), a)[1])([])");
            assert!(from_py(&circular)
                .err()
                .unwrap()
                .is_instance_of::<PyValueError>(py));
            let scope = PyDict::new(py);
            py.run(
                c"d = []\nfor _ in range(9000):\n    d = [d]",
                Some(&scope),
                None,
            )
            .unwrap();
            let deep = scope.get_item("d").unwrap().unwrap();
            let mut doc = from_py(&deep).unwrap();
            assert_eq!(doc.depth(), 9001);
            let back = to_py(py, mem::take(&mut doc.value)).unwrap();
            assert_eq!(back.len().unwrap(), 1);
        });
    }

    #[test]
    fn huge_ints_survive() {
        Python::attach(|py| {
            let src = eval(py, "[10**400, -(10**400)]");
            let mut doc = from_py(&src).unwrap();
            let back = to_py(py, mem::take(&mut doc.value)).unwrap();
            assert!(back.eq(&src).unwrap());
        });
    }
}
