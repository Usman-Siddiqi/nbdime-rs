<!--
Notes for Usman, not part of the PR text. Delete this block before posting.
Only open this PR if the maintainers said yes on the proposal issue, and link that issue below.
If they chose the plugin hook instead, this PR needs rewriting around that.
The macOS and Windows wheel line under Testing is a target: only keep it once those wheels are
built and checked.
-->

Closes #ISSUE (proposal discussed there).

## What this does

Adds an optional, opt-in Rust engine for notebook diffs and merge decisions, provided by the
separate [`nbdime-rs`](https://github.com/Usman-Siddiqi/nbdime-rs) package.

- **Off by default.** The engine is only used with `NBDIME_ACCEL=1`. When the variable is unset
  or `0`, the extension is never imported and nbdime behaves exactly as before.
- **`NBDIME_ACCEL=force`** raises instead of falling back when the extension is missing,
  incompatible or fails, so CI can show the engine ran.
- **Safe fallback.** With `NBDIME_ACCEL=1`, an engine error is logged at debug level and the
  Python code is used. Custom differs and predicates, inputs the engine can't represent, and
  ambiguous merge payload types always use Python.
- **Same API.** Public signatures don't change, and results come back as the usual `DiffEntry`,
  `MergeDecision` and `NotebookNode` objects.
- **No Rust needed to build nbdime.** The new `accel` extra installs a prebuilt wheel.
- **Version-locked.** The engine is only used when it reports the exact nbdime version it was
  tested against, so an nbdime release never runs an untested engine. I'll publish a matching
  nbdime-rs for each nbdime release.

## AI disclosure

Most of nbdime-rs, and this patch, were written by AI coding agents under my direction. I've
tested it heavily (below) and I'm responsible for it, so please ask me about anything that
looks off.

## Testing

- nbdime's non-web test suite passes with `NBDIME_ACCEL` unset, `=0` and `=force` against the
  same wheel and checkout. The only failures in my environment are git-driver and
  server-extension tests that fail the same way without this patch.
- `test_accel.py` covers the off, missing, incompatible and failing-engine cases, custom
  configuration fallback, output order and Python result types. When the extension is
  installed, it also checks that every diff and merge of nbdime's test notebooks matches
  pure Python.
- Separately, nbdime-rs runs 129 side-by-side parity tests against nbdime 4.0.4 (about 10,000
  random and adversarial notebook pairs), also at four times that size with three seeds, with no
  differences.
- Wheels install and import in clean CPython 3.9 and 3.13 environments on Linux, macOS and
  Windows.

## Performance

Median of seven in-process `diff_notebooks` calls on CPython 3.13, on generated notebooks with
source edits in about 5% of cells:

| Code cells | Python | Engine | Speedup |
|---:|---:|---:|---:|
| 100 | 2.8 ms | 0.7 ms | 4x |
| 800 | 135 ms | 5.2 ms | 26x |
| 1600 | 584 ms | 10.4 ms | 56x |

The gap grows with size because Python's cell matching is superlinear. Inputs that fall back to
Python are unchanged.
