# nbdime-rs

A Rust port of [nbdime](https://github.com/jupyter/nbdime) 4.0.4, the diff and merge tool for
Jupyter notebooks. The goal is a drop-in replacement for nbdime's terminal and git workflows,
with the same output, that is far faster. A second goal is an optional accelerated engine that
upstream nbdime could use.

**Status (November 2026).** The CLI, the git integration and the optional Python engine pass the
129-test parity gate against nbdime 4.0.4. Known differences are listed under
[Deviations](#deviations-from-upstream). PyPI publishing is pending; see [Install](#install).

## Why use this?

If `nbdiff` or `git diff` on notebooks feels slow, this is for you. nbdime-rs gives the same
output as nbdime 4.0.4, but much faster, and the bigger the notebook, the bigger the gap:

- **100 cells:** 1.50 s with nbdime, 0.008 s with nbdime-rs
- **1,600 cells:** 2.29 s vs 0.163 s
- **1,600 cells without cell ids** (older notebooks): 39.4 s vs 0.430 s
- **3,000 cells without cell ids:** over 143 s vs 1.37 s

That is the full `nbdiff` command, start to finish, with about 5% of cells changed. The
[benchmarks](#benchmarks) say exactly what was measured.

Upstream aligns cells with a brute-force grid that is quadratic in the number of cells, and most
of the cost is in Python's `difflib`. Notebooks without cell ids (nbformat 4.4 and older, still
very common) are much slower again, and every `nbdiff` run pays about a second of interpreter
start-up. The analysis is in [docs/upstream-performance.md](docs/upstream-performance.md).

nbdime-rs is a drop-in replacement for nbdime's terminal and git tools: `nbdiff`, `nbmerge`,
`nbshow`, `nbpatch`, `nbdime`, and the git diff and merge drivers. The web viewer is not
included.

**About how it was built:** most of the code was written with AI coding agents, under my
direction. That is why it is tested so heavily. Every change runs against real nbdime on about
10,000 random and adversarial notebook pairs and has to match exactly. Known differences are
listed [below](#deviations-from-upstream).

## Install

From PyPI (wheels for Linux x86-64 and arm64, macOS Intel and Apple silicon, and Windows
x86-64, for CPython 3.9 and later):

```sh
pip install nbdime-rs
```

This installs the native commands (`nbdiff`, `nbmerge`, `nbshow`, `nbpatch`, `nbdime` and the
git drivers and tools) and the `nbdime_rs` Python module. Then enable the git integration:

```sh
nbdime config-git --enable --global
```

**PyPI publishing is pending.** Until the first release, install from source. You need Rust 1.82
or newer and git:

```sh
git clone https://github.com/Usman-Siddiqi/nbdime-rs
cd nbdime-rs
cargo install --path crates/nbdime-cli
```

or, without cloning:

```sh
cargo install --git https://github.com/Usman-Siddiqi/nbdime-rs nbdime-cli
```

This installs the commands into `~/.cargo/bin`. They read the same configuration files as
upstream (`nbdime_config.json` in the Jupyter config path and in the current directory). The
`nbdime_rs` module ships only in the wheel.

**Installing next to nbdime.** nbdime-rs installs commands with the same names as nbdime's. If
both are installed in one environment, nbdime-rs replaces nbdime's `nbdiff`, `nbmerge`, `nbshow`,
`nbpatch` and `nbdime` (see F-051 in [docs/findings.md](docs/findings.md)).

`git` must be on `PATH`, as it is for upstream. Multi-line strings in diffs are rendered with
`git diff --no-index`, and inline source conflicts with `git merge-file`, both run exactly as
upstream runs them. If git is missing, the same fallbacks are used: `diff`/`diff3`, then built-in
renderers. Your git configuration is honoured, as upstream honours it.

## Benchmarks

**CLI, end to end.** `nbdiff --no-color base.ipynb local.ipynb`: process start, reading, diffing
and printing. The notebooks are generated realistic notebooks (`bench/gen_notebooks.py`) with
about 5% of cells changed. The machine is a 4-vCPU Intel Xeon at 2.10 GHz cloud VM with 15 GB
RAM, Linux, CPython 3.13. Raw results are in `bench/results/2026-11-21-cli.json` and
`bench/results/2026-11-21-rs-cli.json`.

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
about 2 ms, against about 1 s for upstream. Some cases are still slower than they should be; see
[Known issues](#known-issues).

**Engine inside nbdime.** The `nbdime_rs` module can also serve as an optional engine for
upstream nbdime (a local patch, `docs/upstream/optional-rust-engine.patch`, not yet proposed
upstream). In-process median of 3 calls, nbdime 4.0.4, CPython 3.13, 4-vCPU Xeon at 2.30 GHz, about
5% of cells changed (`bench/upstream_engine.py`, `bench/results/2026-11-27-upstream-engine.json`):

| Notebook | Diff: Python | Diff: engine | Merge: Python | Merge: engine |
|---|---:|---:|---:|---:|
| 100 cells | 6.5 ms | 1.4 ms | 16 ms | 5.2 ms |
| 400 cells | 44 ms | 5.8 ms | 104 ms | 20 ms |
| 1,600 cells | 661 ms | 29 ms | 1.21 s | 107 ms |
| 400 cells, no ids | 1.82 s | 9.6 ms | 3.49 s | 28 ms |
| 1,600 cells, no ids | 30.4 s | 114 ms | 60.8 s | 241 ms |

Full upstream `nbdiff` on the 1,600-cell notebook without ids: 32 s with the engine off, 1.2 s
with it on.

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
deviations listed below. After `nbdime config-git --enable --global`, `git diff` and `git merge`
use the nbdime-rs drivers (`git-nbdiffdriver`, `git-nbmergedriver`) for `.ipynb` files, and
`git difftool -t nbdime` and `git mergetool -t nbdime` are set up as well.

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

## Platform status

- **Linux:** exact-output parity against upstream is tested in CI (x86-64; arm64 wheels are
  smoke-tested).
- **macOS and Windows:** the code builds, the unit tests run, and every wheel is smoke-tested in
  CI. Exact-output parity is not tested there. On Windows, upstream writes CRLF line ends in
  text output and nbdime-rs writes LF, and the Jupyter config search path still uses the POSIX
  locations (F-051 in [docs/findings.md](docs/findings.md)). Treat them as supported but not
  parity-tested.

## What is still to come

- Large differential runs and further performance work.
- The first PyPI release. The upstream integration is prepared in a local patch and has not been
  proposed upstream.
- The web UI (`nbdiff-web`, `nbmerge-web`, the server and the Jupyter extensions) is out of
  scope.

## Known issues

- Line alignment of a long cell without cell ids is quadratic in its characters: the first
  `difflib` search over both sources is the one upstream does too. Re-indenting a 1,000-line
  cell takes about 0.2 s (upstream: 27 s at 250 lines, growing with the cube).

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
