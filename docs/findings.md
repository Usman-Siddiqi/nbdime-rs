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

### F-006: `test_package.py` asserts on a misspelled module name (minor)
- **What**: `nbdime/tests/test_package.py` line 12 asserts `'ndime.webapp' not in sys.modules`
  (missing `b`), so the check that importing nbdime does not import the web app can never fail.
- **Decision**: none needed for the port. Upstream report candidate (trivial); not reported.

### F-007: `bruteforce_compute_snakes` never merges consecutive pairs
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
- **Decision (approved 2026-11-19)**: nbdime-rs fills missing/duplicate ids **deterministically** (stable
  hash of cell position and content, same 8-hex-char shape). Parity cases with such inputs compare
  modulo the generated ids (the harness substitutes ids it can map one-to-one).

## Deliberate deviations

### F-004: no `RuntimeError` from F-001 (see above).

### F-005: lone surrogates in JSON strings
- **What**: Python accepts `"\ud800"` and keeps a lone surrogate; Rust `String` cannot hold it.
- **First proposal (rejected)**: replace with U+FFFD. Hostile review of the pilot showed this
  silently hides real changes: `["\ud800"]` vs `["\udc00"]` diffs as equal, and two distinct
  keys `"\ud800"`/`"\ud801"` collapse into one, so a removed key is never reported.
- **Decision**: reject input containing lone surrogates with a clear error (the default
  `LoneSurrogates::Reject`). Never report a wrong diff silently. Real notebooks essentially never
  contain them, and nbformat cannot write them back as UTF-8 either. Through the Python bindings,
  a `str` with lone surrogates fails conversion, so the integration falls back to pure Python.

### F-008: NaN identity (parity rule, not a deviation)
- CPython's `json` returns one shared `NaN` object, and container equality short-circuits on
  identity, so `[NaN] == [NaN]` is True while `NaN == NaN` is False. nbdime-rs reproduces this
  (porting guide 2.1). Through the Python bindings, callers can pass distinct NaN objects (e.g.
  `float("nan")` built in code), for which Python's answer would differ; the bindings assume the
  JSON-parsed case. Not worth special handling.

### F-009: nesting depth
- Upstream's recursive diff raises `RecursionError` at a few hundred levels of nesting (Python's
  default recursion limit is 1000 frames, several frames per level); `json.loads` accepts 9998.
- **Decision**: nbdime-rs diffs up to `MAX_DIFF_DEPTH` (500) levels and returns an error beyond
  it, instead of overflowing the stack (which aborts the process). Upstream raises between 400 and
  500 levels, so we succeed slightly further; deliberate. The default configuration fits 500
  levels on a 2 MB stack in debug builds. A custom configuration with multilevel predicates at
  every nesting level needs more stack (overflow at about 450 levels on 2 MB in debug; fine in
  release); the CLI and bindings run the engine on a large-stack thread.

### F-010: integer digit limit
- Python 3.11+ refuses to parse integers with more than 4300 digits (`ValueError`), so
  `json.loads`/`nbformat.read` fail on such files. nbdime-rs reproduces the error.

### F-011: the approximate string predicate is defined on str/str and list/list only
- **What**: upstream's `compare_strings_approximate` is duck-typed: it works on any operands
  with `len()`, `==` and hashable iteration, so with a custom config it compares a str with a
  list, or iterates dicts by key, and raises `TypeError`/`KeyError` in some cases and not others
  (it hashes `y` eagerly, `x` lazily; a full-grid LCS evaluates every pair).
- **Where it matters**: nowhere in nbdime itself. nbdime applies this predicate only to lines of
  strings (and the notebook predicates join text before calling it). Only a user-supplied
  `DiffConfig` that puts it on arbitrary JSON can reach the other cases.
- **Decision**: nbdime-rs supports str/str and list/list operands with upstream's results, and
  returns an error for other operand types. Because nbdime-rs strips a common suffix before the
  LCS and checks length ratios before hashing, some of upstream's errors in these unsupported
  configurations become results instead. The Python integration only uses the engine for
  configurations it can express, so this is never visible through nbdime.
- **Review trail**: raised as major by the P1 sign-off re-review; downgraded by the orchestrator
  because no nbdime configuration reaches it.

## Open questions

- **Q-001** (decided): deterministic ids, see F-003.
- **Q-002** (answered): `nbformat.read` rejects a UTF-8 BOM (`NotJSONError`, `nbdiff` exits 1
  with a traceback). Decision (approved 2026-11-19): accept and ignore a leading BOM, since editors
  on Windows sometimes add one (deliberate deviation). The JSON layer itself rejects it, like `json.loads`; the notebook reader
  (U02) decides.
- **Q-003**: nbformat v3 and older inputs: port `nbformat.convert` upconversion, or shell out /
  reject with a clear error? Proposed: port v3→v4 conversion in Phase 2 (small, well specified).
