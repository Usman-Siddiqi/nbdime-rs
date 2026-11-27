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

### F-014: `diff_notebooks` reorders the caller's output dicts
- **What**: `diff_single_outputs` does `a.pop('data')` ... `a.data = tmp_data` on the caller's
  outputs, which moves `data` to the end of their key order. It runs during cell alignment
  (`compare_outputs_approximate` at the moderate level), not only on the final pairs, so
  outputs that end up as values in the diff (an `addrange` of cells) show `data` last, and the
  caller's notebooks are modified too. Visible in `nbdiff --out` JSON. Reproduction:
  `docs/process/u04-notebook-diff.md` and `test_u04_f014_fix_is_neutral`.
- **Impact**: cosmetic (JSON key order), but a diff function should not mutate its inputs, and
  which outputs are affected depends on which predicate pairs were evaluated, so a faster
  aligner cannot reproduce it without evaluating every pair upstream evaluates.
- **Decision**: deviation. nbdime-rs never mutates inputs. The oracle wraps
  `diff_single_outputs` with shallow copies; the neutrality test shows the results are equal
  with keys sorted. Candidate for an upstream report (ask the owner first).

### F-016: `identifier=False` does not ignore cell ids
- **What**: `set_notebook_diff_targets(identifier=False)` sets the differ of `/cells/*/id` to
  `diff_ignore`, but `/cells/*/id` is also in `atomic_paths`, so `diff_dicts` replaces atomic
  values without consulting the differ. A changed cell id is therefore always reported.
- **Evidence** (nbdime 4.0.4): two notebooks that differ only in `cells[0].id` give
  `[patch cells [patch 0 [replace id]]]` with default targets, with `identifier=False` and with
  `set_notebook_diff_ignores({"/cells/*/id": True})`.
- **Decision**: replicated (parity), since the flag is observable through `nbdiff`. Candidate for
  an upstream report (ask the owner first).
- **Terminal output**: upstream's pretty-printer (`PrettyPrintConfig.should_ignore_path`) hides
  `/cells/*/id` when `id=False`, so `nbdiff -I` looks right in the terminal while the id still
  appears in the JSON (`--out`) and web output. The engine reproduces the diff; the U06
  pretty-printer unit reproduces the hiding.

### F-017: shapes that are not nbformat-shaped
- **What**: the notebook predicates are duck-typed upstream: a cell `source` that is a list or
  dict, a stream `text` that is a list in one output and a string in the other, a `traceback`
  that is a string, a list or dict `text/*` mime value, a falsy non-list `outputs` (`""`), or
  `outputs` that is a non-list truthy value sometimes yield results (`compare_text_strict`
  joins lists; `compare_strings_approximate` iterates any sequence) and sometimes raise
  (`compare_text_approximate` is `lru_cache`d, so list and dict arguments raise `TypeError`
  before the body runs). nbformat's reader never produces such shapes, but `json.load` does:
  the on-disk form has list sources, and upstream diffs those pairs when they are identical
  or when only one side is a list.
- **Decision**: nbdime-rs supports the shapes nbformat produces (strings where nbformat joins
  lines, lists of strings for tracebacks) with upstream's results, and returns an error for the
  other shapes, including where upstream happens to return a value. Missing keys and the
  predicates that raise upstream raise here too. Further differences:
  - The brute-force LCS strips a common suffix of aligned items before evaluating the
    predicate grid (see `docs/upstream-performance.md`). The stripped pairs are still compared
    with their partner; only their comparisons with the other items are skipped, so a
    malformed item there can avoid an error that upstream raises.
  - Whether outputs align is decided from the alignment alone (`compare_outputs_approximate`):
    the diffs of aligned output pairs, which upstream computes and discards, are not computed.
    Those can only raise for shapes outside nbformat's schema (or nesting beyond Python's recursion
    limit, F-009), so the engine may return a
    result where upstream raises.
  - Identical notebooks with list sources, which upstream diffs without error, are an error
    here ("text operands must be strings"). Notebook merges (`decide_notebook_merge`,
    `merge_notebooks`) inherit this: they fail on notebooks whose sources are lists of lines,
    where upstream merges.
- **Integration requirement**: upstream callers often pass `json.load` output (on-disk form,
  list sources). Differential run on 300 such pairs: the engine errored where upstream
  succeeded on 28. The PyO3 integration must fall back to the Python implementation when the
  engine returns an error (or rejoin list sources into strings before calling it).
- Found by a differential run of 7,500 predicate and 1,500 notebook cases built from
  deliberately malformed cells and outputs: no case produced a different result, only these
  error/no-error differences.

### F-019: `nbdiff a.ipynb a.ipynb` crashes on a JSON-scalar mime value
- **What**: `add_mime_diff` sends every `application/json` (and `*+json`, `text/*`, ...) value to
  the generic `diff`, which raises `RuntimeError: Can currently only diff list, dict, or str
  objects.` for numbers, bools and null, even when the two values are equal. The notebook schema
  allows any JSON value for JSON mime types, so a valid notebook (an output or attachment with
  `"application/json": 1`) cannot be diffed, not even against itself. Values of different kinds
  (`"a"` vs `["a"]`) raise the same way. Found by the U04 hostile review.
- **Decision**: deviation. Values the generic diff cannot recurse into are replaced when they
  differ (`!=`), which is what `add_mime_diff` does for non-split mime types. The parity oracle
  applies the same fix; `test_u04_f019_fix_is_neutral` shows it changes nothing where upstream
  does not raise. Candidate for an upstream report (ask the owner first).

### F-020: output metadata ignores do not apply to display data
- **What**: `diff_single_outputs` diffs the part of a `display_data`/`execute_result` output
  other than `data` (its `metadata`, `execution_count`) with the default configuration, not the
  notebook one. So `nbdiff -M` (`targets` with `metadata=False`) and ignores on
  `/cells/*/outputs/*/metadata` (or `.../data`) never hide those outputs' metadata changes.
