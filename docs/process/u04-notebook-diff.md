# Unit U04: notebook diff (predicates, differs, diff targets)

Unit spec for implementer, reviewers and fixer. The orchestrator owns it. (The roadmap's U03
"notebook predicates" is merged into this unit, because `compare_cell_moderate` runs the
configured outputs differ.)

## Scope

Port `nbdime/diffing/notebooks.py` (nbdime 4.0.4) into `crates/nbdime-core/src/diffing/`
(`predicates.rs`, `notebooks.rs`), extending `config.rs`. Read the upstream file in full, and
`generic.py`, `config.py`, `snakes.py` again where they meet it. Porting guide: sections 6.8,
6.9, 7, 9, 10, 11. `docs/upstream-performance.md` explains why upstream is slow; read it.

## Configuration as data

Extend `Predicate` with the six notebook predicates (`CellApproximate`, `CellModerate`,
`CellStrict`, `CellById`, `OutputApproximate`, `OutputStrict`) and `Differ` with what
`notebook_differs` holds: `SequenceMultilevel`, `StringLines` (`diff_string_lines`),
`SingleOutputs`, `Attachments`, `Ignore`, and `IgnoreKeys { inner: Box<Differ>, keys: ... }`
(`diff_ignore_keys` wraps whatever differ the path had, so wrapping can nest).

```rust
pub struct NotebookDiffConfig { ... }            // notebook_config, with reset defaults
impl NotebookDiffConfig {
    pub fn new() -> Self;                         // = reset_notebook_differ()
    pub fn set_ignores(&mut self, ignores: &[(String, Ignore)]) -> Result<(), DiffError>;
    pub fn set_targets(&mut self, targets: &Targets);  // set_notebook_diff_targets
}
pub fn diff_notebooks(a: &Value, b: &Value, config: &NotebookDiffConfig) -> Result<Diff, DiffError>;
```

`set_ignores` must behave exactly like `set_notebook_diff_ignores` applied to upstream's table,
in the given order: `true` sets `Ignore`; `false` removes the override (back to the default for
that path); a list wraps the current differ (default included) in `IgnoreKeys`; any other value
is an error. `set_targets` builds upstream's dict (note its key order and that `/cells/*` and
`/cells/*/outputs/*` get `False` or `("execution_count",)`) and applies it the same way. No
global state.

## Semantics to get exactly right

- Every path is the one upstream builds. In particular `diff_single_outputs` diffs the output
  **without `data` using the default generic config** (`diff(a_conj, b_conj)`), and other output
  types with `diff(a, b, config=config)`, i.e. **path `""` with the notebook config**, so the
  notebook's `/metadata`, `/cells` ... differs and predicates apply at those relative paths
  inside a stream or error output. `diff_mime_bundle` and `add_mime_diff` call the generic diff
  with the default config. `compare_outputs_approximate` runs the **configured** differ at
  `/cells/*/outputs`, so ignore flags change cell alignment.
- Predicates: `compare_text_approximate` (fast empty cutoff via truthiness, the `< 10` code point
  shortcut on both lengths), `compare_text_strict` (0.95), `make_compare_text`, `_is_base64`
  (min length 64, applied to the string with line breaks removed via `splitlines`, regex
  case-insensitive), `re_pointer` split for `text/plain` one-liners, `compare_mimebundle_*`
  (`None` handling, key **sets**), `compare_output_*` (key sets, `handled` sets, unknown-key `!=`
  with Python equality, `STREAM_MAX_COMPARE_LENGTH` 1000, `TEXT_MIMEDATA_MAX_COMPARE_LENGTH`
  10000), `compare_cell_*` (`outputs or ()` truthiness), `compare_cell_by_ids` (`'id' in x and
  'id' in y and ==`). Accessing a missing key upstream raises; so must we (error, not `false`).
- `diff_attachments` / `diff_mime_bundle`: `remove` for sorted missing keys, then sorted common
  keys, then sorted added keys; type errors for non-dicts. `add_mime_diff` lowercases the mime
  type only for the prefix test; equal strings short-circuit; non-split mimes use `!=`.
- `atomic_paths = {"/cells/*/id": True}`.
- `diff_notebooks` rejects non-dict inputs with an error.
- **F-001** applies as in the pilot: never raise the "Found predicate(s) ... pointing to dict
  entry" error.
- **F-014 (deliberate deviation, new)**: upstream's `diff_single_outputs` pops and re-adds
  `data`, which moves it to the end of the caller's output dicts; outputs that later appear as
  values in the diff (for example in an `addrange` of cells) then show `data` last. nbdime-rs
  never mutates its inputs. The oracle applies the same fix, and
  `test_u04_f014_fix_is_neutral` shows that it changes nothing but that key order.

## Performance requirements (this unit is where the speed-up is earned)

- Precompute per-cell and per-output data **once** per `diff_notebooks` call: joined source as
  code points (`PreparedStr`), lengths, character histograms, `_is_base64` results, pointer-split
  forms of `text/plain`. Predicates over a cell list then run on prepared items, not on `Value`s.
  `compute_snakes_multilevel` and `try_lcs_indices` are already generic over `T`.
- `compare_text_*` must decide from lengths and histograms whenever the bounds allow, as the
  pilot's `compare_strings_approximate` does; build a `SequenceMatcher` only when needed.
- Results of `compare_outputs_approximate` for a cell pair may be memoized; it runs a full
  multilevel diff of the two output lists.
- No recursion that grows with the number of cells, outputs, lines or characters.
- Target: a 1,000-cell notebook without ids diffs well under one second in release builds
  (upstream: about 15 s). Report the time you measured with `bench/gen_notebooks.py` input.

## Dev tool functions (fixed by the parity suite)

| fn | args | returns |
|---|---|---|
| `diff_notebooks` | `{a, b, ignores?: {path: true \| false \| [keys]}, targets?: {sources, outputs, attachments, metadata, identifier, details}}` | diff |
| `notebook.compare` | `{pred: "cell_approximate" \| "cell_moderate" \| "cell_strict" \| "cell_by_ids" \| "output_approximate" \| "output_strict", x, y}` | bool |

`ignores` is applied before `targets` (nbdiff's order: config file, then flags). JSON object
order of `ignores` is the order of application. `targets`, when present, has all six keys.

## Acceptance

`scripts/gate.sh parity/test_json_io.py parity/test_pilot_generic_diff.py parity/test_u04_notebook_diff.py`
prints `GATE PASSED`, plus two hostile reviews signed off.
