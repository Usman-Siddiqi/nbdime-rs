# Findings: upstream bugs, deliberate deviations, open questions

Each entry: what, evidence, impact, decision. Nothing here has been reported upstream; reporting
anything requires explicit approval from the project owner (see CLAUDE.md).

## Upstream bugs

### F-001: `diff_dicts` raises depending on earlier calls (predicate table mutated by lookups)
- **What**: `diff_dicts` raises `RuntimeError("Found predicate(s) for path ... pointing to dict
  entry.")` if `(path or '/') in config.predicates` when a value changes type. `notebook_predicates`
  is a `defaultdict2`, and every `config.predicates[path]` lookup in `diff_lists` **inserts**
  the path. So once any list was diffed at a path, a later dict at the same path with a
  type-changed value raises.
- **Evidence** (nbdime 4.0.4, one process):
  ```python
  a = {"cells": [], "metadata": {"x": {"k": 1}},   "nbformat": 4, "nbformat_minor": 5}
  b = {"cells": [], "metadata": {"x": {"k": "s"}}, "nbformat": 4, "nbformat_minor": 5}
  diff_notebooks(a, b)                       # OK: replace /metadata/x/k
  diff_notebooks({... "x": [1] ...}, {... "x": [2] ...})
  diff_notebooks(a, b)                       # RuntimeError
  ```
  The same can happen within a single diff when a starred path is a list in one cell and a dict
  in another.
- **Impact**: long-running processes (Jupyter server extension, library users) can fail
  nondeterministically depending on history.
- **Decision**: do not replicate. nbdime-rs never raises here and produces the `replace` that
  upstream produces on a fresh process. Parity cases are run on a fresh oracle state per case
  family; cases that trigger the raise are recorded as a known deviation.
- **Upstream report**: candidate. Not reported (needs approval).

### F-002: `format_merge_render_lines` duplicates the common last line
- **What**: the loop that extracts equal trailing lines increments (`i += 1`, `j += 1`) instead
  of decrementing, so at most one line is extracted, and the slice `local[:i+1]` keeps it, so the
  line is printed both inside the conflict block and after it.
- **Evidence**: code reading of `prettyprint.py` lines ~214-226. To be confirmed with a test
  case when the merge pretty-printer is ported.
- **Decision**: pending. Probably replicate for byte parity of `nbmerge` output and record, since
  it only affects the builtin renderer (used when git is unavailable). Revisit in Phase 2.

## Nondeterminism in upstream

### F-003: random cell ids on read (nbformat >= 4.5)
- **What**: `nbformat.read` → `validate` fills missing ids, and replaces duplicate ids, with
  random ids for notebooks declaring `nbformat_minor >= 5`.
- **Impact**: upstream's diff of such a notebook against anything contains random ids; two runs
  differ.
- **Decision (proposed)**: nbdime-rs fills missing/duplicate ids **deterministically** (stable
  hash of cell position and content, same 8-hex-char shape). Parity cases with such inputs compare
  modulo the generated ids (the harness substitutes ids it can map one-to-one). Needs owner
  sign-off; see Q-001.

## Deliberate deviations

### F-004: no `RuntimeError` from F-001 (see above).

### F-005: lone surrogates in JSON strings
- **What**: Python accepts `"\ud800"` and keeps a lone surrogate; Rust `String` cannot hold it.
- **Decision (proposed)**: replace with U+FFFD on input and emit a warning on stderr. Real
  notebooks essentially never contain them; nbformat would fail to write them back as UTF-8 too.

## Open questions

- **Q-001**: accept deterministic ids (F-003)?
- **Q-002**: does `nbformat.read` accept a UTF-8 BOM? Check before porting the reader.
- **Q-003**: nbformat v3 and older inputs: port `nbformat.convert` upconversion, or shell out /
  reject with a clear error? Proposed: port v3→v4 conversion in Phase 2 (small, well specified).