- **Evidence** (nbdime 4.0.4): one cell with a `display_data` output whose metadata is
  `{"image/png": {"width": 100}}` against `{"image/png": {"width": 200}}`, `targets` with
  `metadata=False`: both report the replace. Upstream's pretty-printer may hide some of these
  paths in the terminal; the U06 unit handles that.
- **Decision**: replicated (parity). Candidate for an upstream report (ask the owner first).

## Nondeterminism in upstream

### F-003: random cell ids on read (nbformat >= 4.5)
- **What**: `nbformat.read` → `validate` fills missing ids, and replaces duplicate ids, with
  random ids for notebooks declaring `nbformat_minor >= 5`.
- **Impact**: upstream's diff of such a notebook against anything contains random ids; two runs
  differ.
- **Decision (approved 2026-11-19)**: nbdime-rs fills missing/duplicate ids **deterministically**,
  same 8-hex-char shape. Refined for U02: the hash covers the cell's content only, not its
  position (FNV-1a 64 of the canonical cell JSON, suffixed `:1`, `:2`, ... on collision; exact
  rule in `docs/process/u02-nbformat.md`), so an unchanged cell keeps its id when cells are
  inserted before it and a diff of two such notebooks shows no spurious id changes. Parity cases
  with repaired ids compare modulo those ids, and check that they are 8 hex chars, unique and
  stable.

### F-021: random ids on inline-cells conflict marker cells
- **What**: the `inline-cells` merge strategy inserts markdown marker cells made with
  `nbformat.v4.new_markdown_cell`, which draws a random id (`uuid4`) for each.
- **Impact**: `nbmerge` output of a conflicted notebook differs from run to run.
- **Decision**: deterministic, as F-003: 8 lowercase hex chars from a hash of the marker cell's
  content and position, distinct from every other id in the notebook. The U10 suite compares
  merges with marker ids masked and checks shape and distinctness (`compare_modulo_marker_ids`).

### F-022: upstream's text rendering depends on the installed git and its configuration
- **What**: with git on `PATH`, upstream renders modified multi-line strings with
  `git diff --no-index` and resolves inline source conflicts with `git merge-file`
  (`prettyprint.py`). The output therefore depends on the git version and on the user's system
  and global git config (`diff.indentHeuristic`, `diff.algorithm`, `color.diff.*`,
  `core.whitespace`, `merge.conflictStyle`, ...). Without git it falls back to `diff`/`diff3` or
  difflib, which produce different text again.
- **Decision (2026-11-20, revised)**: nbdime-rs runs git exactly as upstream does (same commands,
  temp files, fallbacks to `diff3`/`diff` and the builtin renderers), so text rendering honours
  the user's git version and configuration just like upstream. An in-process reproduction was
  tried (U16) and withdrawn for licensing reasons (F-023). The parity harness pins git 2.43 with no
  system or global config (`parity/conftest.py`), which the engine inherits. Merge rendering: F-026.
- Related upstream quirk replicated: `merge_render_with_git` raises `IndexError` when git's output
  is empty (all three inputs empty, or empty merge result); the engine returns an error there.

### F-024: the `clear-all` output strategy crashes on some conflicts
- **What**: `nbmerge --merge-strategy inline --output-strategy clear-all` (and any merge using
  `collect_diffs` in `merging/strategies.py`) raises `TypeError: 'NoneType' object is not
  iterable` when a one-sided decision sits next to a conflict in the same outputs list:
  `adjust_patch_level` returns `None` for it. Patching that to skip `None` only moves the crash
  (`combine_patches` then sorts int and str keys together), so there is no small local fix.
  Reproductions: two upstream test-notebook family triples with `output_strategy: clear-all`
  (`scratchpad/u10/mid_32.json`, `mid_33.json`; regenerate from `family_triples()`).
- **Decision**: replicate (the engine returns an error where upstream raises) for now; revisit in
  Phase 3 with a correct `clear-all` semantics if one can be defined. Candidate for an upstream
  report (ask the owner first).

### F-027: default `nbmerge` crashes when both sides append different outputs
- **What**: `resolve_strategy_inline_outputs` passes `outputs[key] if outputs else None` as the
  base output; for a conflict at the end of a non-empty outputs list `key == len(outputs)`, so it
  raises `IndexError: list index out of range`. This is the default `nbmerge` path (inline
  strategy) and a realistic case: a cell with output `a`, re-run on both branches, local appends
  `L`, remote appends `R`. With empty base outputs it works. Found by the U10 hostile review.
- **Decision**: deviation. There is no base output at the end of the list, so `None` is passed,
  as upstream already does for an empty list: the conflict is inlined with markers (only inserts,
  so the base is kept). The parity oracle applies the same fix (`merge_upstream` in
  `parity/test_u10_merge.py`); `test_u10_f027_fix_is_neutral` shows it changes nothing where
  upstream does not raise. Candidate for an upstream report (ask the owner first).

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
  release). The CLI uses a large-stack thread and the Python bindings use a larger stack when needed (F-041).

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

### F-012: nbformat v1-v3 input is rejected
- Upstream upconverts v1-v3 notebooks with `nbformat.convert`. nbdime-rs supports v4 only and
  returns an error that names the version and suggests `jupyter nbconvert --to notebook` (which
  upgrades). v3 was superseded in 2015; the converters are large and rarely exercised. A missing
  `nbformat` key (upstream: v1) and non-integer versions (`4.0`; upstream: `AssertionError`) are
  errors too. Resolves Q-003. Can be revisited in Phase 4 if users ask.
- The error message depends on the version: 1 to 3 suggest the conversion above, 5 and above say
  the notebook is newer than supported, anything else says `"nbformat"` must be the integer 4.

### F-013: no schema validation messages
- `nbformat.read` validates against the JSON schema and logs "Notebook JSON is invalid" to
  stderr, then carries on. nbdime-rs does not validate against the schema (only the id repair
  part of validation has an effect on the data), so it prints nothing. stdout is unaffected.
