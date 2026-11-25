# Current state (compact; read this first on restart)

Updated 2026-11-25 17:00 UTC. Details live in `docs/progress.md`, decisions in `docs/findings.md`.

## Merged to main
- U01–U05, U02 (JSON I/O, generic diff, patch, nbformat), U04 notebook diff, U10 merge engine,
  U06 terminal pretty-printer, U07 CLI (`nbdiff`, `nbmerge`, `nbdime`; 97 parity tests incl.
  `test_cli.py`; ~2 ms per nbdiff run vs ~1 s upstream). U16 (in-process git diff) withdrawn: rendering spawns git like
  upstream (F-022, F-023).
- Enabled parity suites: `parity/enabled_suites.txt`.

## Merged 2026-11-25 (weekly usage at 97%, so reviews were deferred)
- U08 nbshow, P4 performance, U14 git drivers/config-git/nbpatch: gate passed on main
  (106 parity tests incl. test_cli_git and test_upstream_suite). Their hostile reviews hit the
  rate limit mid-run and are owed after the usage reset; review agents' work is saved.

## Next (after the usage reset)
1. Finish the U08, U14 and P4 hostile reviews; findings become parity cases, then fix.
2. U15 (`unit/u15`): review done, SIGN-OFF no. Fix: circular refs must raise ValueError at once
   (track container ptrs on the stack), dict mutated mid-conversion gives PanicException (catch
   it, raise RuntimeError), dict/list subclasses must convert via items()/iteration like json,
   wheel LICENSE + manylinux2014, run nbdime-py clippy and unit tests in the suite; smaller
   items 7-13 and F-040/F-041/F-043 corrections. Add the regression tests to test_u15_python.py
   first. Then gate, merge, enable.
3. Phase 3 large differential runs; Phase 5 local upstream integration; then stop and ask.

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
