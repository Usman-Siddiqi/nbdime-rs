# Handoff for any coding agent (Codex, etc.)

**Read `CLAUDE.md` first. It applies in full to every agent.** Its two hard rules are absolute:
1. Take no action on any other repository (no fork, issue, PR, comment or star, and no contact
   with maintainers) without Usman's explicit approval for that specific action.
2. Add no AI attribution anywhere: no Co-Authored-By trailers, no "Generated with" lines, in
   commits, docs or code. Commit as `Usman Siddiqi`, using the repo-local git config.

Then read `docs/STATE.md` (current state and next steps), `docs/findings.md` (deviations and
decisions, F-xxx), `docs/porting-guide.md` and `docs/design.md`.

## Goal
A Rust port of nbdime 4.0.4 with byte-for-byte parity against upstream. It covers nbdiff,
nbshow, nbpatch, nbmerge, the git drivers and tools, and `nbdime config-git`. It also ships an
optional PyO3 engine `nbdime_rs`, which upstream nbdime can use later; that integration is
prepared locally only (Phase 5). The web UI is out of scope.

## Layout
- `crates/nbdime-core`: JSON with Python semantics, diff, patch, merge, prettyprint (including
  a Pygments-compatible lexer and a Python-`re` engine).
- `crates/nbdime-cli`: the binaries and a faithful argparse port.
- `crates/nbdime-py`: PyO3 bindings. This crate is not a default workspace member and lives on
  branch `unit/u15`.
- `parity/`: the harness. The oracle is upstream nbdime 4.0.4 in a venv. This path is
  **protected**: never weaken, skip or special-case a test.
- `bench/` holds the benchmarks. `scripts/gate.sh` is the merge gate (protected).

## Build and gate
```
python -m venv ~/ref/venv && ~/ref/venv/bin/pip install nbdime==4.0.4 pytest
git clone https://github.com/jupyter/nbdime ~/ref/nbdime-upstream && git -C ~/ref/nbdime-upstream checkout v4.0.4
cargo build --release
REF_PYTHON=~/ref/venv/bin/python NBDIME_REF_BIN=~/ref/venv/bin scripts/gate.sh   # runs parity/enabled_suites.txt
```
The gate runs fmt, clippy -D warnings, tests, the parity suites and the protected-path check.
`GATE_ALLOW_PROTECTED=1` is for harness edits; make those in separate commits. Look at
`.github/workflows/ci.yml` for the exact environment.

## Workflow
- Work on a branch per unit, then merge and push to `main`.
- Each unit gets one hostile review. Every finding becomes a parity case, and the gate verifies
  the fix.
- When upstream has a bug, record it in `docs/findings.md` with a decision. Do not report it
  upstream.
- Keep the repo BSD-3 clean: never translate GPL or LGPL code (for example git's xdiff, F-023).
  Rendering spawns git or diff the same way upstream does.
- Keep `docs/STATE.md` current.

## Next steps (2026-11-25)
1. **U08, U14 and P4 were merged without their hostile reviews**, because usage ran out. Do
   those reviews now. Focus on lexer and `re` parity (U08), git edge cases, paths and encodings
   (U14), and whether the P4 optimisations changed any results.
2. **U15** (`unit/u15`): fix the items in `docs/process/u15-review.md`, the circular-reference
   crash first. Add the regression tests to `parity/test_u15_python.py`. Then merge `main` in,
   gate with `parity/test_u15_python.py` added, merge, and add that suite to
   `enabled_suites.txt`.
3. Phase 3: large differential runs (`PARITY_SCALE=4`, several seeds, all suites).
4. Phase 5: in a local upstream clone, add `nbdime/_accel.py` as described in `docs/design.md`,
   in nbdime's own style. Then **stop and ask Usman** before any upstream action.