- One schema failure is not survivable upstream: when the first schema error is a `oneOf` failure
  on a cell whose `cell_type` is not a string (nbformat minor <= 5), `better_validation_error`
  computes `cell_type + "_cell"` and raises `TypeError` out of `nbformat.reads`/`writes`. Whether
  it fires depends on which schema error comes first, so nbdime-rs does not try to reproduce it
  and accepts such (garbage) notebooks, as it does for every other schema violation. In a fuzz
  run over about 30,000 notebooks with random scalar values in every field this was the only
  difference found besides the documented deviations (F-003, F-012, Q-002).

### F-018: deep nesting in notebooks
- Upstream fails with `RecursionError` at about 1,000 levels of nesting in `nbformat.reads`
  (`from_dict`) and at about 500 levels in `nbformat.writes` (`copy.deepcopy`). nbdime-rs reads
  and writes anything the JSON parser accepts, up to 9,998 levels (F-009 covers the diff).
- **Decision**: accept (deliberate), as with F-009. `Value`'s derived `Clone`, its drop glue and some
  operations recurse, so library callers on small stacks (the default 2 MB for a spawned thread)
  can overflow on values nested several thousand levels deep. The CLI and the Python bindings
  provide extra stack space for the engine (F-029, F-041). `writes` clones the whole notebook once, as the
  spec allows; cloning only the levels that change is not possible without sharing, which
  `Value` does not do.

### F-015: diff keys and lengths in patch and flatten input
Deliberate deviations; no differ produces any of these, and each is an error here.
- List keys and `removerange` lengths must be non-negative integers that fit `i64`. Upstream also
  accepts `true`/`false` (Python bools are ints; as a length they count as 1/0), negative
  integers (slicing from the end; a negative `removerange` length duplicates items), floats
  (some work through int/float arithmetic) and integers above `i64::MAX` (slices clamp).
- Merged `removerange` lengths in `flatten_list_of_string_diff` must fit `u64`. Upstream's exact
  integer can be larger (three lengths of `i64::MAX` give 27670116110564327421); an end beyond
  `u64` is an error, as upstream's is, since it can never equal the next key.
- `flatten_list_of_string_diff` parses its diff eagerly, so unknown ops, odd keys or non-list
  inner diffs that upstream would copy through untouched are errors. For `patch` and
  `patch_*`, missing fields, unknown ops and wrong types are errors, as upstream.
- `patch` rejects a non-list empty iterable (`{}` or `""`) as a diff; upstream accepts it.
- The dev tool's `flatten_list_of_string_diff` requires every line of `a` to be a string.

### F-023: licensing of the git-compatible diff and merge (U16)
- git's xdiff is LGPL-2.1+ and git is GPL-2.0; nbdime-rs is BSD-3-Clause. U16 does not translate
  git's C sources. It may use permissively licensed crates (e.g. `imara-diff`, Apache-2.0) and
  read `gix-merge` (MIT/Apache) as a reference, and uses the `git` binary only as a black-box
  oracle. Output formatting is written from scratch against git's observed output.
- **2026-11-20, decision**: the U16 hostile review found that the implementation of `myers.rs`,
  `compact.rs` and `merge.rs` translated xdiff function by function despite this rule. The branch
  was never pushed or merged, and it has been deleted. U16 is withdrawn: nbdime-rs spawns git
  like upstream (F-022), which keeps the BSD license clean and gives exact parity. A clean-room
  in-process renderer can be reconsidered in Phase 4 only from a behaviour spec, by an author who
  has not read git's sources.

### F-025: `copy.copy(notebook_config)` drops the configured ignores (U10)
Upstream `_split_addrange` aligns inserted items with `copy.copy(notebook_config)`.
`defaultdict2.copy` builds the copy through `defaultdict.copy`, which passes the original dict as
`default_values` and then replaces it, so every key set on `notebook_differs` after import
(`set_notebook_diff_ignores` / `set_notebook_diff_targets`) is lost in the copy. Inserts are
therefore always aligned with the default notebook differs, even when the notebook diffs were
made with sources, outputs, etc. ignored. Reproduced as is (the decision list depends on it);
upstream's intent was presumably to honour the configured ignores. Candidate for an upstream report
(ask the owner first).

