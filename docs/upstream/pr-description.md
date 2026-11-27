<!--
Notes for Usman, not part of the PR. Delete this block before posting.
- Only open this if the maintainers said yes on the issue. Put the issue number below.
- If they picked the plugin hook instead, this needs rewriting.
- Only keep the macOS/Windows line once those wheels are actually built and checked.
-->

Follow-up to #ISSUE.

This adds the optional Rust engine we talked about. It comes from the separate
[`nbdime-rs`](https://github.com/Usman-Siddiqi/nbdime-rs) package.

**How it works**

- It's off by default. nbdime only uses it with `NBDIME_ACCEL=1`. If the variable isn't set (or
  is `0`), the extension isn't even imported.
- `NBDIME_ACCEL=force` makes it raise instead of falling back, so CI can check the engine
  actually ran.
- With `NBDIME_ACCEL=1`, if the engine hits an error it logs it at debug level and uses the
  Python code. Custom differs/predicates and anything it can't represent always use Python.
- Conflict marker cells from the inline merge strategies get stable ids instead of random ones.
  Everything else in the result is the same.
- The public API doesn't change. Results come back as the normal `DiffEntry`, `MergeDecision`
  and `NotebookNode` objects.
- Building nbdime doesn't need Rust. The `accel` extra pulls in a prebuilt wheel.
- It only switches on if the engine says it matches the exact nbdime version, so a new release
  never runs an untested engine. I'll release a matching nbdime-rs for each nbdime release.

**AI note:** like I mentioned in the issue, AI coding agents wrote most of nbdime-rs and this
patch, with me directing it. I tested it a lot (below), and I'm the one responsible for it, so
ask me about anything.

**Testing**

- nbdime's non-web tests pass with `NBDIME_ACCEL` unset, `0` and `force`. The only failures I get
  locally are git-driver and server-extension tests that fail the same way without this patch.
- `test_accel.py` covers the off/missing/incompatible/broken engine cases, fallback for custom
  config, output order and result types. If the extension is installed, it also checks every
  diff and merge of nbdime's test notebooks against pure Python.
- On the nbdime-rs side, 129 parity tests compare against nbdime 4.0.4 directly (about 10,000
  random and weird notebook pairs), and also ran at 4x size with three seeds. No differences.
- Wheels install and import on clean CPython 3.9 and 3.13 on Linux, macOS and Windows.

**Speed** (median of 3 in-process calls, CPython 3.13, 4-core Xeon VM, ~5% of cells changed;
rerun with `bench/upstream_engine.py` from nbdime-rs)

| Notebook | Diff: Python | Diff: engine | Merge: Python | Merge: engine |
|---|---:|---:|---:|---:|
| 100 cells | 6.5 ms | 1.4 ms | 16 ms | 5.2 ms |
| 400 cells | 44 ms | 5.8 ms | 104 ms | 20 ms |
| 1,600 cells | 661 ms | 29 ms | 1.21 s | 107 ms |
| 400 cells, no ids | 1.82 s | 9.6 ms | 3.49 s | 28 ms |
| 1,600 cells, no ids | 30.4 s | 114 ms | 60.8 s | 241 ms |

Full `nbdiff` on the 1,600-cell notebook without ids: 32 s with the engine off, 1.2 s with it
on. It gets better as notebooks get bigger, because the Python cell matching grows faster than
the number of cells.
