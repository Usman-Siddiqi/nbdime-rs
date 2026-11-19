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
  The same happens **within a single call**, also for the generic `nbdime.diff`: whenever the
  same starred path holds a list in one place and a dict with a type-changed value in another.
  In a seeded random corpus of 6,000 JSON pairs, 617 (10%) raise this error.
- **Impact**: long-running processes (Jupyter server extension, library users) can fail
  nondeterministically depending on history.
- **Decision**: do not replicate. nbdime-rs never raises here and produces the `replace`
  upstream intends. The parity oracle for generic diffs is upstream with a predicate table whose
  lookups do not insert keys (`parity/harness.py: generic_diff_fixed`);
  `test_pilot_f001_fix_is_neutral` checks that this only changes cases where upstream raises.
- **Upstream report**: candidate. Not reported (needs approval).

### F-002: `format_merge_render_lines` duplicates the common last line
- **What**: the loop that extracts equal trailing lines increments (`i += 1`, `j += 1`) instead
  of decrementing, so at most one line is extracted, and the slice `local[:i+1]` keeps it, so the
  line is printed both inside the conflict block and after it.
- **Evidence**: code reading of `prettyprint.py` lines ~214-226. To be confirmed with a test
  case when the merge pretty-printer is ported.
- **Decision**: pending. Probably replicate for byte parity of `nbmerge` output and record, since
  it only affects the builtin renderer (used when git is unavailable). Revisit in Phase 2.

### F-006: `bruteforce_compute_snakes` never merges consecutive pairs
- **What**: while building snakes from the LCS index pairs, upstream extends the last snake when
  `snakes[-1][0] == i and snakes[-1][1] == j`, comparing the snake's *start* with the new pair
  instead of its end. Only a pair at (0, 0) of the rectangle ever extends a snake; every other
  pair becomes a snake of length 1.
- **Evidence**: `compute_snakes_multilevel(["a","bb"], ["a","bc"], [first_letter_eq, eq])` returns
  `[(0, 0, 1), (1, 1, 1)]`; the parity case in `test_pilot_snakes_multilevel_lines` observes the
  unmerged runs (`[5,6,1],[6,7,9]` where `[5,6,10]` would be the merged form).
- **Impact**: none on diffs, since `compute_diff_from_snakes` treats contiguous snakes exactly
  like one long snake. Only the snake list itself differs from what the comments promise.
- **Decision**: replicated, because the parity suite compares the snake list. Harmless; not worth
  reporting upstream on its own.

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
- **Q-002** (answered): `nbformat.read` rejects a UTF-8 BOM (`NotJSONError`, `nbdiff` exits 1
  with a traceback). Proposed deviation: accept and ignore a BOM, since editors on Windows
  sometimes add one. The JSON layer itself rejects it, like `json.loads`; the notebook reader
  (U02) decides.
- **Q-003**: nbformat v3 and older inputs: port `nbformat.convert` upconversion, or shell out /
  reject with a clear error? Proposed: port v3→v4 conversion in Phase 2 (small, well specified).