### F-026: `merge_render` runs the same external tools as upstream (U10)
`merging::text::merge_render` is the implementation of upstream's `prettyprint.merge_render`, not
a stand-in: the in-process git merge (U16) is not coming for now (F-022's plan for it is on hold).
It follows upstream's fallback chain exactly. With `git` on `PATH` it runs `git merge-file -p
local base remote` (plus `--ours`, `--theirs` or `--union` for the `use-local`, `use-remote` and
`union` strategies; `use-base` returns the base); otherwise, with `diff3` on `PATH`, `diff3 -m
local base remote`; otherwise the built-in renderer, which calls any difference between the
sides one conflict (`builtin_merge_render` and `format_merge_render_lines`, quirks included: the
doubled final newline, and a common last line that is repeated after the conflict).
- The tools run in a private temporary directory (mode 0700, unpredictable name, files created
  exclusively with mode 0600; removed on every path), and inherit the caller's environment. The
  library sets no `GIT_CONFIG_*` variable: like upstream, it honours the user's git configuration
  (`merge.conflictStyle`, `core.autocrlf`, ...). The parity harness pins git's configuration in
  its own environment (`parity/conftest.py`), which the engine inherits, so parity runs see git's
  defaults.
- Same results as upstream, hence the same dependence on the installed tool: the three renderers
  produce different text for the same conflict (diff3 shows the base section, the built-in one
  has no merge at all), and a clean merge in git can be a conflict in the built-in renderer.
- `\r\n` in the tool's output becomes `\n`; git output that is empty is an error (upstream's
  `IndexError`), with git's standard error text added to the message. A tool that cannot be run
  or whose output is not UTF-8 is an error as well (upstream raises).
- Upstream logs a warning when a strategy is ignored; the engine does not print.

### F-028: path keys that Python accepts and the engine rejects (U10)
- **Decision (deliberate, same class as F-015)**: `common_path` entries of a decision must be
  non-negative integers that fit `i64`, or strings. Upstream also accepts negative integers
  (`resolved[-1]` counts from the end of a list) and booleans (ints in Python) when it resolves
  the path in `apply_decisions`. `decide_merge` and the notebook merge never produce such paths;
  only hand-written decisions fed to `apply_decisions` can, and they are an error here.
- **`build_diffs(Which::Merged)` always fails**: upstream's `build_diffs(base, decisions, which)`
  with `which="merged"` resolves each decision with `resolve_action(base[path], md)`, where
  `path` is a tuple: `KeyError` on a dict, `TypeError` on a list. The only caller, the
  `autoresolve` module (itself unused by the rest of upstream), asks for `local` and `remote`
  only, so nothing in upstream reaches it. The engine returns an error for it too rather than
  inventing a meaning. Candidate for an upstream cleanup (ask the owner first).

### F-029: command-line surface that is not byte-identical
- `--version` prints nbdime-rs's own version (`nbdiff 0.x.y (nbdime-rs; compatible with nbdime
  4.0.4)`), and `--help` and usage texts are written by hand to match argparse closely but are not
  compared byte for byte. `--config` (stderr, exit 1) is byte-identical to upstream: sorted keys,
  the same entries, and `nbdime --config` lists all eleven entry points (`NbDiff`, `NbDiffWeb`,
  `NbMerge`, `NbMergeWeb`, `NbShow`, `Server`, `Extension` and the four git driver/tool classes).
- The Jupyter config path has no Python prefix: `$VIRTUAL_ENV` or `$CONDA_PREFIX` stand in for
  `sys.prefix/etc/jupyter`. Upstream uses `sys.prefix` of the interpreter that runs nbdime, which
  ignores `VIRTUAL_ENV`, so a differing `VIRTUAL_ENV` is the one place the config path can differ.
- Where upstream crashes with a traceback, nbdime-rs exits 1 with a one-line error.
- Subcommands of the `nbdime` dispatcher: `extensions` and `reg-extensions` are implemented with
  upstream's messages (`--enable`/`--disable` run the four `jupyter` commands and ignore their
  statuses). `show`, `diff-web`, `merge-web`, `server` and `config-git` are not ported; they exit 1
  with our own "does not provide" message.
- **Non-UTF-8 names**: argv and file names are kept as bytes. Undecodable bytes are held internally
  as the code points U+10F780..U+10F7FF (the analogue of `surrogateescape`) and written back as the
  original bytes when a path is opened. Upstream reconfigures stdout and stderr with
  `backslashreplace`, so such a name prints as `\udcXX` on both streams; the port does the same
  (this overrides the earlier plan to emit raw bytes on stdout). The escaping is applied where a
  name or argument is formatted (diff headers, `Missing file`, log and error messages, the
  mergetool `repr`), never to a stream, so notebook text in that private-use range is printed
  unchanged. A genuine character of that range inside a name is indistinguishable from an escaped
  byte, and prints as `\udcXX`.
- **Undecodable file names inside a repository** (a revision diff, or the working tree against a
  revision): upstream's GitPython stdout pump crashes on the name (a traceback on stderr, empty
  stdout, exit status 0). nbdime-rs prints the correct diff instead. Deliberate deviation; it looks
  like an upstream bug (not reported; ask the owner first).
- **Stack**: the commands run on a thread with a 512 MiB stack. When that does not map (`ulimit
  -v` or similar), the first size that does (128, 32, 8, 2 MiB) is only a probe and the run starts
  from the next smaller one, so the heap keeps some address space; the main thread is the last
  resort.
- Config values of the wrong type fail with exit 1 where upstream misuses them; the shapes that
  traitlets accepts (`""`, a list of pairs, a wrong-length `dict(...)` element) are reproduced.
  Exotic mistyped values beyond those listed in F-031 may differ in the error message.
- Repository discovery honours `GIT_WORK_TREE` and `GIT_COMMON_DIR` and handles linked worktrees
  (`commondir`), submodule `.git` files, bare repositories and a working directory inside `.git`.
  It does not read `core.worktree`, and `GIT_DIR` does not select the repository (upstream passes
  the path to `Repo(path)`, so the variable is not consulted); `GIT_DIR` is exported to the git
  commands, see F-031.
- `os.path.exists(<int>)` (a numeric `base` in a config file is a file descriptor to Python) is
  approximated by checking `/proc/self/fd/N`.
- nbformat's schema validation messages that `nbformat.write` logs (F-013) are not reproduced.
### F-030: pretty-printer scope and quirks (U06)
- `prettyprint` ports `config`, `format_value`, `hash_string`, `file_timestamp`, `pretty_print_notebook_diff`,
  `pretty_print_merge_decisions` and the text renderers (`diff_render`, `merge_render` with their git,
  diff, diff3, difflib and built-in fallbacks). `pprint.pformat` (CPython 3.13) and `repr` are ported;
  `str.isprintable` and the regex classes `\s`/`\w` use tables generated from the oracle Python
  (Unicode 15.1) by `scripts/gen_prettyprint_tables.py`.
- Upstream quirks reproduced on purpose: the `'attachment'` typo in the cell exclude list (so
  `--ignore-attachments` does not hide attachments, which print twice), decisions at the root path
  print `//cells` (`"/".join(("/", "cells"))`), `pretty_print_output` ignores `config.metadata`, and
  an empty text prints a bare newline.
- **Type-change messages name `NotebookNode`, not `dict`**: for a dict-valued new value, upstream
  prints `type(value).__name__`. The harness hands the engine plain dicts that upstream sees as
  `DiffEntry`/`NotebookNode`; the engine prints `NotebookNode` for notebook-derived values like real
  nbdime. Not a deviation from upstream itself. The exception is the `nbdime-conflicts` value the
  merge engine writes into metadata: it is a plain `dict` upstream, so the engine prints
  `type changed from NotebookNode to dict` for it (`is_conflict_record`).
- An invalid `use_color` (not equal to `True` or `False`) prints the three header lines and then
  fails with exit 1, as the `KeyError`/`TypeError` upstream raises mid-print.
- **Markdown highlighting** is a Pygments `MarkdownLexer` plus `Terminal256Formatter` port under
  `prettyprint/highlight`. A fenced code block whose language Pygments has a lexer for returns
  `Unsupported` (the engine has no other lexers); this is a known gap. Highlighting happens wherever
  a markdown cell is printed with a blank prefix: `pretty_print_notebook` (U08) and also
  `pretty_print_value_at` for a cell value (a markdown cell with a ```` ```python ```` fence at
  `/cells/0`, `prefix=""`, oracle highlights, engine `Unsupported`). U08 adds the lexers.
- Duck-typed non-notebook shapes (attachments or metadata given as lists of integers) can behave
  differently from Python; real notebooks never have them.
- Recursion depth is bounded by what is visited: the printer counts the dict, list and diff levels it
  actually descends through (values printed item by item, `pformat` of a list, patch entries of a
  diff, metadata), and descending past `MAX_DEPTH` (500) is a `TooDeep` error (see F-009, F-018). A
  deep part of a notebook that the diff or the decisions never visit is not looked at, as upstream
  does not. Measured on the oracle (a shallow call stack): nested dicts print to about 495 levels,
  nested lists to 328 (`pformat` uses several frames per level) and chains of `patch` entries to
  328, so the engine succeeds somewhat further, as F-009 decided.
- Decisions are printed from their JSON value in the dev tool (`Printer::merge_decision_values`),
  which keeps every key upstream keeps: extra keys in sorted order, a non-string `strategy`, the
  `diff` key, and a missing `conflict` is an error (`KeyError` upstream). Engine decisions
  (`merge_decisions`) have `strategy` and `_level` as their only extra keys.
- The renderers pick their temporary directory like `tempfile.gettempdir()` (`TMPDIR`, `TEMP`,
  `TMP`, `/tmp`, `/var/tmp`, `/usr/tmp`, then the working directory), find tools like
  `shutil.which` (`access(X_OK)`, directories skipped, `PATH=""` finds nothing) and leave the
  tools' standard error to the caller, as `Popen` does.
- The dev tool takes `use_color`, `color_words`, `use_git` and `use_diff` as booleans only (error
  otherwise). Upstream raises `KeyError` for a `use_color` that is not equal to a bool (`1` and `0`
  work, it indexes a `{True, False}` dict) and reads the other three by truthiness; a deliberate
  tightening of a test interface.

### F-031: command line behaviour reproduced or decided (U07)
- **Config lookup includes the current directory**: `build_config` inserts `os.getcwd()` at the
  front of Jupyter's config path, so `./nbdime_config.json` wins over every other file. Config
  directories are expanded like traitlets' `filefind` (`expanduser` then `expandvars`), and `HOME`
  unset falls back to the passwd entry. Sections are the classes in the entry point's MRO
  (`NbdimeConfigurable`, `_Ignorables`, `_Diffing`, `Diff`, `GitDiff`, `NbDiff` for `nbdiff`;
  `...Merge`, `NbMerge` for `nbmerge`), applied base class first. `Global` is not in the MRO of any
  CLI entry point, so a `Global` section (and its `log_level`) has no effect upstream; reproduced.
  Disk values are not validated upstream (the trait validation only runs for trait defaults).
  Config shapes follow `traitlets.Config`: a top-level `""` is an empty configuration, a list of
  pairs is a configuration, a pair list element of the wrong length is ignored (`ValueError`), and
  `"version"`, `[1]`, `5`, a top-level `null` or a section that is neither a dict nor `null` are
  errors (exit 1); a section that is `null` (`{"Diff": null}`) is pruned. A value of the wrong
  type that upstream would misuse (`use_color`, `paths`, `base`) is an error here as well, except
  that `use_color` fails mid-print as upstream does (F-030).
- **`nbdime diff` and `nbdime merge` read no config**: their parser's `prog` is `nbdime`, which
  is not an entry point name, and `ConfigBackedParser` swallows the `ValueError`. Reproduced.
- **`pushd` does not restore the directory** (`utils.pushd` saves `os.curdir`, i.e. `"."`): after
  the first working-tree notebook of a revision diff the process stays in the repository root,
  relative `--out` paths resolve there, and from a subdirectory the second working-tree notebook
  is looked up in the parent of the root (and reads as a missing file). Reproduced exactly,
  including the `git check-attr` runs and their stderr. Candidate for an upstream report (ask the
  owner first; nothing has been reported).
- **A `None` base revision means the working tree** (`nbdiff a b c` with `a` not a revision gives
  `base = remote = None`; `_get_diff_entry_stream` tests `ref_name is GitRefWorkingTree`, which
  `None` satisfies): both sides are read from disk. Reproduced.
- **GitPython is replaced by `git` subprocesses**, and its revision parser is ported. `repo.commit(rev)`
  is `revparse::rev_parse`, GitPython's own grammar (`git.repo.fun.rev_parse` of the oracle's
  GitPython 3.2.1), with the object lookups done by `git rev-parse --verify --quiet` (cached),
  `cat-file -t`, `symbolic-ref` and the reflog files. Outcomes follow GitPython's exceptions:
  - `BadName`, so the argument is a path: an unknown name, a `~n`/`^n` beyond the history, a
    `:path` not in the index (`:nope.ipynb`), `:/text` with no match, `@{upstream}` without one (and
    on anything that is not a `refs/heads/` reference, `HEAD@{u}` included).
  - Any other exception, exit 1 with a one-line message on stderr (upstream: a traceback):
    `^{type}` with an unknown type or a type the object cannot be made into (`^{blob}` of a
    commit, `^{tag}` of a lightweight tag, `^{tree}~1`), reflog `@{n}` out of range or without a
    reflog, `@{-n}` (out of range, or with an explicit `ref@{-n}`), `@{<anything but an integer
    or git-time date>}` (`HEAD@{yesterday}`), `:/` with an empty pattern or `:/!x`, `rev:path`
    where the path is not in the tree, `ref@{n}` where `ref` is not a reference (`deadbee@{1}`),
    ranges and `^!`, a missing closing brace, and every revision that is not a commit
    (`HEAD^{tree}`, `HEAD:a.ipynb`, `:a.ipynb`, a tag of a tree). `HEAD:path` is recognised as a
    revision form and then exits 1, as upstream does.
  - Not reproduced: reflog dates other than git's `<timestamp> <offset>` form (RFC 2822 and ISO
    8601 dates exit 1 here; upstream resolves them), and GitPython's own reference-file lookup
    (a name is looked up by git's rules, which agree with GitPython's list of prefixes).
  Diffs use the same `diff`/`diff-tree --raw -z` commands GitPython runs, `cat-file blob`, and for
  clean filters `check-attr -z filter` and `config --get -z`, with the filter command run by
  `/bin/sh -c`. Repository discovery follows GitPython's `is_git_dir`: linked worktrees
  (`commondir`), a working directory inside `.git`, `.git` files for submodules; bare
  repositories exit 1 with a message on stderr. An empty pathspec is passed to git. A repository
  without commits makes `is_gitref(None)` raise upstream; here too.
- **`GIT_DIR` in the environment**: like GitPython, the git commands that read the repository
  run with `GIT_DIR` set to the git directory that was found (absolute), `GIT_COMMON_DIR` too when
  the environment sets it, and `GIT_OBJECT_DIRECTORY` made absolute. `GIT_DIR=.git` therefore keeps
  meaning the repository that was found, whatever the working directory of the child.
- **Log lines**: `[W module:line]` names the upstream module and call line for the merge
  diagnostics (`strategies:217/224/465/468/638/707`, `decisions:224`) and `nbmergeapp`.
  Not reproduced: the `DEBUG` records, the `generic:497/553/631` and `decisions:219` errors (each
  is followed by an exception upstream), the quadratic "Found duplicated decisions" check
  (`generic:702`), and Python `warnings` routed through logging.
- **Config path** (F-029): `JUPYTER_PLATFORM_DIRS` is ignored, and the user-site directory
  (`~/.local/etc/jupyter`) is not searched. The oracle does not search it either: virtual
  environments and pipx installs disable user site, and `PYTHONUSERBASE` is ignored for the same
  reason. A default Python install that enables user site would search it; not reproduced.
- **Mergetool**: the skipped-files message uses Python's `repr` of the list (`\udcXX` for
  undecodable bytes), and a `git` killed by a signal exits with `(-signal) & 0xFF` (SIGTERM is
  241), as `sys.exit(-15)` does.
- **`nbmerge --out`** serializes the merged notebook before opening the file, so a notebook that
  cannot be written leaves an existing file alone.
- **`argparse` edge**: `IgnorableAction` builds `ValueError(...)` without raising it for options
  that are neither `-x` nor `--xyz`; unreachable here. The mixed positive/negative ignorables check
  (`process_exclusive_ignorables`) raises `ArgumentError(None, ...)` *after* parsing, so it is a
  traceback with exit status 1, not a usage error with status 2; reproduced (exit 1, message on
  stderr). With `--config` the same conflict is raised by the parser and is a usage error (exit 2).

### F-032: nbshow highlighting and reading (U08)
- **Lexers.** nbshow always highlights: Markdown cells with `MarkdownLexer`, code cells with the lexer
  named by `language_info.pygments_lexer`, else `language_info.name`, through
  `find_lexer_class_by_name` and `Terminal256Formatter` (default style). Ported exactly, with
  Pygments' regex-lexer machinery (states, `bygroups`, `using(this)`, `include`, `default`,
  `words` via `regex_opt`, `combined`) and a backtracking engine with Python `re` semantics
  (`prettyprint/highlight/pyre`): `PythonLexer` (aliases `python py sage python3 py3 bazel
  starlark pyi`; note the spec's "sanic" is `sage` in Pygments 2.21) and `MarkdownLexer` (`markdown
  md`), including Python code in fenced Markdown blocks. Verified token-for-token (byte-identical
  terminal output) against Pygments on 3,000 stdlib and site-packages Python files and 1,497
  Markdown files.
- **Decision**: IPython lexers (`ipython`, `ipython3`, ...) are plugin lexers, registered only when
  IPython is installed. A plain `pip install nbdime` and the oracle venv have none, so such
  notebooks print unhighlighted source, as upstream does there (and as we do).
- **Deviation**: every other name in Pygments' built-in lexer table (all 927 aliases of
  `LEXER_ALIASES` except the Python and Markdown ones above, for example `python2`, `pycon`,
  `cython`, `bash`, `json`, `rust`, `julia`, `r`) prints its source unhighlighted; upstream would
  highlight. In a fenced Markdown block with such a language the code is left unstyled (upstream:
  that lexer's tokens). Names Pygments does not know behave as upstream (source unchanged; fenced
  code is a string token). Previously nbdiff failed on fenced code of any known language; it now
  degrades the same way.
- **Unknown notebook keys** print as `unknown keys: {'a', 'b'}` with the names sorted; upstream
  prints a Python `set`, whose order varies between runs.
- **nbshow parsing**: `process_exclusive_ignorables` is called with five names (not `id`), so
  `-i/-I` never conflicts with the other flags, and `id` stays unset (falsy), which only matters
  to diff filtering.
- **v3 notebooks (amends F-012)**: the parity suite feeds nbshow a v3 notebook with no worksheets,
  which upstream upgrades to 4.5 with `orig_nbformat: 3` in the metadata. `nbformat::reads` now does
  that upgrade for v3 notebooks whose `worksheets` is an empty list (no cell conversion, no random
  ids involved); every other v3 notebook is still rejected, as the U02 suite requires. The two
  suites disagree otherwise: test_u02_nbformat expects any v3 to be rejected.

### F-033: git integration and nbpatch decisions (U14)

- **System-scope fallback bug (upstream)**: `locate_gitattributes("system")` falls back to
  `os.path.join(['etc', 'gitattributes'])` when `git config --system -e` fails, which passes a
  list and raises `TypeError`, so the fallback path never works upstream. Reproduction: with no
  system git config file and `GIT_EDITOR` unusable, `git-nbdiffdriver config --enable --system`
  exits 1 with a traceback. Reproduced (exit 1, the `TypeError` message on stderr) rather than
  "fixed" to `/etc/gitattributes`, so the exit status matches; worth reporting upstream, which
  needs the owner's approval (hard rule), so nothing has been filed.
- **Exact `argparse` help and usage**: the git tools print `-h` and usage-error text through a port
  of `argparse.HelpFormatter` and `textwrap` (`COLUMNS` honoured, default 80 minus 2), because the
  suites compare stderr and stdout exactly. This is more than F-029 required; the other CLIs use
  it too now.
- **Sub-parsers**: `nargs=PARSER` swallows everything after the sub-command, so `merge
  --log-level ERROR` is "unrecognized arguments" in upstream's merge driver; reproduced. A parser
  exit inside `nbdime config-git` (help, usage error) ends the whole `or` chain with that status,
  as `SystemExit` does; a command that merely returns non-zero stops the chain too.
- **`nbpatch` without `--output`**: upstream validates the patched notebook against the schema
  and, when it fails, prints "Patch result is not a valid notebook, printing as JSON:" and the
  JSON. nbdime-rs has no schema validation (F-013), so it always pretty-prints. Deliberate
  deviation, only for patches that produce a schema-invalid notebook.
- **Highlighting in `nbpatch` output**: Python source highlighting comes from the U08 lexer. Until
  it merges, the seven `nbpatch` cases that print a notebook, and the empty-patch case, differ from
  upstream in colour codes only; with ANSI sequences stripped they are identical except one
  case with U+2028 in a code cell, which sits on the same highlighting path (re-check after U08).
- **`parity/upstream_shim.py`**: the shim makes every `main` return the exit status, but
  upstream's `test_{diffdriver,difftool,mergedriver,mergetool}_config` call `main(['config',
  '-h'])` inside `assert_clean_exit`, which needs `SystemExit`. They fail against any binary with
  "DID NOT RAISE SystemExit". With a scratch copy of the shim that raises `SystemExit(code)` when
  `-h` is in the arguments, every upstream test passes except `test_nbshow_app` (U08). The suite
  is protected, so this is for the orchestrator.

### F-051: PyPI wheels and Windows/macOS builds

- **Packaging**: the `nbdime-rs` wheel holds the `nbdime_rs` module and native executables for
  every command, in `.data/scripts` (`scripts/build_wheel.py`). The sdist builds the module
  only, because maturin cannot build extra binaries from an sdist. Installing nbdime-rs next to
  nbdime replaces nbdime's commands of the same names. An upstream `accel` extra that depends on
  this package would therefore change nbdime's commands. That needs a decision before the
  upstream PR: an engine-only distribution, or no commands in the extra's dependency.
- **Windows**: it now builds. `~` expands as `ntpath` does (`USERPROFILE`, then
  `HOMEDRIVE`+`HOMEPATH`). `git-nbdifftool`'s `os.execvp` becomes "run git and exit with its
  status". The terminal width comes from `COLUMNS` or defaults to 80. Parity is verified on Linux
  only. On Windows, upstream's text output uses CRLF line ends and nbdime-rs writes LF. Treat
  Windows as supported but not parity-tested.

## Open questions

- **Q-001** (decided): deterministic ids, see F-003.
- **Q-002** (answered): `nbformat.read` rejects a UTF-8 BOM (`NotJSONError`, `nbdiff` exits 1
  with a traceback). Decision (approved 2026-11-19): accept and ignore a leading BOM, since editors
  on Windows sometimes add one (deliberate deviation). The JSON layer itself rejects it, like `json.loads`; the notebook reader
  (U02) decides.
- **Q-003** (decided 2026-11-19): v4 only, see F-012.

## Python extension (U15)

### F-040: Python object conversion (U15)
- `nbdime_rs` reads Python objects directly into `Value`: `dict` with `str` keys, `list`, `str`,
  `int`, `float`, `bool`, `None`, and subclasses of those (`NotebookNode`, `OrderedDict`, `IntEnum`).
  Dicts use a Python `items()` iterator, including for subclasses such as `OrderedDict`. A size
  change raises `RuntimeError`. Exact lists have a direct path and list subclasses use Python
  iteration. Strings use their stored value. Tuple and set values are also accepted in the
  `ignores` option. Unsupported document types and non-string dict keys raise `TypeError`.
- Lone surrogates in a `str` are a `ValueError` (F-005). Integers beyond `i64` go through
  `int.__repr__`, so the Python 3.11 limit of 4300 digits applies (`ValueError`), as in F-010.
- Revisiting a container on the current path raises `ValueError("Circular reference detected")`
  immediately. A container shared by multiple branches is converted at each occurrence. Nesting
  deeper than 10,000 levels raises `ValueError`. The core diff limit of 500 is an `EngineError`
  (F-009). Conversion and disposal of converted inputs use explicit stacks.
- Results are plain `dict`/`list`; `merge_notebooks` returns a `tuple` `(merged, decisions)`.

### F-041: threads and the GIL in `nbdime_rs` (U15)
- The core runs with the GIL released (`Python::detach`). Inputs up to 24 levels deep use the
  caller's existing stack when at least 512 KiB remains. Deeper inputs and callers with less
  available stack use a temporary 32 MiB stack on the same thread. The parity suite includes a
  patch call from a Python thread with a 32 KiB stack. Ctrl-C is handled when the call returns to
  the interpreter.
- Engine errors and Rust panics that unwind become `EngineError`. Fatal process failures such as
  out-of-memory aborts are outside that error boundary.
- The notebook differ configuration is built per call from `ignores`/`targets`; there is no
  module state, so calls from several threads cannot interfere.

### F-042: argument decoding is shared (U15)
`nbdime_core::options` decodes `ignores`, `targets`, `strategies`/`transients`/`fall_back` and
the merge `args` for both `nbdime-rs-dev` and `nbdime_rs`; the dev tool's behaviour and messages
are unchanged. The extension validates dict options at the Python boundary and rejects unknown
merge argument names. Decisions convert with `merging::decisions_to_value` / `decisions_from_values`.

### F-043: packaging notes (U15)
- maturin 1.16 lays the wheel out as a package: `nbdime_rs/__init__.py` (a generated
  `from .nbdime_rs import *` shim) and `nbdime_rs/nbdime_rs.abi3.so`, even with no Python sources
  in the repository; `import nbdime_rs` and `nbdime_rs.EngineError.__module__` are as specified.
  The repository itself holds no Python files.
- `cargo test -p nbdime-py` must run with `--no-default-features` and `PYO3_PYTHON` set: the
  default `extension-module` feature leaves libpython unlinked, which a test binary needs.
- The wheel declares BSD-3-Clause and contains the license text. Build the Linux wheel against
  manylinux2014 for older glibc systems.

## Phase 4 performance (P4)

### F-045: why aligning a re-indented cell was cubic, and the exact fix
- **What**: the cost was not in the line alignment (`compute_snakes_multilevel` over lines is
  quadratic and cheap: a 1,000-line re-indent aligns in well under 0.1 s) but in the *cell*
  alignment. Without cell ids (nbformat < 4.5, or ids that differ), `compare_cell_strict` /
  `compare_cell_approximate` call `compare_strings_approximate` on the two whole sources with no
  `maxlen`, and the bounds pass for a re-indent, so `SequenceMatcher(None, a, b).ratio()` runs on
  tens of thousands of characters. `find_longest_match` costs one step per pair of equal
  characters in its rectangle (about `|a| * |b| / alphabet`), and for text that matches line by
  line `get_matching_blocks` finds the lines first to last: every search returns the first line
  of its rectangle and recurses into everything after it. That is one full rescan per line,
  cubic in the number of lines. Upstream has the same behaviour (CPython's difflib), roughly 75
  times slower.
- **Fix (identical results)**: `get_matching_blocks` only searches rectangles nested in earlier
  ones, and the rows of pending rectangles are disjoint. A match ending in row `i` of a nested
  rectangle is never longer than the longest match ending in row `i` when that row was last
  scanned in full (clipping to a smaller rectangle only shortens runs). The search keeps that
  per-row maximum; a rectangle's bound is the maximum over its rows. The scan visits candidates
  in the tie-breaking order and only a strictly longer match replaces the best, so the first match
  that reaches the bound is the answer and the scan stops there; rectangles whose bound is 0 are
  not scanned at all. With autojunk the bound is on the indexed (non-popular) run lengths, which
  is what the scan computes, and the extension over popular elements runs unchanged afterwards.
  Checked against a literal port of CPython's algorithm on random and re-indented inputs (unit
  tests in `difflib.rs`) and against CPython itself on 3,000 random pairs and the re-indent cases
  up to 250 lines.
- **Effect**: one 1,000-line cell re-indented, no ids: 14.4 s to 0.16 s (generated statements),
  repeated statements 500 lines 5.0 s to 0.12 s. What remains is the first, full search of the
  whole texts, which CPython also does: quadratic in characters (0.3 s at 1,000 lines). A suffix
  automaton per large rectangle would make that search linear and also yields the per-row
  maxima; not done (see progress, next steps).
- **Upstream**: relevant to a future upstream performance note (the same bound applies to
  CPython's difflib); nothing to report as a bug.

### F-046: other shortcuts that keep results identical
- `_compare_mimedata` on two equal non-text strings returns True without testing `_is_base64`:
  both the base64 branch (`x == y`) and the text branch accept equal strings. This removed 47% of
  the instructions on identical image-heavy notebooks.
- `quick_ratio` is the size of the multiset intersection, so it is computed from per-character
  counts (`sorted_char_counts`) instead of merging sorted character lists.
- `compare_cell_by_ids` compares a precomputed hash of string ids first; equal hashes still
  compare the strings.
- The brute-force LCS table is kept in Allison-Dix bit-vector form (64 columns per addition). It
  equals the textbook table for any compare relation, not only equivalences, because the
  derivation uses only the recurrence; the backtrack reads `R[x][y]` from the bit rows and is
  unchanged. Checked against the textbook table on 400 random relations of up to 150x150.


### F-047: U08 terminal and regex review

- `nbshow --help` and `nbdime show --help` now use the same ignorables group as upstream.
  Both forms have byte-for-byte CLI parity cases.
- The regex engine now matches CPython 3.13 on `(?i)[a-z]` with dotted capital I, literal
  nonbreaking spaces in verbose patterns, and `\B` on empty input. Unit tests cover the
  cases directly because the bundled lexers do not exercise those patterns.
- Explicit Python stream encodings remain a CLI deviation. For `PYTHONIOENCODING=latin-1`,
  piping ASCII JSON with a Markdown source of `"\u00e9\n"` to `nbshow -s -` yields byte
  `e9` from upstream and UTF-8 bytes `c3 a9` from Rust. A literal Latin-1 byte in that JSON
  is accepted by upstream and rejected by the Rust UTF-8 stdin reader.

### F-048: U14 clean-filter and merge diagnostics review

- The clean filter uses UTF-8 replacement decoding for `git check-attr`, inherits Git config
  stderr and refuses a remote directory before launching the filter. Regression cases cover
  an undecodable path, malformed `GIT_CONFIG` and a filter that emits a notebook without reading
  stdin.
- Merge output reports duplicate-cell-ID repairs at warning level. Rust uses the stable
  `nbformat/validator.py` suffix for the warning location. The Git harness normalizes the
  absolute installation prefix and Git-generated `.merge_file_XXXXXX` name. It still compares
  the complete warning, other stderr, exit status and notebook contents under the existing
  cell-ID normalization.

### F-049: P4 public helper compatibility

`quick_ratio_sorted` is restored with its original signature and multiplicity semantics.
Notebook matching still uses the counted fast path.

### F-050: local optional engine integration

The adapter loads the wheel lazily, verifies its compatibility, tracks supported ignore and
target settings, and reconstructs the Python diff and merge-decision types. It uses Python
for custom configurations, mixed container trees, output key-order side effects and custom
merge payloads with ambiguous object types. The prototype was tested on nbdime v4.0.4.
Remaining Rust deviations, including F-019 and F-027, need an upstream acceptance decision
before enabling the engine for upstream users. The change is prepared locally.
