# Design

## Goals and constraints

1. Same results as nbdime 4.0.4 for diff, patch and merge, in nbdime's own JSON formats
   (`diff_format.schema.json`, `merge_format.schema.json`), so existing consumers (including the
   web viewer) work unchanged.
2. Same CLI surface for `nbdiff`, `nbshow`, `nbpatch`, `nbmerge` and the git drivers.
3. Usable from Python with nbdime's own function signatures, so upstream can adopt the engine
   behind a thin, optional integration layer.
4. Dramatically faster, by doing less work (see `upstream-performance.md`), not only by being
   compiled.
5. Builds on a weak machine and works offline once dependencies are fetched.

## Workspace

```
Cargo.toml                 workspace; default-members = core + cli
crates/
  nbdime-core/             library, no I/O beyond reading/writing JSON text
    src/
      value.rs             Value, Int, py_eq, same_type, truthiness
      json/parse.rs        Python-json-compatible parser (NaN, big ints, dup keys)
      json/dump.rs         Python-json-compatible serializer (ensure_ascii, indent, float repr)
      py.rs                Python string helpers: splitlines, code point utilities
      path.rs              star_path / split_path / join_path
      nbformat.rs          read normalization (rejoin, strip transient, ids policy)
      diff_format.rs       DiffEntry, Op, Key, builders, validation, JSON (de)serialization
      diff_utils.rs
      difflib.rs           SequenceMatcher (isjunk=None, autojunk=False)
      diffing/
        sequence.rs        brute-force LCS (fast, exact), diff_from_lcs
        snakes.rs          multilevel snakes
        generic.rs         diff, diff_lists, diff_dicts, compare_strings_approximate
        config.rs          DiffConfig as data
        predicates.rs      notebook predicates with precomputed per-item data
        notebooks.rs       diff_notebooks, differs, ignore targets
      patch.rs
      merge/               decisions, chunks, generic merge, strategies, autoresolve, notebooks
  nbdime-cli/              `nbdime` binary (subcommands) + thin `nbdiff`, `nbshow`, ... binaries
    src/pretty/            terminal pretty-printing
    src/git/               diff/merge drivers, config commands
  nbdime-py/               PyO3 extension, Python package `nbdime_rs` (built with maturin)
parity/                    pytest harness; oracle = upstream nbdime (protected)
bench/                     generators, runner, results, chart script
```

Dependencies are kept small: `indexmap`, `thiserror`, `regex`, `ryu` (float digits), `clap`,
`anyhow` (binaries only), `pyo3` (bindings only). Dev: `proptest` where useful.

## Core API (Rust)

```rust
pub fn diff_notebooks(a: &Value, b: &Value, cfg: &NotebookDiffConfig) -> Result<Diff>;
pub fn diff(a: &Value, b: &Value) -> Result<Diff>;                 // generic, default config
pub fn patch(obj: &Value, diff: &Diff) -> Result<Value>;
pub fn merge_notebooks(base: &Value, local: &Value, remote: &Value, args: &MergeArgs)
    -> Result<(Value, Vec<MergeDecision>)>;
pub fn decide_notebook_merge(...) -> Result<Vec<MergeDecision>>;
pub fn apply_decisions(base: &Value, decisions: &[MergeDecision]) -> Result<Value>;
```

`NotebookDiffConfig` is built from the same switches as upstream's `set_notebook_diff_targets`
(`sources`, `outputs`, `attachments`, `metadata`, `identifier`, `details`) plus an explicit
ignore-path table equivalent to `set_notebook_diff_ignores`. No global state.

## Python-facing boundary (`nbdime_rs`)

The boundary is plain Python objects in, plain Python objects out, mirroring nbdime signatures:

```python
nbdime_rs.diff(a, b) -> list
nbdime_rs.patch(obj, diff) -> object
nbdime_rs.diff_notebooks(a, b, *, ignores=None, targets=None) -> list
nbdime_rs.decide_merge(base, local, remote, *, strategies=None, transients=None, fall_back=None) -> list
nbdime_rs.apply_decisions(base, decisions) -> object
nbdime_rs.decide_notebook_merge(base, local, remote, args=None, *, ignores=None, targets=None) -> list
nbdime_rs.merge_notebooks(base, local, remote, args=None, *, ignores=None, targets=None) -> (object, list)
nbdime_rs.ENGINE_VERSION, nbdime_rs.NBDIME_COMPAT = "4.0.4", nbdime_rs.EngineError
```

