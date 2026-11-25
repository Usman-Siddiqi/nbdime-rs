//! Conversion between Python objects and [`Value`], without a JSON round trip.
//!
//! Both directions walk the data with an explicit stack, so deeply nested input cannot overflow
//! the native stack (Python's own recursion limit does not apply to this code). Nesting beyond
//! [`MAX_NESTING`] is an error, which also stops circular references.

use std::mem;

use nbdime_core::{Int, Map, Value};
use pyo3::exceptions::{PyOverflowError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString};

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

type ListIter<'py> = <Bound<'py, PyList> as IntoIterator>::IntoIter;
type DictIter<'py> = <Bound<'py, PyDict> as IntoIterator>::IntoIter;

enum Frame<'py> {
    List {
        source: ListIter<'py>,
        items: Vec<Value>,
    },
    Dict {
        source: DictIter<'py>,
        entries: Map,
        key: Option<String>,
    },
}

impl Frame<'_> {
    fn dispose(self) {
        match self {
            Frame::List { items, .. } => items.into_iter().for_each(dispose),
            Frame::Dict { entries, .. } => entries.into_values().for_each(dispose),
        }
    }
}

/// Converts `obj` as `json.dumps` would read it: `dict` with `str` keys, `list`, `str`, `int`,
/// `float`, `bool` and `None`, or subclasses of those. Anything else is a `TypeError`.
pub fn from_py(obj: &Bound<'_, PyAny>) -> PyResult<Doc> {
    let mut stack = Vec::new();
    let mut depth = 0;
    match convert(obj, &mut stack, &mut depth) {
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
) -> PyResult<Value> {
    let mut pending = Some(root.clone());
    let mut done: Option<Value> = None;
    loop {
        if let Some(obj) = pending.take() {
            match open(&obj)? {
                Opened::Scalar(value) => done = Some(value),
                Opened::Frame(frame) => {
                    stack.push(frame);
                    *depth = (*depth).max(stack.len());
                    if stack.len() > MAX_NESTING {
                        return Err(PyValueError::new_err(format!(
                            "circular reference or nesting deeper than {MAX_NESTING} levels"
                        )));
                    }
                }
            }
        }
        if let Some(value) = done.take() {
            match stack.last_mut() {
                None => return Ok(value),
                Some(Frame::List { items, .. }) => items.push(value),
                Some(Frame::Dict { entries, key, .. }) => {
                    entries.insert(key.take().expect("a dict value follows its key"), value);
                }
            }
        }
        let top = stack.last_mut().expect("a container is open");
        match next_child(top)? {
            Some(child) => pending = Some(child),
            None => {
                done = Some(match stack.pop().expect("a container is open") {
                    Frame::List { items, .. } => Value::List(items),
                    Frame::Dict { entries, .. } => Value::Object(entries),
                });
            }
        }
    }
}

enum Opened<'py> {
    Scalar(Value),
    Frame(Frame<'py>),
}

fn open<'py>(obj: &Bound<'py, PyAny>) -> PyResult<Opened<'py>> {
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
    if let Ok(list) = obj.cast::<PyList>() {
        return Ok(Opened::Frame(Frame::List {
            items: Vec::with_capacity(list.len()),
            source: list.iter(),
        }));
    }
    if let Ok(dict) = obj.cast::<PyDict>() {
        return Ok(Opened::Frame(Frame::Dict {
            entries: Map::with_capacity(dict.len()),
            source: dict.iter(),
            key: None,
        }));
    }
    Err(PyTypeError::new_err(format!(
        "cannot convert an object of type {} (only dict, list, str, int, float, bool and None)",
        obj.get_type().name()?
    )))
}

fn next_child<'py>(frame: &mut Frame<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
    match frame {
        Frame::List { source, .. } => Ok(source.next()),
        Frame::Dict { source, key, .. } => {
            let Some((k, v)) = source.next() else {
                return Ok(None);
            };
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
    // `to_str` fails for lone surrogates, which a Rust `String` cannot hold (F-005).
    s.to_cow().map(std::borrow::Cow::into_owned).map_err(|_| {
        PyValueError::new_err("string contains a lone surrogate, which the engine cannot represent")
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
