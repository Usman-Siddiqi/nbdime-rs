# Unit P1 (pilot): generic diff engine

This file is the unit spec handed to the implementer, reviewers and fixer. The orchestrator
owns it.

## Scope

Port, into `crates/nbdime-core`, the parts of upstream nbdime 4.0.4 that make up the generic
diff engine:

| Upstream | Rust |
|---|---|
| `difflib.SequenceMatcher` (stdlib, `isjunk=None, autojunk=False` only): `get_matching_blocks`, `get_opcodes`, `ratio`, `quick_ratio`, `real_quick_ratio` | `src/difflib.rs` |
| `diffing/seq_bruteforce.py`, `diffing/lcs.py` | `src/diffing/sequence.rs` |
| `diffing/snakes.py` | `src/diffing/snakes.rs` |
| `diffing/sequences.py` (`diff_strings_by_char`, `diff_strings_linewise`, `diff_sequence` with the hard-coded brute-force algorithm), `diffing/seq_difflib.py` | `src/diffing/strings.rs` (or `sequence.rs`) |
| `diffing/generic.py` (`diff`, `diff_lists`, `diff_dicts`, `diff_sequence_multilevel`, `diff_string_lines`, `compare_strings_approximate`) | `src/diffing/generic.rs` |
| `diffing/config.py` | `src/diffing/config.rs` |
| `diff_utils.count_consumed_symbols` | where needed |

Read the upstream sources (`/home/claude/ref/nbdime-upstream/nbdime/diffing/`, `diff_format.py`,
`diff_utils.py`) and `/usr/local/lib/python3.13/difflib.py` yourself; do not work from memory.

## Interfaces

Use the existing foundation: `Value` (`value.rs`, with `py_eq`, `same_type`, `truthy`),
`json`, `py::splitlines_keepends`, and `diff_format` (`DiffEntry`, `Op`, `Key`,
`SequenceDiffBuilder`, `MappingDiffBuilder`, `validate_diff`). Extend them only if necessary and
keep their existing behavior.

`DiffConfig` must be **data, not closures**, so that later units can add notebook predicates and
differs and so that the Python bindings can describe a configuration:

```rust
pub enum Predicate { Eq, StringsApproximate /* notebook predicates are added by later units */ }
pub enum Differ { Generic, StringsByChar /* later: StringLines, SequenceMultilevel, Ignore, ... */ }
pub struct DiffConfig { /* predicates per path (default [Eq]), differs per path (default Generic),
                           atomic path overrides */ }
```

Paths are built exactly as upstream builds them (`"/".join((path, key))`, `path + "/*"`), and
looked up with `path or "/"` where upstream does. Expose at least:

```rust
pub fn diff(a: &Value, b: &Value) -> Result<Diff, DiffError>;                       // nbdime.diff
pub fn diff_with_config(a: &Value, b: &Value, path: &str, config: &DiffConfig) -> Result<Diff, DiffError>;
pub fn diff_strings_linewise(a: &str, b: &str) -> Diff;
pub fn diff_strings_by_char(a: &str, b: &str) -> Diff;
pub fn compare_strings_approximate(x: &str, y: &str, threshold: f64, maxlen: Option<usize>) -> bool;
```

plus whatever the dev tool needs. Mismatched or non-container top-level types are an error,
as upstream raises.

## Semantics to get exactly right

Read `docs/porting-guide.md` sections 2, 6.1 to 6.7, 9, 10 and 11 before writing code. In
particular:

- `diff_dicts`: never raise the "Found predicate(s) ... pointing to dict entry" error
  (findings F-001); produce the replace.
- Brute-force LCS backtracking order, `diff_from_lcs`, builder ordering, multilevel snakes merge
  of contiguous snakes, `compute_diff_from_snakes` calling the differ unconditionally.
- difflib: `find_longest_match` with strict `>`, `get_matching_blocks` stack order and
  collapsing, `ratio` arithmetic order, `_calculate_ratio` returning 1.0 for empty input.
- Code point indexing; `valuelist` is a `Str` in character diffs; `splitlines` boundaries.
- Python equality in the `==` predicate (`1 == 1.0 == True`).

## Performance requirements

- No recursion whose depth grows with the number of list items, lines or characters.
- LCS tables in flat buffers; do not allocate per cell.
- `compare_strings_approximate` must decide `real_quick_ratio` from lengths and `quick_ratio`
  from character counts without building the full matcher; build the matcher only when needed.
- Character diffs work on `Vec<char>` (or equivalent) built once per string.

## Dev tool functions (fixed by the parity suite)

Add these to `crates/nbdime-cli/src/dev/mod.rs` (`dispatch`), with exactly these names and
argument shapes; return values must serialize to the same JSON the oracle produces:

| fn | args | returns |
|---|---|---|
| `difflib.opcodes` | `{a: str, b: str}` | `[[tag, i1, i2, j1, j2], ...]` |
| `difflib.matching_blocks` | `{a, b}` | `[[i, j, n], ...]` including the final `(len(a), len(b), 0)` |
| `difflib.ratios` | `{a, b}` | `[ratio, quick_ratio, real_quick_ratio]` (floats) |
| `compare_strings_approximate` | `{x, y, threshold?: float (default 0.7), maxlen?: int}` | bool |
| `diff_strings_by_char` | `{a, b}` | diff |
| `diff_strings_linewise` | `{a, b}` | diff |
| `snakes_multilevel_lines` | `{a: [str], b: [str]}` | `[[i, j, n], ...]` from `compute_snakes_multilevel(a, b, [compare_strings_approximate, ==])` |
| `diff_sequence_bruteforce` | `{a: list, b: list}` | diff with Python `==` as the predicate |
| `diff` | `{a, b}` | `nbdime.diff(a, b)` |

Note that `threshold` may arrive as an int-valued JSON number (`0` or `1`) or a float.

## Acceptance

`scripts/gate.sh parity/test_json_io.py parity/test_pilot_generic_diff.py` passes: rustfmt,
clippy `-D warnings`, unit tests, and every parity test in those files (about 40,000 cases, no
skips). Plus two hostile reviews signed off.
