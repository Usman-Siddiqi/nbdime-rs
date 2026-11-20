# Current state (compact; read this first on restart)

Updated 2026-11-21 00:10 UTC. Details live in `docs/progress.md`, decisions in `docs/findings.md`.

## Merged to main
- U01–U05, U02 (JSON I/O, generic diff, patch, nbformat), U04 notebook diff, U10 merge engine,
  U06 terminal pretty-printer, U07 CLI (`nbdiff`, `nbmerge`, `nbdime`; 97 parity tests incl.
  `test_cli.py`; ~2 ms per nbdiff run vs ~1 s upstream). U16 (in-process git diff) withdrawn: rendering spawns git like
  upstream (F-022, F-023).
- Enabled parity suites: `parity/enabled_suites.txt`.

## In flight
- Nothing. Next unit: U08.

## Next
- U08 nbshow/nbpatch (Pygments lexers; IPython environment question open).
- U14 git drivers and filters (config-git, diff/merge driver).
- U15 PyO3 package `nbdime_rs` (falls back to Python on engine error, F-017).
- Phase 3 large differential runs, Phase 4 perf (json dump, is_base64, line alignment) and
  README benchmarks, Phase 5 local upstream prep, then stop and ask.

## Workflow (owner instruction 2026-11-20, usage at 75% of the weekly limit)
- One hostile review per unit (Opus), aimed at the riskiest areas: git edge cases, encodings,
  Python lookalikes. No delta re-reviews.
- Every review finding becomes parity cases; the gate verifies fixes.
- Sonnet/Haiku for mechanical work; the strongest model only for design and tricky logic.
- Port remaining units against upstream's own test suite first; review deeply only failures or
  unusual code.
- CLAUDE.md still says "two hostile reviews"; the owner's newer instruction above takes
  precedence until CLAUDE.md is updated.
