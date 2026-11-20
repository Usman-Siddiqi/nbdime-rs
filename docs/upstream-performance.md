# Why upstream nbdime is slow

Measured against nbdime 4.0.4 (PyPI), nbformat 5.11.1, CPython 3.13.18, on the development VM
(4 vCPU cloud container, 15 GB RAM; the brief's VM was described as 2 cores / 8 GB, so absolute
numbers will differ). Notebooks come from `bench/gen_notebooks.py`: realistic code/markdown cells,
0 to 2 outputs per code cell, and about 5% of cells edited, replaced, inserted or deleted between
base and local. `ids` notebooks are nbformat 4.5 with cell ids. `noids` notebooks are nbformat
4.4 without ids, which is what most notebooks written before 2021, and many tools today, produce.

## Measurements

Time inside `nbdime.diff_notebooks(a, b)` only (excludes reading, printing and interpreter start):

| cells | with ids | without ids |
|------:|---------:|------------:|
| 100   | 0.009 s  | 0.199 s     |
| 200   | 0.023 s  | 0.597 s     |
| 400   | 0.076 s  | 2.43 s      |
| 800   | 0.202 s  | 9.64 s      |
| 1,600 | 0.747 s  | 43.2 s      |
| 3,000 | 3.12 s   | 142.8 s     |

Both columns grow roughly 4x per doubling, i.e. O(N²) in the number of cells. Without ids the
constant factor is about 50x larger.

The `nbdiff` command adds about 1.1 s of fixed cost (Python start-up and imports: `python -c
"import nbdime.nbdiffapp"` alone takes 1.15 s) and then spawns one `git diff --no-index`
subprocess, with two temp files, for every modified source or text output it prints. For 400
cells without ids the full `nbdiff` run takes 3.7 s, of which 2.4 s is diffing.

The brief's numbers (3.3 s at 200 cells, 45 s at 1,000, 6.5 min at 3,000) are of the same shape;
their notebooks presumably had more text per cell, which raises the constant (see below).

## Where the time goes

`cProfile` of the 400-cell no-ids case (18.7 s under the profiler, 59 M function calls):

| tottime | function |
|--------:|---|
| 8.6 s  | `difflib.SequenceMatcher.__chain_b` (99,771 calls) |
| 3.4 s  | `dict.setdefault` (24.5 M calls, inside `__chain_b`) |
| 3.0 s  | `list.append` (24.5 M calls, inside `__chain_b`) |
| 1.5 s  | `SequenceMatcher.quick_ratio` |
| 0.35 s | `compare_text_strict` (100,504 calls) |

### 1. Brute-force O(N·M) predicate grid for cell alignment

Cells are aligned with a multilevel LCS (`diffing/snakes.py: compute_snakes_multilevel`) over
four predicates, from strict to loose: `compare_cell_by_ids`, `compare_cell_strict`,
`compare_cell_moderate`, `compare_cell_approximate`. Each level calls
`seq_bruteforce.bruteforce_compare_grid`, which evaluates the predicate for **every** pair
`(A[i], B[j])` in the rectangle, then builds a full `(N+1)·(M+1)` LCS length table in Python.

- With ids, level 0 (`compare_cell_by_ids`) matches almost everything, so the grid is cheap per
  cell (a dict lookup and string compare) but still N·M Python calls: 638,389 calls for 800
  cells. Recursion into the few unmatched gaps is small.
- Without ids, `compare_cell_by_ids` matches nothing, so the next level, `compare_cell_strict`,
  is evaluated on the full N·M grid. That predicate is expensive (below).

### 2. Each strict comparison builds a full difflib index it then throws away

`compare_cell_strict` → `compare_text_strict` → `compare_strings_approximate`, which constructs
`difflib.SequenceMatcher(None, x, y, autojunk=False)` on the two **source strings, character by
character**. The constructor eagerly builds `b2j`, a dict mapping every character of `y` to the
list of its positions: O(len(y)) Python operations, with a `setdefault` and an `append` per
character. Only after that does it call `real_quick_ratio()` (which needs nothing but the two
lengths) and `quick_ratio()` (a multiset intersection).

For almost all pairs of unrelated cells, `real_quick_ratio` or `quick_ratio` rejects the pair,
so the expensive index is pure waste: 99,771 constructions, about 80% of total run time.
`lru_cache` on `compare_text_approximate` does not help the strict level, and the cache on
`_compare_mimedata_strings` never hits because a new closure is passed on every call.

### 3. Everything is O(N·M) memory too

The compare grid `G` and LCS table `R` are Python lists of lists: about 9 M entries each at
3,000 cells, so hundreds of MB of Python objects before any diff is produced.

### 4. Secondary costs

- `SequenceDiffBuilder.append` inserts each entry at its sorted position with a linear scan
  (O(k²) in the number of entries; small in practice).
- `compare_outputs_approximate` (moderate level) computes a full recursive diff of two output
  lists just to check that nothing was added or removed.
- `diff_single_outputs` deep-copies every output dict to diff it without its `data`.
- `nbdiff` pretty-printing forks `git diff` per modified string.

## What a faster engine must and must not do

Output parity requires the **same** LCS as the brute-force backtrack, including its
tie-breaking, because the predicates are not equivalence relations (approximate similarity is not
transitive) and the LCS is generally not unique. So the speed-up has to come from:

1. **Making each predicate evaluation cheap while returning exactly the same boolean**:
   precompute per-cell data once (joined source as code points, its length, a character
   histogram, a hash); evaluate `real_quick_ratio` from lengths alone; evaluate `quick_ratio` from
   histograms without building `b2j`; build the full matcher only for pairs that pass both
   bounds; memoize per (i, j).
2. **A compact DP**: the LCS table in a flat `Vec<u32>` (or two rows plus a bit matrix of `G` for
   backtracking), no per-cell allocation.
3. **Safe structural shortcuts only**: stripping a common *suffix* of pairs where the predicate
   holds is equivalent to the brute-force backtrack (it starts at `(N, M)` and takes the diagonal
   whenever `G` is true). Stripping a common *prefix* is not: with `A=[p]`, `B=[p, p]` the
   backtrack matches `A[0]` with `B[1]`. Any shortcut beyond suffix stripping needs a proof or a
   differential test campaign, and a note in `docs/findings.md`.
4. **No process spawning, no interpreter start-up** in the CLI.
5. **Deciding output alignment from the snakes** (U04): `compare_outputs_approximate` only needs
   to know whether the output alignment adds or removes anything, so with the default outputs
   differ (possibly wrapped in key filters) the engine reads the keys of the unaligned runs off
   the snakes and never diffs the aligned pairs or copies the added outputs.
6. **One ratio per string pair** (U04): the difflib ratio of a pair is remembered on the first
   string, so the strict, approximate and moderate levels share one `SequenceMatcher` run.

Further algorithmic options (e.g. band-limited DP when an id- or hash-anchored alignment exists)
are Phase 4 work and must be validated against the oracle on large differential runs.
