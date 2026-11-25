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

## Session 2 (2026-11-19 23:00 to 2026-11-20, Phase 2 waves 1 and 2)

### Done
- Owner decisions recorded (2026-11-19 23:01): deterministic ids (F-003), accept a leading BOM
  (Q-002), proceed through Phases 2 to 4 without checking in, work on `main` and push often.
- Wave 1 merged: U05 patch + diff utils, U02 nbformat read/write (each implementer, hostile
  review, fixer, gate). U04 notebook diff implemented and reviewed twice (A and B); fixer running.
- Upstream bugs found and fixed in the oracle (with neutrality tests): F-014 `diff_single_outputs`
  mutates and reorders its input, F-019 JSON-scalar mime values crash the differ. Others recorded:
  F-013 addendum (schema `TypeError`), F-015 extended (patch key/length types), F-018 deep nesting.
- Wave 2 prepared: parity suites for the merge engine (U10: recorded corpus of 429 merge calls made
  by upstream's own tests, plus generic and notebook random triples) and for git-compatible text
  rendering (U16), both validated against mirror engines that run the oracle behind the batch
  protocol; specs `docs/process/u10-merge.md`, `u16-xdiff.md`; findings F-021 (marker cell ids),
  F-022 (git 2.43 default config is the target), F-023 (no translation of LGPL xdiff).
- Roadmap: merge units U10 to U12 folded into one U10 (they share one suite); new U16.

### Agent cost record (tokens / wall time)
| Stage | U05 | U02 | U04 |
|---|---|---|---|
| implementer (Sonnet) | 137k / 6 min | 194k / 9.5 min | 274k / 19 min |
| review A (Opus) | 134k (combined) | 160k / 11 min | 209k / 35 min |
| review B (Opus) | | | 218k / 26 min |
| fixer (Sonnet) | 107k | 121k / 9 min | running |
A five-hour rate limit stopped all agents from 23:18 to 01:50 UTC; they were resumed with their
context intact.

### Workflow changes made
- Mirror engines: before an implementer starts, the suite is run against a Python script that
  speaks the batch protocol and answers with the oracle itself. Catches suite bugs (nondeterminism,
  oracle crashes in generation) before they cost an implementer iteration.
- Oracle-side fixes for upstream bugs live in one `upstream()` context manager, each with a
  neutrality test proving it changes nothing where upstream does not fail.

### Next steps
- Verify and merge U04 (fixer report, timings, gate at seed 6 scale 2); enable its suite.
- U16 implementer running. Start U10 once U04 is on `main` (it needs the notebook differ's
  configuration); U10 uses a temporary `git merge-file` subprocess until U16 lands.
- Wave 3: U06 pretty-printer (needs U16), U07 to U09 CLIs. Wave 4: U13, U14, U15.

### Performance notes for Phase 4 (from the U04 delta review, 2026-11-20)
- End to end (parse + diff + dump) the engine beats upstream everywhere measured except diffs that
  are trivial but carry large added values (new image-heavy notebook vs empty: engine library
  1.45 s vs upstream 1.31 s). Causes: `json::dump::write_string` pushes one char at a time (~37
  instructions/byte, ~1.5x slower than CPython's C encoder), added values are copied into the diff
  and again by `diff_to_value`, and the dev tool deep-copies its `args`.
- `is_base64` (predicates.rs) walks base64 strings char by char: 44% of instructions on identical
  image notebooks.
- Line alignment grows faster than quadratic: dedenting a 1,000-line cell takes 150 s (upstream
  is ~75x slower still). Needs an algorithmic look that keeps parity.

## Phase 4 performance unit P4 (2026-11-25)

### Done
All output unchanged (gate with every enabled suite, plus U04 and U10 at `PARITY_SCALE=2
PARITY_SEED=3`). "Before" is `0043ca0` built from a clean export; instruction counts are
callgrind totals for one `nbdiff` run, wall times best of 3 (`bench/micro.py`, `bench/run.py`;
the machine was shared with other builds, so prefer the instruction counts).

| Case | Before | After |
|---|---|---|
| one 1,000-line cell re-indented, no ids, generated statements | 2.21 s | 0.21 s |
| the same, one statement repeated, 250 / 500 / 1,000 lines | 0.64 / 4.14 / 35.4 s | 0.015 / 0.048 / 0.19 s |
| empty vs 100 cells with a 200 kB PNG each, `--out` | 1,038 M instr, 0.14 s | 241 M instr, 0.10 s |
| that notebook vs an identical copy | 1,108 M instr, 0.20 s | 286 M instr, 0.10 s |
| `bench/run.py` 1,600 cells, ids | 551 M instr, 0.196 s | 288 M instr, 0.171 s |
| `bench/run.py` 1,600 cells, no ids | 1,753 M instr, 0.548 s | 917 M instr, 0.292 s |
| `bench/run.py` 3,000 cells, ids / no ids | 0.447 / 1.516 s | 0.343 / 0.706 s |

Upstream (`nbdiff` 4.0.4) on the same micro cases: 1.47 s (image add), 1.91 s (identical images),
26.7 s (repeated statement, 250 lines).

- Re-indented cells (target 3): the cubic cost was `SequenceMatcher` on whole cell sources in the
  cell predicates (no cell ids), not the line alignment; fixed with an exact per-row bound in
  `get_matching_blocks`, F-045.
- JSON strings are scanned eight bytes at a time when parsing and dumping (`json/scan.rs`):
  `write_string` from ~37 to ~4.6 instructions per byte, `parse_string` from ~22 to ~3.6.
  `diff_into_value` moves a diff into its JSON form for `nbdiff --out` and the dev tool, and the
  dev tool no longer copies its arguments. Values are still copied from the notebook into the
  diff (owned `Value`s in `DiffEntry`); borrowing would need a lifetime on the diff types and the
  copy is now a few percent of the image case (memcpy), so it was left.
- `is_base64` scans bytes with a fallback to characters at the first non-ASCII byte, and is not
  called at all for equal MIME strings (F-046).
- Bench profile (target 4): bit-parallel LCS table, `quick_ratio` from character counts, cell ids
  compared by hash first, string fast path in `py_eq` (F-046).
- New `bench/micro.py` for the targeted cases; results in `bench/results/2026-11-25-*.json`.

### Next steps
- What remains of a long re-indented cell is the first, full `find_longest_match` over both
  sources, quadratic in characters (about 0.2 s at 1,000 lines, 2 s at 3,000). A suffix automaton
  of `b[blo..bhi]` per large rectangle finds the same match (longest, then earliest in `a`, then
  earliest `firstpos` in `b`) in linear time and also gives the per-row maxima the bound needs.
- No-ids bench profile: `is_similar` (strict cell predicate over all pairs) ~30%, the
  `Predicate::apply` dispatch per pair ~13%.
