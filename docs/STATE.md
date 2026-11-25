# Current state (compact; read this first on restart)

Updated 2026-11-25 03:10 UTC. Details live in `docs/progress.md`, decisions in `docs/findings.md`.

## Merged to main
- U01–U05, U02 (JSON I/O, generic diff, patch, nbformat), U04 notebook diff, U10 merge engine,
  U06 terminal pretty-printer, U07 CLI (`nbdiff`, `nbmerge`, `nbdime`; 97 parity tests incl.
  `test_cli.py`; ~2 ms per nbdiff run vs ~1 s upstream). U16 (in-process git diff) withdrawn: rendering spawns git like
  upstream (F-022, F-023).
- Enabled parity suites: `parity/enabled_suites.txt`.

## In flight (resumed 2026-11-25 03:00 UTC, all in parallel)
| Unit | Branch / worktree | Spec | Gate suite |
|---|---|---|---|
| U14 git drivers, config-git, nbpatch | `unit/u14`, `../wt/u14` | `docs/process/u14-git.md` | `test_cli_git.py` (+ `test_upstream_suite.py` report) |
| U08 nbshow + Pygments lexer machinery | `unit/u08`, `../wt/u08` | `docs/process/u08-nbshow.md` | `test_cli_nbshow.py` |
| U15 Python extension `nbdime_rs` | `unit/u15`, `../wt/u15` | `docs/process/u15-python.md` | `test_u15_python.py` |
| P4 performance (json dump, is_base64, line alignment) | `unit/p4`, `../wt/p4` | `docs/process/p4-perf.md` | all enabled suites |
After each: one hostile review (findings become parity cases), fix, gate, merge, enable suite.
`test_upstream_suite.py` is enabled once U08 and U14 are both merged.
Then: Phase 3 large differential runs (all suites, scale 4, several seeds), Phase 5 local
upstream integration (`nbdime/_accel.py` per docs/design.md) once U15 is merged, then stop and ask.

## Next
- U08 nbshow/nbpatch (Pygments lexers; IPython environment question open).
- U14 git drivers and filters (config-git, diff/merge driver).
- U15 PyO3 package `nbdime_rs` (falls back to Python on engine error, F-017).
- Phase 3 large differential runs, Phase 4 perf (json dump, is_base64, line alignment) and
  README benchmarks, Phase 5 local upstream prep, then stop and ask.

## Workflow (owner instructions 2026-11-20 and 2026-11-25)
- Fan out every unblocked unit at once, each in its own worktree; the orchestrator writes the
  parity suites first so units can be gated independently. Owner gave standing authority to
  change the workflow for quality or speed.
- One hostile review per unit (Opus), aimed at the riskiest areas: git edge cases, encodings,
  Python lookalikes. No delta re-reviews.
- Every review finding becomes parity cases; the gate verifies fixes.
- Sonnet/Haiku for mechanical work; the strongest model only for design and tricky logic.
- Port remaining units against upstream's own test suite first; review deeply only failures or
  unusual code.
- CLAUDE.md still says "two hostile reviews"; the owner's newer instruction above takes
  precedence until CLAUDE.md is updated.
