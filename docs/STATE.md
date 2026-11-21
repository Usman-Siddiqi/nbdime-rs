# Current state (compact; read this first on restart)

Updated 2026-11-21 07:40 UTC. Details live in `docs/progress.md`, decisions in `docs/findings.md`.

## Merged to main
- U01–U05, U02 (JSON I/O, generic diff, patch, nbformat), U04 notebook diff, U10 merge engine,
  U06 terminal pretty-printer, U07 CLI (`nbdiff`, `nbmerge`, `nbdime`; 97 parity tests incl.
  `test_cli.py`; ~2 ms per nbdiff run vs ~1 s upstream). U16 (in-process git diff) withdrawn: rendering spawns git like
  upstream (F-022, F-023).
- Enabled parity suites: `parity/enabled_suites.txt`.

## PAUSED 2026-11-21 07:40 UTC (owner's usage limit). Resume Thursday 2026-11-26, 2 PM Toronto.
Exactly where to resume:
1. U14 (git drivers, config-git, nbpatch) and U08 (nbshow with Pygments-compatible Python
   lexer) implementers were stopped mid-work by the rate limit. Their unfinished work is
   committed locally as `wip(u14)` / `wip(u08)` on branches `unit/u14`, `unit/u08` (worktrees
   `../wt/u14`, `../wt/u08`) and saved as git bundles in the project files folder
   (`/mnt/project-files/nbdime-rs-wip/unit-u14.bundle`, `unit-u08.bundle`; both are
   `main..unit/uXX` at main 308dbf8). If the worktrees are gone: `git fetch <bundle>
   unit/uXX:unit/uXX`, then `git worktree add ../wt/uXX unit/uXX`.
2. U14's last state: 4 config tests of `parity/test_upstream_suite.py` still failed beyond
   `test_nbshow_app`. U08 had the regex-lexer machinery and Python lexer in progress.
3. Resume each implementer with its prompt (`scratchpad/wave1/impl-u14.md`, `impl-u08.md`; if the
   scratchpad is gone, the specs are `docs/process/u14-git.md` and `docs/process/u08-nbshow.md`
   and the acceptance gates are in them). Then one hostile review each (findings become parity
   cases), fix, gate, merge; enable `test_cli_git.py`, `test_cli_nbshow.py`,
   `test_upstream_suite.py` in `parity/enabled_suites.txt`.
4. Then U15 (PyO3 package), Phase 3 to 5.

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
