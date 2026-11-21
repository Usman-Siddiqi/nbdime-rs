# nbdime-rs

A Rust port of [nbdime](https://github.com/jupyter/nbdime) 4.0.4, the diff and merge tool for
Jupyter notebooks. The goal is a drop-in replacement for nbdime's terminal and git workflows,
with the same output, that is far faster. A second goal is an optional accelerated engine that
upstream nbdime could use.

**Status: work in progress (November 2026).** `nbdiff` and `nbmerge` are complete and match
upstream byte for byte on the parity suite. `nbshow`, `nbpatch`, the git drivers and the Python
package are in progress. See [What works](#what-works) and [What is still to come](#what-is-still-to-come).

## Why

On real notebooks upstream nbdime is slow. It aligns cells with a brute-force grid that is
quadratic in the number of cells, and most of the cost is in Python's `difflib`. Notebooks
without cell ids (nbformat 4.4 and older, still very common) are about 50 times slower again.
On top of that, every `nbdiff` run pays about a second of interpreter start-up. The analysis is in
[docs/upstream-performance.md](docs/upstream-performance.md).

## Performance

`nbdiff --no-color base.ipynb local.ipynb`, end to end: process start, reading, diffing and
printing. The notebooks are generated realistic notebooks (`bench/gen_notebooks.py`) with about 5%
of cells changed. The machine is a 4-vCPU Intel Xeon at 2.10 GHz cloud VM with 15 GB RAM. Raw
results are in `bench/results/2026-11-21-cli.json` and `bench/results/2026-11-21-rs-cli.json`.

| cells | cell ids | nbdime 4.0.4 | nbdime-rs | speed-up |
|------:|:--------:|-------------:|----------:|---------:|
| 100   | yes | 1.50 s  | 0.008 s | ~190x |
| 400   | yes | 1.26 s  | 0.030 s | ~40x  |
| 1,600 | yes | 2.29 s  | 0.163 s | ~14x  |
| 100   | no  | 1.29 s  | 0.012 s | ~110x |
| 400   | no  | 3.79 s  | 0.052 s | ~70x  |
| 1,600 | no  | 39.4 s  | 0.430 s | ~90x  |
| 3,000 | no  | > 143 s | 1.37 s  | > 100x |

The 3,000-cell upstream figure is the time spent inside `nbdime.diff_notebooks` alone, measured
in-process (`bench/results/2026-11-19-upstream-baseline.json`). The full command is slower still.

The upstream column is a single run per size; nbdime-rs is the best of three. Start-up alone is
about 2 ms, against about 1 s for upstream. Some cases are still slower than they should be; the
list is in [Known issues](#known-issues).

## Install

nbdime-rs is not on PyPI or crates.io yet. To build from source you need Rust 1.82 or newer and
git:

```sh
git clone https://github.com/Usman-Siddiqi/nbdime-rs
cd nbdime-rs
cargo install --path crates/nbdime-cli
```

This installs `nbdiff`, `nbmerge` and `nbdime` into `~/.cargo/bin`. They read the same
configuration files as upstream (`nbdime_config.json` in the Jupyter config path and in the
current directory).

`git` must be on `PATH`, as it is for upstream. Multi-line strings in diffs are rendered with
`git diff --no-index`, and inline source conflicts with `git merge-file`, both run exactly as
upstream runs them. If git is missing, the same fallbacks are used: `diff`/`diff3`, then built-in
renderers. Your git configuration is honoured, as upstream honours it.

## Usage

The commands take the same arguments as upstream's:

```sh
# Diff two notebooks
nbdiff before.ipynb after.ipynb
nbdiff -s before.ipynb after.ipynb          # sources only (-o outputs, -a attachments, -m metadata, -i ids, -d details)
nbdiff --out diff.json before.ipynb after.ipynb

# Diff against git
nbdiff                                      # working tree vs HEAD, all changed notebooks
nbdiff HEAD~1                               # working tree vs a revision
nbdiff main feature -- path/to/nb.ipynb     # two revisions

# Three-way merge
nbmerge base.ipynb local.ipynb remote.ipynb --out merged.ipynb
nbmerge base.ipynb local.ipynb remote.ipynb --decisions      # print the merge decisions
nbmerge --merge-strategy use-local --input-strategy inline base.ipynb local.ipynb remote.ipynb

# Dispatcher
nbdime diff a.ipynb b.ipynb
nbdime merge base.ipynb local.ipynb remote.ipynb
```

Exit codes, stdout, output files and stderr messages are the same as upstream's, apart from the
deviations listed below. The git diff and merge drivers (`git-nbdiffdriver`, `git-nbmergedriver`,
`nbdime config-git`) are being ported. Until they land you can keep using upstream's drivers, or
run `nbdiff` in git mode as shown above.

## What works

| Area | Status |
|---|---|
| JSON reading and writing | Byte-identical to Python's `json`, including float formatting and escapes |
| Generic and notebook diff | Identical to nbdime 4.0.4 on the parity suites: upstream's own test notebooks, ~10k random and adversarial notebook pairs, every predicate and ignore option |
| Patch | Identical on ~20k cases |
| Three-way merge | Identical decisions and merged notebooks for every merge, input and output strategy, autoresolve and inline conflict markers |
| Terminal pretty-printer | Byte-identical output, including colours, `pprint` formatting of values, base64 snipping and timestamps |
| `nbdiff`, `nbmerge`, `nbdime` | Identical exit code, stdout, output file and stderr on the CLI suite: files, generated notebooks, special inputs, config files, git revisions, worktrees, non-UTF-8 file names, about 100 review regression cases |

**How parity is checked.** Correctness here means behaving like upstream. The harness in
`parity/` runs upstream nbdime 4.0.4 (from PyPI) and nbdime-rs on the same inputs and compares
canonical JSON or raw bytes. A unit merges only when its suites pass with nothing skipped, after
a hostile review. `parity/test_upstream_suite.py` also runs upstream's own CLI and git test files
against the nbdime-rs binaries.

## Deviations from upstream

Every deliberate difference and every upstream bug found is recorded, with a reproduction and a
decision, in [docs/findings.md](docs/findings.md). The ones a user might notice:

- **Stable cell ids (F-003, F-021).** When upstream reads a notebook without cell ids, or creates
  conflict-marker cells, it assigns random ids. nbdime-rs assigns deterministic ids, so repeated
  runs give the same output.
- **No crashes where upstream crashes on valid input.** Examples: F-001 (a diff that raises
  depending on earlier calls), F-019 (`nbdiff a a` on a scalar mime value), F-027 (`nbmerge` when
  both sides append different outputs). nbdime-rs produces the result upstream would produce
  without the bug. F-024 (`clear-all` strategy) is reproduced as an error.
- **Input limits.** A UTF-8 BOM is accepted. nbformat v1–v3 notebooks are rejected rather than
  converted (F-012). There is no JSON-schema validation output (F-013). Lone surrogates in JSON are
  rejected (F-005).
- **Command-line text (F-029, F-031).** `--version`, `--help` and usage text differ. Unexpected
  errors print a one-line message instead of a Python traceback; the exit code is the same. A few
  GitPython-specific revision forms behave like git's own (F-031).
- **Undecodable file names in a repository.** Upstream crashes on these in git mode; nbdime-rs
  prints the correct diff.

## What is still to come

- `nbshow` with Pygments-compatible syntax highlighting, and `nbpatch` (in progress).
- The git diff and merge drivers, difftool/mergetool configuration, clean-filter support, and
  `nbdime config-git` (in progress).
- The Python package `nbdime_rs` (PyO3), which upstream nbdime can use as an optional accelerated
  engine, falling back to pure Python when the engine declines an input (F-017).
- Large differential runs, further performance work, and packaging (wheels and crates).
- The web UI (`nbdiff-web`, `nbmerge-web`, the server and the Jupyter extensions) is out of
  scope.

## Known issues

- Diffs whose only change is a large added value, such as a new image-heavy notebook against an
  empty one, are about as fast as upstream rather than faster. JSON string output and base64
  detection are the hot spots.
- Line alignment inside a cell grows faster than quadratically. Re-indenting a 1,000-line cell
  takes minutes, though upstream is about 75 times slower still. A fix that keeps parity is
  planned.

## Repository layout

- `crates/nbdime-core`: data model, JSON I/O with Python semantics, diff, patch, merge,
  pretty-printer.
- `crates/nbdime-cli`: the `nbdime`, `nbdiff` and `nbmerge` binaries.
- `parity/`: the parity harness against upstream nbdime.
- `bench/`: notebook generator and benchmark runner.
- `docs/`: the porting guide, design, findings, progress log and current state
  ([docs/STATE.md](docs/STATE.md)).

## License

BSD-3-Clause, like nbdime. Copyright (c) 2026 Usman Siddiqi; portions derived from nbdime are
Copyright (c) Jupyter Development Team. See [LICENSE](LICENSE).
