# Unit U10: the merge engine

Unit spec for implementer, reviewers and fixer. The orchestrator owns it. This unit covers what
the roadmap listed as U10 to U12 (decisions, chunks, generic merge, strategies, autoresolve) plus
the engine half of U13 (`merging/notebooks.py`); the `nbmerge` CLI stays in U13.

## Scope

Port `nbdime/merging/` (`decisions.py`, `chunks.py`, `generic.py`, `strategies.py`,
`autoresolve.py`, `notebooks.py`) and the merge parts of `nbdime/utils.py` (`Strategies`, path
helpers) to `crates/nbdime-core/src/merging/`. Read the upstream files yourself; they are about
2,800 lines. Porting guide: sections 2, 6.8 to 7, 8, 10, 11. Builds on U04 (notebook diff and its
ignore/target configuration) and U05 (patch).

Public API (shape, adjust names to fit the crate):

```rust
pub struct MergeDecision { /* common_path, conflict, action, local_diff, remote_diff,
                              custom_diff, similar_insert, ... as upstream, key order kept */ }
pub fn decide_merge(base: &Value, local: &Value, remote: &Value, strategies: &Strategies)
    -> Result<Vec<MergeDecision>, MergeError>;
pub fn decide_merge_with_diff(base, local, remote, local_diff: &[DiffEntry],
    remote_diff: &[DiffEntry], strategies) -> Result<Vec<MergeDecision>, MergeError>;
pub fn apply_decisions(base: &Value, decisions: &[MergeDecision]) -> Result<Value, MergeError>;
pub fn decide_notebook_merge(base, local, remote, args: Option<&MergeArgs>, diff_config)
    -> Result<Vec<MergeDecision>, MergeError>;
pub fn merge_notebooks(...) -> Result<(Value, Vec<MergeDecision>), MergeError>;
```

## Semantics to get exactly right

- Decision dicts serialize with upstream's key order and only the keys upstream sets
  (`MergeDecision` is a dict subclass; `conflict` defaults, `common_path` as a list in JSON).
  The suite compares canonical JSON with order significant.
- `_sort_key`, `pop_patch_decision`, `push_patch_decision`, `split_string_path`,
  `build_diffs`, `resolve_action`, `apply_decisions` with nested patch building: port exactly,
  including which errors are raised (they are errors in the engine, the harness treats
  both-sides-error as agreement).
- `make_merge_chunks` / `chunks.py` boundary rules, `_merge_concurrent_inserts`,
  `_merge_lists` similar-insert handling, `_merge_strings` (line-based via the string diff),
  `_merge_dicts` key order (sorted? check: upstream iterates `sorted(set(...))` in places and
  insertion order in others).
- Strategies: `Strategies` path matching (`/`, `/*`, `/a/*/b`, transients, `fall_back`), every
  strategy name upstream accepts in `strategies.py` (`clear`, `remove`, `clear-all`, `fail`,
  `mergetool`, `take-max`, `union`, `inline-source`, `inline-outputs`, `inline-attachments`,
  `record-conflict`, `use-base`, `use-local`, `use-remote`, ...), autoresolve order.
- `decide_notebook_merge(base, local, remote, args)`: `notebook_merge_strategies(args)` builds the
  strategy table from `merge_strategy`, `input_strategy`, `output_strategy`,
  `ignore_transients`; `args=None` uses upstream's defaults. The notebook diffs are computed with
  the U04 differ under the given ignore/target configuration.
- Inline source conflicts call `merge_render(base, local, remote, None)` (upstream:
  `prettyprint.merge_render`, which runs `git merge-file`). U16 is porting git's merge in
  parallel. Until U16 lands, implement `merge_render` behind one internal function in
  `merging/text.rs` that runs `git merge-file -p local base remote` (with upstream's diff3 and builtin fallbacks) in a temp dir exactly as
  upstream's `external_merge_render` + `merge_render_with_git` do (including `\r\n` → `\n` and
  the trailing `rstrip()`); the orchestrator switches it to `crate::xdiff::merge_render` when
  both are merged. Do not override the user's git configuration (F-022, F-026).
- **F-021 (deliberate deviation)**: inline-cells conflict marker cells
  (`nbformat.v4.new_markdown_cell`) get random ids upstream; ours are deterministic, 8 lowercase
  hex chars, distinct within one notebook and from all other ids in it (same scheme as F-003
  repaired ids, hashing the marker cell's content plus its position). The suite masks them.
- Known upstream behavior to replicate as errors: `decide_merge("", "", "")` raises
  `AssertionError`; `_combine_ops` returning `None` paths. Anything else that looks like an
  upstream bug: write it in your report with a reproduction, do not fix silently.
- Logging: upstream logs warnings/info during merges; the engine does not print.

## Performance

Notebook merges must scale linearly with cells (U04 already makes the diff fast). No recursion
that scales with list length; recursion by document depth is fine. Do not deep-copy the base
per decision.

## Dev tool functions (fixed by the parity suite)

| fn | args | returns |
|---|---|---|
| `decide_merge` | `{base, local, remote, strategies?: {path: name}, transients?: [path], fall_back?: name}` | decisions |
| `decide_merge_with_diff` | same plus `local_diff`, `remote_diff` | decisions |
| `apply_decisions` | `{base, decisions}` | merged value |
| `decide_notebook_merge` | `{base, local, remote, args?, ignores?, targets?}` | decisions |
| `merge_notebooks` | same | `[merged, decisions]` |

`args` keys default as in `parity/test_u10_merge.py` (`ARG_DEFAULTS`); `ignores` and `targets`
are applied exactly as in the U04 suite before diffing.

## Acceptance

`scripts/gate.sh` with all enabled suites plus `parity/test_u04_notebook_diff.py` and
`parity/test_u10_merge.py` prints `GATE PASSED`; also run the U10 suite with `PARITY_SCALE=2` and
`PARITY_SEED=` 1 and 2, and report. Two hostile reviews.