`ignores` maps paths to `True`/`False` or a list, tuple or set of keys. `targets` has the keyword arguments of
`set_notebook_diff_targets`, `args` the `nbmerge` options; all as in the `nbdime-rs-dev`
functions of the same names. Conversion rules and threading are in F-040 and F-041.

- Conversion walks Python objects directly (dict/list/str/int/float/bool/None) into `Value`; no
  JSON round trip. Unsupported types raise `TypeError`, and the integration layer falls back to
  Python.
- Results are plain `dict`/`list`. The upstream integration wraps them (`to_diffentry_dicts`, and
  `NotebookNode` for values where nbdime callers use attribute access).
- The GIL is released while the Rust core runs.

## Parity harness (`parity/`)

Owned by the orchestrator; implementers may not modify it.

- **Oracle**: upstream nbdime 4.0.4 imported in-process from the reference venv.
- **System under test**: the `nbdime-rs-dev` binary (a hidden dev tool built from `nbdime-cli`)
  in batch mode: it reads JSON Lines `{"id", "fn", "args"}` on stdin and writes
  `{"id", "ok", "result" | "error"}`, so one process serves thousands of cases. Later, the PyO3
  module is tested through the same case files.
- **Case sources**:
  1. Upstream test data: every notebook in `nbdime/tests/files`, diffed pairwise within each
     family (`X--1` vs `X--2`, `base` vs `local`/`remote`, ...), plus each against itself.
  2. Unit-level corpora per function (`difflib_opcodes`, `diff_strings_linewise`,
     `compare_strings_approximate`, generic `diff` of random JSON), from seeded generators that
     deliberately produce near-equal values, non-ASCII and astral characters, every `splitlines`
     boundary, empty values, numeric edge cases.
  3. Differential notebook corpora from `bench/gen_notebooks.py` and mutation of the upstream
     test notebooks.
- **Equivalence**: the default is exact equality of the JSON produced by both sides, parsed and
  compared structurally with **types distinguished** (`1` ≠ `1.0` ≠ `true`) and **list and
  object order significant** (object order is compared too, because output files must match byte
  for byte). For the CLI, outputs are compared byte for byte. Any relaxed equivalence for a unit
  (for example where upstream is nondeterministic) is defined in that unit's test module and
  recorded in `docs/findings.md`.
- **Anti-skip**: each suite asserts a minimum case count, and the gate fails on any skipped or
  xfailed parity test that is not listed in `parity/known_deviations.toml` with a findings
  reference.

## Benchmarks (`bench/`)

- `gen_notebooks.py`: deterministic notebooks of N cells (with and without ids).
- `run.py`: times upstream (`diff_notebooks` in-process and the `nbdiff` CLI end to end) and
  nbdime-rs (CLI and, later, the Python module) over a size ladder; writes
  `bench/results/<date>-<host>.json` with hardware info. A separate script renders charts for the
  README from those files only.

## Upstream integration sketch (deliverable 2)

Minimal, optional, default-off-unless-installed:

```python
# nbdime/_accel.py (new, ~80 lines)
try:
    import nbdime_rs as _engine
except ImportError:
    _engine = None

def enabled():
    return _engine is not None and os.environ.get("NBDIME_ACCEL", "1") != "0" \
        and _engine.NBDIME_COMPAT == <supported range>
```

- `diffing/notebooks.py: diff_notebooks()` uses the engine only when the current differ
  configuration is one that the engine can express. Upstream's configuration is a table of
  callables; the integration records the *declarative* ignore specification each time
  `set_notebook_diff_ignores`/`set_notebook_diff_targets`/`reset_notebook_differ` is called, and
  treats any other mutation of `notebook_differs`/`notebook_predicates` (e.g. a third-party
  differ) as "not expressible" → pure-Python path.
- `merging/notebooks.py: decide_notebook_merge()` likewise, with `args` translated to keywords.
- Results are converted with `to_diffentry_dicts` / `MergeDecision` / `NotebookNode` so callers
  see the same types.
- Tests: upstream's suite runs twice in CI (with and without the engine), plus a small new test
  file checking the switch and the fallback.
- Packaging: `nbdime[accel]` extra pointing at `nbdime-rs` wheels (abi3, manylinux/macOS/Windows).
  nbdime itself stays pure Python.

What upstream maintainers are *not* asked to take on: Rust code, a Rust toolchain in their CI
(wheels come from PyPI), or changes to their public API.
