//! The `nbdime_rs` Python extension: nbdime-rs' diff and merge engine behind plain Python
//! objects. Arguments and results have the shapes of the `nbdime-rs-dev` functions of the same
//! names.

mod convert;
mod run;

use nbdime_core::diff_format::{diff_from_value, diff_to_value};
use nbdime_core::{diffing, merging, options, Value};
use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;

use convert::{from_ignores, from_py, to_py, Doc};
use run::run;

create_exception!(
    nbdime_rs,
    EngineError,
    PyException,
    "The engine could not diff, patch or merge the given documents."
);

fn dict_option(
    name: &str,
    arg: Option<&Bound<'_, PyAny>>,
    convert: fn(&Bound<'_, PyAny>) -> PyResult<Doc>,
) -> PyResult<Option<Doc>> {
    let Some(arg) = arg.filter(|arg| !arg.is_none()) else {
        return Ok(None);
    };
    if arg.cast::<pyo3::types::PyDict>().is_err() {
        return Err(EngineError::new_err(format!("{name} must be a dict")));
    }
    convert(arg).map(Some)
}

fn value(doc: &Option<Doc>) -> Option<&Value> {
    doc.as_deref()
}

fn nesting(docs: &[Option<usize>]) -> usize {
    docs.iter().flatten().copied().max().unwrap_or(0)
}

fn depth_of(doc: &Option<Doc>) -> Option<usize> {
    doc.as_ref().map(Doc::depth)
}

/// The diff of two lists, dicts or strings, as nbdime's `diff`.
#[pyfunction]
fn diff<'py>(
    py: Python<'py>,
    a: &Bound<'py, PyAny>,
    b: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    let (a, b) = (from_py(a)?, from_py(b)?);
    let result = run(py, a.depth().max(b.depth()), || {
        Ok(diff_to_value(&diffing::diff(&a, &b)?))
    })?;
    to_py(py, result)
}

/// `obj` with `diff` applied, as nbdime's `patch`.
#[pyfunction]
fn patch<'py>(
    py: Python<'py>,
    obj: &Bound<'py, PyAny>,
    diff: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    let (obj, diff) = (from_py(obj)?, from_py(diff)?);
    let result = run(py, obj.depth().max(diff.depth()), || {
        Ok(nbdime_core::patch::patch(&obj, &diff_from_value(&diff)?)?)
    })?;
    to_py(py, result)
}

/// The diff of two notebooks (`diff_notebooks`) with the differ configured by `ignores` (a dict
/// of path to `True`, `False` or a collection of keys) and `targets` (a dict of the keyword arguments
/// of `set_notebook_diff_targets`).
#[pyfunction]
#[pyo3(signature = (a, b, *, ignores=None, targets=None))]
fn diff_notebooks<'py>(
    py: Python<'py>,
    a: &Bound<'py, PyAny>,
    b: &Bound<'py, PyAny>,
    ignores: Option<&Bound<'py, PyAny>>,
    targets: Option<&Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    let (a, b) = (from_py(a)?, from_py(b)?);
    let ignores = dict_option("ignores", ignores, from_ignores)?;
    let targets = dict_option("targets", targets, from_py)?;
    let depth = nesting(&[
        Some(a.depth()),
        Some(b.depth()),
        depth_of(&ignores),
        depth_of(&targets),
    ]);
    let result = run(py, depth, || {
        let config = options::notebook_diff_config(value(&ignores), value(&targets))?;
        Ok(diff_to_value(&diffing::diff_notebooks(&a, &b, &config)?))
    })?;
    to_py(py, result)
}

/// The merge decisions for three generic documents. `strategies` maps paths to strategy names,
/// `transients` lists paths of values that change without meaning.
#[pyfunction]
#[pyo3(signature = (base, local, remote, *, strategies=None, transients=None, fall_back=None))]
fn decide_merge<'py>(
    py: Python<'py>,
    base: &Bound<'py, PyAny>,
    local: &Bound<'py, PyAny>,
    remote: &Bound<'py, PyAny>,
    strategies: Option<&Bound<'py, PyAny>>,
    transients: Option<&Bound<'py, PyAny>>,
    fall_back: Option<&Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    let (base, local, remote) = (from_py(base)?, from_py(local)?, from_py(remote)?);
    let (strategies, transients, fall_back) = (
        dict_option("strategies", strategies, from_py)?,
        transients.map(from_py).transpose()?,
        fall_back.map(from_py).transpose()?,
    );
    let depth = nesting(&[
        Some(base.depth()),
        Some(local.depth()),
        Some(remote.depth()),
        depth_of(&strategies),
        depth_of(&transients),
        depth_of(&fall_back),
    ]);
    let result = run(py, depth, || {
        let strategies =
            options::strategies(value(&strategies), value(&transients), value(&fall_back))?;
        let decisions = merging::decide_merge(&base, &local, &remote, &strategies)?;
        Ok(merging::decisions_to_value(&decisions))
    })?;
    to_py(py, result)
}

