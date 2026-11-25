# Phase 4: performance work (parity-preserving)

Unit spec for implementer and fixer. The orchestrator owns it. One hostile review. Every change
must keep all enabled parity suites passing; no output may change.

## Targets (from docs/progress.md, "Performance notes for Phase 4")
1. `json` dump: `write_string` pushes one char at a time; write runs of unescaped bytes at once
   (memchr-style scan over the UTF-8 bytes for `"`, `\\`, control chars and, with
   ensure_ascii, non-ASCII). Avoid copying added values into the diff and again in
   `diff_to_value` (borrow or move).
2. `is_base64` (diffing/predicates.rs): byte-level scan instead of per-char; 44% of instructions
   on identical image-heavy notebooks.
3. Line alignment inside a cell grows faster than quadratic (dedenting a 1,000-line cell takes
   150 s). Profile, find the algorithmic cause (likely the multilevel snakes / SequenceMatcher
   usage pattern repeated per level or per gap), and fix it without changing results. If
   identical results require upstream's exact algorithm, find the redundant work (memoization of
   identical subproblems, caching `SequenceMatcher` b2j for the same b, avoiding re-diffing
   unchanged gaps) rather than changing the algorithm.
4. Anything else a profile of `bench/run.py` cases shows above 10% of the time.

## Measure
Before/after with `bench/run.py` (rs-cli, sizes 100..3000, ids and noids, repeat 3) plus targeted
micro-benchmarks for each target (an image-heavy notebook vs empty, identical image notebooks,
a 1,000-line cell re-indented). Use `perf` or `valgrind --tool=callgrind` if available. Commit the
new results JSON under bench/results/ and record the numbers in docs/progress.md.

## Acceptance
`scripts/gate.sh $(grep -v '^#' parity/enabled_suites.txt | sed 's|^|parity/|')` prints
`GATE PASSED`, plus the U04 and U10 suites at `PARITY_SCALE=2 PARITY_SEED=3`.
