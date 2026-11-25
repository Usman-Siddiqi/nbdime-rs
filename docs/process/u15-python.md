# Unit U15: the `nbdime_rs` Python extension

Unit spec for implementer and fixer. The orchestrator owns it. One hostile review.

## Scope
New crate `crates/nbdime-py` (PyO3, abi3-py39, `extension-module` feature; a workspace member but
not a default member, so `cargo build`/`cargo test` of the workspace do not need Python), built as
the single extension module `nbdime_rs` (cdylib `nbdime_rs`; no Python files). Packaging with
maturin (`pyproject.toml` in the crate, project name `nbdime-rs`, BSD-3, wheels abi3). Design:
`docs/design.md` "Python-facing boundary"; this spec fixes the signatures:

```python
nbdime_rs.diff(a, b) -> list
nbdime_rs.patch(obj, diff) -> object
nbdime_rs.diff_notebooks(a, b, *, ignores=None, targets=None) -> list
nbdime_rs.decide_merge(base, local, remote, *, strategies=None, transients=None, fall_back=None) -> list
nbdime_rs.apply_decisions(base, decisions) -> object
nbdime_rs.decide_notebook_merge(base, local, remote, args=None, *, ignores=None, targets=None) -> list
nbdime_rs.merge_notebooks(base, local, remote, args=None, *, ignores=None, targets=None) -> (object, list)
nbdime_rs.ENGINE_VERSION: str; nbdime_rs.NBDIME_COMPAT = "4.0.4"; nbdime_rs.EngineError(Exception)
```

- Arguments and results have exactly the shapes and semantics of the `nbdime-rs-dev` functions
  of the same names (`crates/nbdime-cli/src/dev/mod.rs`): `ignores`, `targets`, `args`,
  `strategies`... as there. Move the argument decoding the dev tool does into `nbdime-core` (or a
  small shared module) so both use one implementation; keep the dev tool's behaviour unchanged.
- Conversion walks Python objects directly into `Value` and back (no JSON round trip): `dict`
  with `str` keys (insertion order kept), `list`, `str` (lone surrogates: `TypeError`/
  `ValueError`, see F-005), `int` of any size (Python int semantics as `Value` has them), `float`
  (NaN/inf kept), `bool` (distinct from int), `None`. Subclasses of dict/list/str (e.g.
  nbformat's `NotebookNode`) are accepted. Anything else, including tuples, raises `TypeError`.
  Recursion bounded by document depth as elsewhere (F-009 limits apply, as errors, no crash).
- Engine errors raise `nbdime_rs.EngineError` with the engine's message. Never panic across the
  FFI boundary (catch, convert to EngineError).
- Release the GIL while the core runs (`py.allow_threads`); conversion happens with the GIL held.
- No global state: the notebook differ configuration is passed per call.

## Acceptance
`scripts/gate.sh $(grep -v '^#' parity/enabled_suites.txt | sed 's|^|parity/|') parity/test_u15_python.py`
prints `GATE PASSED` (the suite builds the extension with `PYO3_PYTHON` set to the oracle's
interpreter and re-runs the diff, patch, notebook-diff and merge suites through it). Also
`maturin build --release` (install maturin with `uv tool install maturin` or into a scratch venv,
never into the oracle venv) must produce an abi3 wheel that `pip install`s into a fresh venv and
imports; report the wheel size and import time.