/// `base` with the merge `decisions` applied.
#[pyfunction]
fn apply_decisions<'py>(
    py: Python<'py>,
    base: &Bound<'py, PyAny>,
    decisions: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    let (base, decisions) = (from_py(base)?, from_py(decisions)?);
    let result = run(py, base.depth().max(decisions.depth()), || {
        let Value::List(decisions) = &*decisions else {
            return Err("decisions must be a list".into());
        };
        let decisions = merging::decisions_from_values(decisions)?;
        Ok(merging::apply_decisions(&base, &decisions)?)
    })?;
    to_py(py, result)
}

/// The merge decisions for three notebooks. `args` has the `nbmerge` options (`merge_strategy`,
/// `input_strategy`, `output_strategy`, `ignore_transients`).
#[pyfunction]
#[pyo3(signature = (base, local, remote, args=None, *, ignores=None, targets=None))]
fn decide_notebook_merge<'py>(
    py: Python<'py>,
    base: &Bound<'py, PyAny>,
    local: &Bound<'py, PyAny>,
    remote: &Bound<'py, PyAny>,
    args: Option<&Bound<'py, PyAny>>,
    ignores: Option<&Bound<'py, PyAny>>,
    targets: Option<&Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    let inputs = NotebookMerge::new(base, local, remote, args, ignores, targets)?;
    let result = run(py, inputs.depth(), || {
        let decisions = merging::decide_notebook_merge(
            &inputs.base,
            &inputs.local,
            &inputs.remote,
            options::merge_args(value(&inputs.args))?.as_ref(),
            &options::notebook_diff_config(value(&inputs.ignores), value(&inputs.targets))?,
        )?;
        Ok(merging::decisions_to_value(&decisions))
    })?;
    to_py(py, result)
}

/// The merged notebook and the decisions that produced it, as a tuple.
#[pyfunction]
#[pyo3(signature = (base, local, remote, args=None, *, ignores=None, targets=None))]
fn merge_notebooks<'py>(
    py: Python<'py>,
    base: &Bound<'py, PyAny>,
    local: &Bound<'py, PyAny>,
    remote: &Bound<'py, PyAny>,
    args: Option<&Bound<'py, PyAny>>,
    ignores: Option<&Bound<'py, PyAny>>,
    targets: Option<&Bound<'py, PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    let inputs = NotebookMerge::new(base, local, remote, args, ignores, targets)?;
    let (merged, decisions) = run(py, inputs.depth(), || {
        let (merged, decisions) = merging::merge_notebooks(
            &inputs.base,
            &inputs.local,
            &inputs.remote,
            options::merge_args(value(&inputs.args))?.as_ref(),
            &options::notebook_diff_config(value(&inputs.ignores), value(&inputs.targets))?,
        )?;
        Ok((merged, merging::decisions_to_value(&decisions)))
    })?;
    Ok(pyo3::types::PyTuple::new(py, [to_py(py, merged)?, to_py(py, decisions)?])?.into_any())
}

/// The converted arguments of a notebook merge.
struct NotebookMerge {
    base: Doc,
    local: Doc,
    remote: Doc,
    args: Option<Doc>,
    ignores: Option<Doc>,
    targets: Option<Doc>,
}

impl NotebookMerge {
    fn new(
        base: &Bound<'_, PyAny>,
        local: &Bound<'_, PyAny>,
        remote: &Bound<'_, PyAny>,
        args: Option<&Bound<'_, PyAny>>,
        ignores: Option<&Bound<'_, PyAny>>,
        targets: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let args = dict_option("args", args, from_py)?;
        if let Some(Value::Object(options)) = value(&args) {
            let known = [
                "merge_strategy",
                "input_strategy",
                "output_strategy",
                "ignore_transients",
            ];
            if let Some(key) = options.keys().find(|key| !known.contains(&key.as_str())) {
                return Err(EngineError::new_err(format!(
                    "unexpected merge argument {key:?}"
                )));
            }
        }
        Ok(NotebookMerge {
            base: from_py(base)?,
            local: from_py(local)?,
            remote: from_py(remote)?,
            args,
            ignores: dict_option("ignores", ignores, from_ignores)?,
            targets: dict_option("targets", targets, from_py)?,
        })
    }

