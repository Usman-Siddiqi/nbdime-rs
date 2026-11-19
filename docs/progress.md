# Progress log

## Session 1 (2026-11-19, 21:49 to 23:00 UTC)

### Done
- **Phase 0.** Read upstream nbdime 4.0.4. Its own test suite runs green on the reference venv
  (6,351 passed, 7 skipped, 4 xfailed, 38 s; web/server tests excluded). Reproduced and profiled
  the scaling problem (`docs/upstream-performance.md`, raw data in
  `bench/results/2026-11-19-upstream-baseline.json`). Wrote `CLAUDE.md`, porting guide, design,
  findings, roadmap, and an inventory of upstream tests (`docs/inventory/upstream-tests.md`).
- **Phase 1 foundation (orchestrator).** Workspace, BSD-3 license, `Value` with Python equality,
  a JSON parser/serializer byte-identical to Python's `json` (verified on 100k random floats, 5k
  documents, the upstream notebooks and rejection cases), `str.splitlines`, diff format types and
  builders, the `nbdime-rs-dev` batch tool, the parity harness, the merge gate
  (`scripts/gate.sh`), CI, and the benchmark runner.
- **Phase 1 pilot (P1, generic diff engine)** through the full loop and merged: difflib
  `SequenceMatcher`, character and line diffs, approximate comparison, brute-force LCS, multilevel
  snakes, generic `diff`. Parity: identical to nbdime 4.0.4 on all pilot suites (~45k cases per
  run, also re-run at 4x scale with another seed, ~180k cases).

### Pilot loop record
| Stage | Model | Tokens | Tool calls | Wall time | Outcome |
|---|---|---:|---:|---:|---|
| Implementer | Sonnet | 190,778 | 44 | 9.4 min | 2,300 lines; gate failed on 2 **harness** flaws it diagnosed correctly; found upstream quirk F-007 |
| Reviewer A (correctness) | Opus | 171,900 | 47 | 11.1 min | sign-off no: NaN-in-containers equality (major), 5 minor, 3 nit |
| Reviewer B (Python vs Rust) | Opus | 179,721 | 49 | 10.0 min | sign-off no: same major found independently, plus lone surrogates, depth, digit limit |
| Fixer | Sonnet | 218,805 | 64 | 10.9 min | 9 of 9 fix-list items fixed, regression tests added |
| Delta re-review | Opus | 144,004 | 31 | 6.1 min | one "major" in custom-only configs, downgraded by orchestrator (F-011) |
| **Loop total** | | **905,208** | 235 | **~48 min** elapsed (reviews in parallel) | |
| Inventory (mechanical) | Haiku | 116,603 | 14 | 2.3 min | test inventory |

What the loop caught that the gate did not:
- **NaN equality** (both reviewers): CPython's json shares one NaN object and container equality
  short-circuits on identity, so `[NaN] == [NaN]` is True. My porting guide said the opposite and
  the harness fed the oracle Python-built objects, which hid it. Fixed in the guide, the core, and
  the harness (the oracle now receives json-decoded arguments).
- Lone surrogates silently hiding changes (F-005 decision reversed: reject instead of replace).
- Stack overflow (process abort) on deep nesting; Python's 4300-digit integer limit.
- Performance and hygiene: per-pair `Vec<char>` rebuilding, needless deep clones, an unseeded
  hasher on untrusted input.
What the implementer caught: two flaws in my own parity tests (F-001 cases that did not trigger
the bug; a generator producing too many error-only cases).

### Workflow changes made
- The oracle now gets arguments decoded from the same JSON text the engine receives (F-008).
- `check_parity` refuses suites where more than 5% of cases are agreed errors, and never counts
  "unknown function" as agreement.
- Added a delta re-review stage after the fixer (one Opus reviewer on the fix commits only).
- The orchestrator writes the unit's parity suite before the implementer starts; implementers may
  report test flaws with evidence but never edit tests.

### Known issues / open decisions
- Q-001 deterministic cell ids (F-003), BOM acceptance (Q-002), v3 notebooks (Q-003).
- CI status on GitHub not verified from here (the GitHub connector failed to connect).
- `py_float_repr` formats every float twice (`{:.800e}` tie check); fine now, optimise in Phase 4.

### Next steps
- Phase 2 per the fan-out plan in the session summary (waves of up to 3 units in parallel).
