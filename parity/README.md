# Parity harness

Compares nbdime-rs against upstream nbdime 4.0.4 (the oracle), case by case.

**Protected**: only the orchestrator edits this directory. Implementers and fixers must not
change, skip, weaken or special-case anything here.

Run (from the repository root, with the reference venv that has `nbdime==4.0.4`):

    cargo build --release -p nbdime-cli
    /home/claude/ref/venv/bin/python -m pytest parity            # everything
    /home/claude/ref/venv/bin/python -m pytest parity -k pilot   # one suite

Environment:
- `NBDIME_RS_DEV`: path to the `nbdime-rs-dev` binary (default `target/release/nbdime-rs-dev`).
- `NBDIME_UPSTREAM`: path to the upstream clone (default `/home/claude/ref/nbdime-upstream`), used
  for its test notebooks.
- `PARITY_SCALE`: multiply generated case counts (default 1).
- `PARITY_SEED`: offset for all generator seeds (default 0).

Equivalence: results are compared as canonical JSON text (`json.dumps(..., separators=(",",
":"))`), so types (`1` vs `1.0` vs `true`), list order and object key order all count. A suite
that needs a looser equivalence defines it explicitly and cites `docs/findings.md`.

Skips and xfails fail the session (see `conftest.py`); every suite asserts a minimum case count.