    fn depth(&self) -> usize {
        nesting(&[
            Some(self.base.depth()),
            Some(self.local.depth()),
            Some(self.remote.depth()),
            depth_of(&self.args),
            depth_of(&self.ignores),
            depth_of(&self.targets),
        ])
    }
}

#[pymodule]
fn nbdime_rs(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("ENGINE_VERSION", env!("CARGO_PKG_VERSION"))?;
    m.add("NBDIME_COMPAT", "4.0.4")?;
    m.add("EngineError", m.py().get_type::<EngineError>())?;
    m.add_function(wrap_pyfunction!(diff, m)?)?;
    m.add_function(wrap_pyfunction!(patch, m)?)?;
    m.add_function(wrap_pyfunction!(diff_notebooks, m)?)?;
    m.add_function(wrap_pyfunction!(decide_merge, m)?)?;
    m.add_function(wrap_pyfunction!(apply_decisions, m)?)?;
    m.add_function(wrap_pyfunction!(decide_notebook_merge, m)?)?;
    m.add_function(wrap_pyfunction!(merge_notebooks, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::{PyDict, PyTuple};

    fn eval<'py>(py: Python<'py>, code: &str) -> Bound<'py, PyAny> {
        py.eval(&std::ffi::CString::new(code).unwrap(), None, None)
            .unwrap()
    }

    #[test]
    fn diff_and_patch_round_trip() {
        Python::attach(|py| {
            let (a, b) = (
                eval(py, "{'a': [1, 2], 's': 'x\\ny\\n'}"),
                eval(py, "{'a': [1, 3]}"),
            );
            let d = diff(py, &a, &b).unwrap();
            let patched = patch(py, &a, &d).unwrap();
            assert!(patched.eq(&b).unwrap());
        });
    }

    #[test]
    fn engine_failures_are_engine_errors() {
        Python::attach(|py| {
            let a = eval(py, "{'cells': 'nope'}");
            let err = diff_notebooks(py, &a, &a, None, None).unwrap_err();
            assert!(err.is_instance_of::<EngineError>(py));
            let targets = eval(py, "{'bogus': True}");
            let err = diff_notebooks(py, &a, &a, None, Some(&targets)).unwrap_err();
            assert!(err.is_instance_of::<EngineError>(py));
        });
    }

    #[test]
    fn notebook_merge_returns_the_merged_notebook_and_decisions() {
        Python::attach(|py| {
            let nb = |meta: &str| {
                eval(
                    py,
                    &format!(
                        "{{'cells': [], 'metadata': {meta}, 'nbformat': 4, 'nbformat_minor': 5}}"
                    ),
                )
            };
            let (base, local, remote) = (nb("{}"), nb("{}"), nb("{'x': 1}"));
            let out = merge_notebooks(py, &base, &local, &remote, None, None, None).unwrap();
            let out = out.cast::<PyTuple>().unwrap();
            let merged = out.get_item(0).unwrap();
            assert!(merged.eq(&remote).unwrap());
            let decisions =
                decide_notebook_merge(py, &base, &local, &remote, None, None, None).unwrap();
            assert!(decisions.eq(out.get_item(1).unwrap()).unwrap());
            let applied = apply_decisions(py, &base, &decisions).unwrap();
            assert!(applied.eq(&remote).unwrap());
        });
    }

    #[test]
    fn generic_merge_takes_strategies() {
        Python::attach(|py| {
            let (base, local, remote) = (
                eval(py, "{'a': 1}"),
                eval(py, "{'a': 2}"),
                eval(py, "{'a': 3}"),
            );
            let conflict = decide_merge(py, &base, &local, &remote, None, None, None).unwrap();
            assert!(conflict
                .get_item(0)
                .unwrap()
                .get_item("conflict")
                .unwrap()
                .is_truthy()
                .unwrap());
            let strategies = PyDict::new(py);
            strategies.set_item("/a", "use-local").unwrap();
            let resolved = decide_merge(
                py,
                &base,
                &local,
                &remote,
                Some(strategies.as_any()),
                None,
                None,
            )
            .unwrap();
            let first = resolved.get_item(0).unwrap();
            assert!(!first.get_item("conflict").unwrap().is_truthy().unwrap());
            assert_eq!(
                first
                    .get_item("action")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "local"
            );
        });
    }

    #[test]
    fn unsupported_arguments_are_type_errors() {
        Python::attach(|py| {
            let (good, bad) = (eval(py, "[1]"), eval(py, "(1,)"));
            assert!(diff(py, &good, &bad)
                .unwrap_err()
                .is_instance_of::<pyo3::exceptions::PyTypeError>(py));
        });
    }
}
