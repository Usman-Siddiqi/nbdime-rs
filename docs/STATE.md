# Current state

Updated 2026-11-26. The session history is in `docs/progress.md` and compatibility decisions are
in `docs/findings.md`.

## Merged and verified

- U08, U14 and P4 reviews are closed. The fixes cover `nbshow` help, Python regex cases, Git clean
  filters, duplicate-ID diagnostics during merge and the public sorted quick-ratio helper.
- U15 is merged. The full Linux gate on `d83205a` passed all 129 parity tests with no skips.
  That run included `test_u15_python.py`, which is now in `parity/enabled_suites.txt`. The
  extension's Clippy, Rust tests, Python boundary tests and nested parity suites passed.
- The x86-64 manylinux2014 `cp39-abi3` wheel was installed and tested in fresh CPython 3.9.25 and
  3.13.18 environments. It includes the BSD license.

## Next

1. Resume Phase 3. The scale-4 runs for seeds 3, 17 and 29 were interrupted before pytest printed
   a final result. Rerun every enabled suite for each seed and triage any failures before marking
   Phase 3 complete. The planned random notebook suites cover at least 130,560 pairs and triples
   over those seeds. See `docs/progress.md` for the environment and commands.
2. Phase 5 is prepared locally at upstream commit `9495782`, based on nbdime v4.0.4. Use
   `docs/upstream/optional-rust-engine.patch` to reproduce it in a separate local checkout.
   The upstream non-web suite had 6,365 passes, 7 skips and 4 expected failures in each mode.
   Keep the prototype local and ask the owner before each upstream action. The proposal and PR
   description drafts are in `docs/upstream/`.

## Workflow

- One hostile review per unit. Record the findings, add regression coverage and verify fixes with
  the gate. Keep harness changes in separate commits.
- U16 remains withdrawn for licensing reasons. Text rendering delegates to Git or the existing
  fallbacks. The web UI is outside the port's scope.
