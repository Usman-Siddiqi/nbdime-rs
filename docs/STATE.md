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

1. Phase 3 is complete (2026-11-27). Every enabled suite ran at `PARITY_SCALE=4` with seeds 3,
   17 and 29. Seed 29 passed 129/129. Seeds 3 and 17 showed only two harness bugs: nbformat's
   install path was not masked in generated merges, and the delta fixture was invalid for some
   seeds. Both are fixed in `parity/test_cli.py`, and the affected tests pass again at those seeds.
   No engine or CLI differences were found.
2. Phase 5 is prepared locally at upstream commit `bbf08bc`, based on nbdime v4.0.4. Use
   `docs/upstream/optional-rust-engine.patch` to reproduce it in a separate local checkout.
   The patch now logs engine errors at debug level, adds `NBDIME_ACCEL=force`, loads the engine
   under a lock and tests the real extension against pure Python. The upstream non-web suite
   had 6,383 passes, 7 skips and 4 expected failures in each of `NBDIME_ACCEL=0`, the default
   and `force`. The only other failure in each mode is the JS-artifacts test. Keep the
   prototype local and ask the owner before each upstream action. The proposal and PR
   description drafts are in `docs/upstream/`.
3. Upstream plan (owner agreed 2026-11-27): the engine is now opt-in (`NBDIME_ACCEL=1`; unset
   or `0` never imports it), re-verified against v4.0.4 with the real extension. The drafts
   disclose AI authorship and lead with testing. Order: publish nbdime-rs to PyPI with Linux,
   macOS and Windows wheels, then post `proposal.md` as an issue, then the PR only if invited.
   The proposal also offers a minimal plugin hook as a smaller alternative.
   Merges with inline cell or output conflicts no longer fall back to Python (marker-cell
   payloads are converted to `NotebookNode`); gate 129/129 after the change. Benchmark:
   `bench/upstream_engine.py`, results in `bench/results/2026-11-27-upstream-engine.json`.

## PyPI release (ready, not published)
- `pip install nbdime-rs` gives the `nbdime_rs` module and native commands (wheel built by
  `scripts/build_wheel.py`). `.github/workflows/release.yml` builds wheels for Linux x86-64 and
  arm64 (manylinux2014), macOS Intel and arm64, and Windows x86-64, plus an sdist. It smoke-tests
  every wheel (`scripts/smoke_wheel.py`) and runs the unit tests on macOS and Windows. It
  publishes only for a `v*` tag, through trusted publishing. Green since 2026-11-27.
- CI parity also runs on macOS (129/129, blocking) and Windows (111/129, non-blocking); details in F-051.
- The owner's steps are in `docs/release.md`. Open: installing nbdime-rs next to nbdime replaces
  nbdime's commands, which matters for an upstream `accel` extra (F-051).

## Workflow

- One hostile review per unit. Record the findings, add regression coverage and verify fixes with
  the gate. Keep harness changes in separate commits.
- U16 remains withdrawn for licensing reasons. Text rendering delegates to Git or the existing
  fallbacks. The web UI is outside the port's scope.
