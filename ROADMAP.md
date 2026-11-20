# Roadmap

Status legend: ✅ done · 🔄 in progress · ⏳ not started · ⛔ blocked

Every unit's exit condition: compiles warning-free (fmt, clippy `-D warnings`), its parity suite
passes with no skips and at least the stated case count, two hostile reviews signed off, merged by
the orchestrator. Parity is measured against nbdime 4.0.4.

## Phase 0: understand and design ✅
- Read upstream (diffing, merging, patching, prettyprint, CLI, git integration, tests, packaging).
- Reproduce and explain the scaling problem: `docs/upstream-performance.md`.
- `docs/porting-guide.md`, `docs/design.md`, `docs/findings.md`, `CLAUDE.md`.

## Phase 1: foundation and pilot ✅ (pilot merged; awaiting go-ahead for Phase 2)
| Unit | Owner | Status | Acceptance |
|---|---|---|---|
| F1 workspace, CI, license | orchestrator | ✅ | `cargo build`, CI green |
| F2 `Value`, Python equality, truthiness | orchestrator | ✅ | unit tests incl. int/float/bool cross-equality |
| F3 JSON parse/dump with Python semantics | orchestrator | ✅ | parity: 10k random docs, float repr sample of 100k, byte-identical to `json.dumps` |
| F4 diff format types + JSON | orchestrator | ✅ | round-trips upstream test diffs byte for byte |
| F5 parity harness + dev batch tool | orchestrator | ✅ | runs, counts cases, rejects skips |
| F6 benchmark harness | orchestrator | ✅ | upstream timings reproducible from `bench/run.py` |
| **P1 pilot: generic diff engine** | implementer loop | ✅ | see below |

**P1 pilot scope**: `difflib.SequenceMatcher` (matching blocks, opcodes, ratio, quick_ratio,
real_quick_ratio), `diff_strings_by_char`, `diff_strings_linewise`, `compare_strings_approximate`,
brute-force LCS, multilevel snakes, `diff_lists`, `diff_dicts`, generic `diff(a, b)` with the
default config. Parity: `nbdime.diff` and the individual functions on ≥ 20,000 generated cases and
all string fields of the upstream test notebooks, byte-identical JSON.

## Phase 2: fan out the port 🔄
| Unit | Depends on | Scope |
|---|---|---|
| U02 nbformat read/write | F3 | rejoin/split lines, strip transient, id policy (F-003), v3→v4 upconversion, `nbformat.write` formatting |
| U03 notebook predicates | P1 | all `compare_*` predicates, precomputed per-cell data, exact results; decide text-as-list operands (F-011) |
| U04 `diff_notebooks` + ignore config | P1, U03 | differs, `diff_single_outputs`, mime bundles, attachments, targets/ignores |
| U05 patch + diff utils | F4 | `patch`, `flatten_list_of_string_diff`, `to_json_patch` (if needed); `diff_from_value` key types (bool/negative/big int keys accepted by Python) |
| U06 diff pretty-printer | U04 | `pretty_print_notebook_diff` byte parity incl. git `--color-words` rendering |
| U07 `nbdiff` CLI | U04, U06 | flags, `--out`, gitref mode, exit codes |
| U08 `nbshow` | U02 | flags and output |
| U09 `nbpatch` | U05 | |
| U10 merge engine (was U10 to U12) | U04, U05 | decisions, chunks, apply, generic merge, strategies, autoresolve, `decide_notebook_merge`/`merge_notebooks`; spec `docs/process/u10-merge.md` |
| U13 `nbmerge` CLI | U10, U02 | strategies from args, merge pretty-print, exit codes |
| U14 git integration | U07, U13 | diff driver, merge driver, `nbdime config-git`, filters |
| U15 Python package `nbdime_rs` | U04, U13 | PyO3 API per `docs/design.md`, wheels via maturin |
| U16 git-compatible text diff/merge | withdrawn | licensing (F-023); git is spawned like upstream (F-022). `parity/test_u16_xdiff.py` now checks U06's git renderers |

## Phase 3: integration and parity ⏳
Full upstream-derived test set through the harness, large differential runs (≥ 100k notebook
pairs/triples), triage → fix → review loop until parity targets are met or remaining differences
are documented deviations.

## Phase 4: performance and polish ⏳
Benchmarks vs upstream at 100 to 10,000 cells, profiling, algorithmic work that keeps parity,
pip packaging, docs, README with charts from real runs.

## Phase 5: upstream contribution prep (local only) ⏳
Local nbdime clone, optional engine integration, tests with and without the engine,
`docs/upstream/proposal.md`, `docs/upstream/pr-description.md`, skeptical-maintainer review, then
stop and ask.

## Parity status
| Area | Status |
|---|---|
| JSON I/O | byte-identical to Python `json` on all suites (100k floats, 5k docs, edge cases); deviations F-005 (lone surrogates rejected) |
| Generic diff | identical to nbdime 4.0.4 on ~45k cases per run (also at 4x scale, seed 17); deviations F-001 (no spurious error), F-009 (depth limit), F-011 (approximate predicate scope) |
| Notebook diff | identical on the U04 suite (upstream families, ~10k random/flagged pairs, predicates); deviations F-003, F-014, F-016, F-017, F-019; F-020 replicated |
| Patch | identical on the U05 suite (~20k cases); deviation F-015 |
| Merge | identical on the U10 suite (decisions, apply, notebook merges, strategies); deviations F-021, F-027; F-024 replicated |
| Pretty-printer | byte-identical on the U06 suite (diffs, merge decisions, values, git renderer suite U16); deviations F-030 |
| CLI output | `nbdiff`/`nbmerge`/`nbdime` identical on `test_cli.py` (exit code, stdout, output file, stderr) incl. git ref mode, config files, review regressions; deviations F-029, F-031 |
