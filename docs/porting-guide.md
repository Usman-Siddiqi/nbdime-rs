# Porting guide: nbdime (Python) → nbdime-rs (Rust)

This is the contract every implementer follows. It maps nbdime's data model, algorithms and
Python idioms to their Rust equivalents and lists the semantic traps that have already been
identified. When the guide and upstream disagree, **upstream behavior wins** unless
`docs/findings.md` records a deliberate deviation.

Reference sources (read-only):
- Upstream clone at tag `v4.0.4`: `/home/claude/ref/nbdime-upstream` (Python in `nbdime/`).
- The installed oracle: `/home/claude/ref/venv` (`nbdime==4.0.4`, `nbformat==5.11.1`).
- CPython's `difflib.py`: `/usr/local/lib/python3.13/difflib.py`.

Related docs: `docs/design.md` (crate layout, API boundary, harness, upstream integration),
`docs/upstream-performance.md` (why upstream is slow), `docs/findings.md` (bugs, deviations).

---

## 1. Module map

| Python (nbdime/) | Rust (crates/nbdime-core/src/) | Notes |
|---|---|---|
| (json, nbformat reader) | `json/` (`parse.rs`, `dump.rs`), `nbformat.rs` | Own parser/serializer with Python semantics (§3, §4) |
| `diff_format.py` | `diff_format.rs` | `DiffEntry`, `Op`, builders, validation |
| `diff_utils.py` | `diff_utils.rs` | `flatten_list_of_string_diff`, `count_consumed_symbols`, ... |
| `diffing/seq_bruteforce.py`, `lcs.py`, `snakes.py`, `sequences.py` | `diffing/sequence.rs`, `diffing/snakes.rs` | LCS, multilevel snakes |
| `difflib` (stdlib) | `difflib.rs` | `SequenceMatcher` subset, exact port |
| `diffing/generic.py` | `diffing/generic.rs` | `diff`, `diff_lists`, `diff_dicts`, `compare_strings_approximate` |
| `diffing/config.py` | `diffing/config.rs` | `DiffConfig` as data, not callables (§7) |
| `diffing/notebooks.py` | `diffing/notebooks.rs`, `diffing/predicates.rs` | Notebook predicates and differs |
| `patching.py` | `patch.rs` | |
| `merging/*` | `merge/` (`generic.rs`, `decisions.rs`, `strategies.rs`, `chunks.rs`, `autoresolve.rs`, `notebooks.rs`) | |
| `prettyprint.py` | `nbdime-cli/src/pretty/` | Terminal output |
| `args.py`, `nb*app.py` | `nbdime-cli/src/` | clap, same flags |
| `vcs/git/*`, `gitfiles.py` | `nbdime-cli/src/git/` | |
| `utils.py` | spread: `path.rs` (`star_path`, `split_path`, `join_path`), `nbformat.rs` | |

`webapp/` and the TypeScript packages are out of scope; the web viewer consumes our JSON.

---

## 2. Data model

### 2.1 `Value`

```rust
pub enum Value {
    Null,
    Bool(bool),
    Int(Int),        // i64 fast path, arbitrary precision fallback (decimal digits)
    Float(f64),      // includes NaN and ±Infinity (Python's json accepts them)
    Str(String),
    List(Vec<Value>),
    Object(Map),     // insertion-ordered (IndexMap<String, Value>)
}
```

- **Objects preserve insertion order**, as Python dicts do. Never use `HashMap`/`BTreeMap` for
  JSON objects.
- **Equality must be Python equality**, implemented as `Value::py_eq`, not derived `PartialEq`:
  - `1 == 1.0 == True`, `0 == 0.0 == False == -0.0`. `NaN != NaN` (but see identity below).
  - `Int` vs `Float` compares mathematically (Python compares int and float exactly, not by
    converting the int to float: `2**53 + 1 != float(2**53 + 1)` is True).
  - Lists equal elementwise; dicts equal if same key set and values `py_eq` (order ignored).
  - Python's `==` on containers compares elements with `PyObject_RichCompareBool`, which treats
    **identical objects as equal** before comparing values. CPython's `json` returns one shared
    `NaN` object for every `NaN` literal it parses (nbformat keeps it), so in nbdime's world
    `[NaN] == [NaN]` and `{"x": NaN} == {"x": NaN}` are `True`, while a bare `NaN == NaN` (the
    `==` predicate on two list items, or `avalue != bvalue` in `diff_dicts`) is `False`.
    `Value::py_eq` therefore has two levels: top-level NaN is unequal, NaN nested in a container
    is equal to NaN. (Found by hostile review of the pilot; the first version of this guide had it
    wrong.) Values passed in through the Python bindings may carry distinct NaN objects; that edge
    is recorded in findings F-008.
- **Type identity checks** (`type(a) is type(b)`) are a separate function, `Value::same_type`,
  where `Bool`, `Int`, `Float` are three different types.
- **Atomic values**: `config.is_atomic(x)` is true for anything that is not `Str`, `List` or
  `Object`, unless a path override says otherwise.

### 2.2 Strings are sequences of code points

Python `str` indexes by code point. Every integer `key` inside a character-level diff, every
`length` of a character `removerange`, and every length used in a ratio is in **code points**,
not bytes and not UTF-16 units. Convert to `Vec<char>` (or precomputed char-boundary tables) when
doing character work; never slice a `String` with byte offsets derived from a char diff.

Lone surrogates (`"\ud800"` in JSON) are legal Python strings but cannot live in a Rust `String`.
See findings F-005 for the decision.

### 2.3 Diff entries

```rust
pub enum Key { Index(usize), Name(String) }

pub enum Op {
    Add { value: Value },
    Remove,
    Replace { value: Value },
    AddRange { valuelist: Value },   // List of items, or Str for character diffs
    RemoveRange { length: usize },
    Patch { diff: Diff },
}
pub struct DiffEntry { pub key: Key, pub op: Op }
pub type Diff = Vec<DiffEntry>;
```

- `valuelist` is a `List` for list and line diffs but a **`Str`** for character diffs (Python
  slices `b[j0:j]` of a `str`). Serialization must keep that distinction.
- JSON field order is `op`, `key`, then the payload (`value` / `valuelist` / `length` / `diff`),
  which is the kwarg order of `op_*` constructors. Python's `json.dump` preserves it.
- Mapping ops (`add`, `remove`, `replace`, `patch`) use string keys; sequence ops (`addrange`,
  `removerange`, `patch`) use integer keys. `patch_list` also accepts `add`/`remove`/`replace`
  with integer keys; merge code can produce them, so the type allows any combination.
- The merge code uses a private pseudo-op `"parent_deleted"` internally. Model it as a separate
  internal type, never as a public `Op` variant that could leak into output.

### 2.4 Builders and ordering

- `SequenceDiffBuilder.append` keeps entries sorted by key with an insertion scan: an `addrange`
  goes **before** any existing entry with key `>=` its key; any other op goes after entries with
  key `<=` its key. So at one key the order is: all addranges (in insertion order), then the
  rest (in insertion order). Reproduce this exactly (a stable sort on `(key, op != addrange)` is
  equivalent only if all entries are appended through the builder; prefer porting the rule).
- `patch`/`addrange`/`removerange` helpers drop empty payloads (`if diff:`, `if valuelist:`,
  `if length:`).
- `MappingDiffBuilder.validated()` sorts by key. Python sorts `str` by code point; Rust's
  `String` `Ord` compares UTF-8 bytes, which gives the same order. Duplicate keys are an
  assertion error upstream.

---

## 3. JSON input

Upstream reads with `json.loads` (via nbformat). Our parser (`json/parse.rs`) must accept exactly
what Python accepts, and produce the same values:

- `NaN`, `Infinity`, `-Infinity` literals are accepted (as floats).
- Integers of any size are accepted (Python int). Keep big ints exact.
- Numbers with a fraction or exponent are floats, even `1.0` or `1e2`; `-0` is the int 0 and
  `-0.0` is the float -0.0.
- **Duplicate object keys**: the last value wins, at the position of the *first* occurrence
  (Python dict assignment semantics).
- Python's `json` rejects control characters inside strings (strict mode, the default), accepts
  any escapes `\uXXXX` including surrogate pairs (combined) and lone surrogates (kept).
- Leading BOM: nbformat reads files as UTF-8 text; check `nbformat.read` behavior on a BOM before
  deciding (open question Q-002).
- Recursion: the parser must be iterative or depth-limited well above any real notebook (Python's
  default recursion limit allows ~1000 levels; match or exceed that without stack overflow).

## 4. JSON output

`nbdiff --out` uses `json.dump(d, f, indent=2, separators=(",", ": "))`; other tools use other
settings (inventory them per unit). Python's defaults that matter:

- `ensure_ascii=True`: every non-ASCII code point is written as `\uXXXX`, astral characters as a
  surrogate pair (`"😀"` → `"😀"`), lowercase hex. `"` `\` `\n` `\r` `\t` `\b` `\f`
  use short escapes; other control characters use `\u00XX`.
- `allow_nan=True`: floats print as `NaN`, `Infinity`, `-Infinity`.
- Floats print as `float.__repr__`: shortest round-trip digits; exponent form when the decimal
  exponent is `< -4` or `>= 16` (`1e-05`, `1e+16`, `1.5e+300`), otherwise positional with at
  least one fractional digit (`1.0`, `0.0001`, `123456789012345.0`). Rust's `{}`/`{:?}` do not
  match this; implement `py_float_repr` on top of a shortest-digits routine (`ryu`) and test it
  against Python on a large random sample.
- Ints print in decimal, `True`/`False`/`None` as `true`/`false`/`null`.
- With `indent`, empty containers print as `[]` and `{}`; no trailing newline after the top-level
  value (`json.dump` writes none).

## 5. nbformat reading semantics

`nbdime.utils.read_notebook` → `nbformat.read(f, as_version=4)`. For a v4 notebook this does:

1. `json.loads` (§3).
2. `rejoin_lines`: for every cell, a list `source` is joined with `""`; attachments' mimebundles
   are rejoined; for code cells, `execute_result`/`display_data` `data` mimebundles are rejoined
   (`_rejoin_mimebundle`: a value is joined only if the key is not a JSON mimetype
   (`_is_json_mime`) **and** the value is a list whose items are all strings), and other output
   types with a list `text` are joined. Read `nbformat/v4/rwbase.py` in the venv before
   implementing.
3. `strip_transient`: removes `metadata.orig_nbformat`, `metadata.orig_nbformat_minor`,
   `metadata.signature`, and every cell's `metadata.trusted`.
4. `convert(nb, 4)`: no-op for v4. v3 and older are upconverted (out of scope for Phase 1, see
   ROADMAP).
5. `validate(nb)`: for nbformat `>= 4.5`, **missing cell ids are filled with random ids and
   duplicate ids are replaced with random ids** (`_normalize`, `repair_duplicate_cell_ids=True`).
   This makes upstream output nondeterministic for such files. See findings F-003.
   Validation errors are only logged, never fatal.

`on_null='empty'` (the `/dev/null` path) yields `{}`, `'minimal'` yields `nbformat.v4.new_notebook()`
(key order `nbformat`, `nbformat_minor`, `metadata`, `cells`).

## 6. Diff algorithms

### 6.1 Generic `diff(a, b, path, config)`

- list/list → `diff_lists`; dict/dict → `diff_dicts`; str/str → `diff_strings_linewise`; anything
  else raises. Python dispatches on `isinstance`, so `bool` is never a list/dict/str.
- The result is validated (`validate_diff`, shallow). Validation failures are internal errors.

### 6.2 `diff_dicts`

Order of emitted ops (then sorted by key by the builder): removes for `sorted(a - b)`, then for
`sorted(a & b)`: if `type(av) is type(bv)` and not atomic → recurse with `config.differs[subpath]`
(patch if non-empty) **else** `replace` if `av != bv` (Python `!=`, so `1` vs `1.0` produces
nothing), then adds for `sorted(b - a)`.

Before the replace, upstream raises `RuntimeError` if `(path or '/') in config.predicates`. Because
`config.predicates` is a `defaultdict2` that **inserts** every key it is queried with, this check
depends on which list paths were diffed earlier in the same process. See findings F-001: we do not
replicate the raise.

`subpath = path + "/" + key` (string join, so the root path `""` gives `"/key"`).

### 6.3 `diff_lists` and the sequence algorithms

- If `config.predicates[path or '/']` has more than one predicate → `diff_sequence_multilevel`.
- Otherwise a shallow diff with `diff_sequence(a, b, compares[0])` using the **brute-force LCS**
  (`seq_bruteforce.py`; `diff_sequence_algorithm = "bruteforce"` is hard-coded), then recursion into
  the aligned unchanged items: for every pair consumed by "skipping" between entries, if not atomic,
  `diffit = config.differs[path + "/*"]` and a non-empty result becomes a `patch` at index `i+k`.
- **Brute-force LCS tie-breaking** must match exactly. Backtrack from `(N, M)`: if `G[x-1][y-1]`
  take the diagonal, elif `R[x][y] == R[x-1][y]` decrement `x`, else decrement `y`. Any faster
  algorithm must produce the identical index lists. Suffix stripping is safe; prefix stripping is
  not (see `docs/upstream-performance.md`).
- `diff_from_lcs` emits, per LCS pair, `removerange(x, i-x)` then `addrange(x, B[y:j])`, and the
  tail likewise; the builder reorders addrange before removerange at the same key.

### 6.4 Multilevel snakes (`snakes.py`)

`compute_snakes_multilevel(A, B, compares, rect, level)`: start at the **last** (strictest)
predicate; compute brute-force snakes in `rect`; for every gap between consecutive snakes (and the
final corner `(i1, j1, 0)`), if the gap is non-empty in **both** dimensions, recurse with
`level-1` on that sub-rectangle; merge snakes that are contiguous. Port with an explicit loop or
bounded recursion (depth = number of predicates, at most 4).

`compute_diff_from_snakes` emits, per snake: `removerange(i0, i-i0)`, `addrange(i0, b[j0:j])`, then
for each aligned pair calls `diffit` **unconditionally** (no atomic check here, unlike
`diff_lists`) and adds non-empty results as `patch`.

### 6.5 Strings

- `diff_strings_linewise(a, b)`: equal → `[]`; else split both with **`str.splitlines(True)`**
  and diff the line lists with a private config: predicates `[compare_strings_approximate, ==]`
  for every path, differ `diff_strings_by_char` for every path.
- `str.splitlines` splits on `\n`, `\r`, `\r\n`, `\v` (`\x0b`), `\f` (`\x0c`), `\x1c`, `\x1d`,
  `\x1e`, `\x85`, ` `, ` `, keeping the terminator. `str::lines` is wrong.
  `"".splitlines(True) == []`.
- `diff_strings_by_char(a, b)`: equal → `[]`; else `SequenceMatcher(None, a, b, autojunk=False)`
  opcodes over **code points**, converted: `replace` → `removerange` + `addrange`, `insert` →
  `addrange`, `delete` → `removerange`. `valuelist` is a `Str`.
- `diff_string_lines` (cell source differ): short-circuits on equality, then
  `diff_strings_linewise`.

### 6.6 `difflib.SequenceMatcher` (exact port)

Only the `isjunk=None, autojunk=False` configuration is used, so there are no junk or popular
elements. Port from `/usr/local/lib/python3.13/difflib.py`:

- `b2j`: element → ascending list of indices in `b`.
- `find_longest_match(alo, ahi, blo, bhi)`: the `j2len`/`newj2len` dynamic programme; a candidate
  replaces the best only if `k > bestsize` (strictly), which fixes tie-breaking to the earliest `i`,
  then the earliest `j`. The junk-extension loops are no-ops in this configuration but keep the
  semantics in mind if they ever are not.
- `get_matching_blocks`: an explicit **stack** (`queue.pop()` pops the last), then `sort()`, then
  collapse adjacent blocks, then append the sentinel `(la, lb, 0)`.
- `get_opcodes`: as upstream.
- `ratio() = 2.0 * M / T` where `T = la + lb`; `_calculate_ratio` returns `1.0` when `T == 0`.
  Compute in `f64` in the same order (`2.0 * m as f64 / t as f64`) so threshold comparisons agree
  bit for bit.
- `quick_ratio()`: multiset intersection size. `real_quick_ratio()`: `2.0 * min(la, lb) / T`.
- For speed, `quick_ratio` and `real_quick_ratio` must not require building `b2j` (§9).

### 6.7 `compare_strings_approximate(x, y, threshold=0.7, maxlen=None)`

In order: `bool(x) != bool(y)` → False; `len(x) == len(y) and x == y` → True;
`real_quick_ratio < threshold` → False; `quick_ratio < threshold` → False; if `maxlen` is not None
and **both** lengths exceed it → False; finally `ratio() > threshold` (strict `>`). Note the bounds
use `<` and the final test uses `>`: a ratio of exactly `0.7` is rejected by the final test but
passes the bounds. Lengths are code points. `x`, `y` may be strings (char sequences) or, in
principle, lists; in nbdime they are always strings at this call site.

### 6.8 Notebook predicates (`diffing/notebooks.py`)

Port each predicate literally; the details are where parity is won or lost:

- `compare_text_approximate`: lists are joined with `""`; `bool(x) != bool(y)` → False; **both
  shorter than 10 code points → True** (even if completely different); else
  `compare_strings_approximate(x, y, 0.7, maxlen)`.
- `compare_text_strict`: join lists; exact equality → True; else threshold `0.95`.
- `_compare_mimedata`: mimetype lowercased; for `text/plain` without `\n` in either value, equal
  after `re_pointer.split` (`0x[a-f0-9]{8,16}`, case-insensitive) → True; `text/*` → text compare;
  other string pairs → if `_is_base64(x)` (only `x` is tested) → strict equality, otherwise the
  text compare; non-strings → `==`. `_is_base64`: the length check is on the *original* string
  (`len(s) < 64` → False); the regex is then matched against `''.join(s.splitlines())`, so every
  `splitlines` boundary (§6.5), not just `\n`, is removed first. The regex is case-insensitive and
  anchored `^...$` (Python's `$` also matches before a final `\n`, which cannot remain here).
- `compare_output_approximate` / `compare_output_strict` / `compare_mimebundle_*`: note that
  `set(x.keys()) != set(y.keys())` is a key-**set** comparison, and the "play safe on unknown keys"
  loop compares remaining keys with Python `!=`.
- `compare_outputs_approximate` calls the **configured** differ for `/cells/*/outputs` (global
  state, affected by ignore flags) and returns False if any resulting op is not a patch. When
  outputs are ignored the differ returns `[]`, so the predicate is True. Our config model must
  reproduce this coupling (§7).
- `compare_cell_moderate`: for code cells, `xop = x["outputs"] or ()`; `bool(xop) != bool(yop)` →
  False.
- `compare_cell_by_ids`: both have `"id"` and the values are equal.
- `/cells` predicates, low to high: approximate, moderate, strict, by_ids. `/cells/*/outputs`:
  approximate, strict.

### 6.9 Notebook differs

- `/cells/*/outputs/*` → `diff_single_outputs`: for `execute_result`/`display_data`, diff the
  output without `data` with the **default** generic `diff` (not the notebook config!), append
  its entries, then `patch("data", diff_mime_bundle(...))`. Other output types: `diff(a, b,
  config=config)`.
- `diff_mime_bundle` / `add_mime_diff`: equal strings → nothing; mimetypes starting with `text/`,
  `image/svg+xml`, `application/javascript`, `application/json` → recursive generic `diff` (default
  config); otherwise `replace` if `!=`.
- `/cells/*/attachments` → `diff_attachments`, `/cells/*/source` → `diff_string_lines`,
  `/cells/*/id` is atomic.

## 7. Configuration and ignore flags

Upstream configures diffing by mutating a global `notebook_differs` table of callables
(`set_notebook_diff_ignores`, `set_notebook_diff_targets`). In Rust, `DiffConfig` is plain data:

```rust
pub enum DifferRule { Default, Ignore, IgnoreKeys(BTreeSet<String>, Box<DifferRule>) }
```

keyed by starred path, plus the predicate table and atomic paths. `diff_ignore_keys` filters only
the **top level** of the inner differ's result. `set_notebook_diff_targets` maps the CLI flags to
the rule table exactly as upstream (note `details=False` ignores `execution_count` on `/cells/*`
and `/cells/*/outputs/*`). Because predicates call the configured outputs differ (§6.8), the
predicate code takes the config as a parameter; there is no global state.

## 8. Patching

`patch_list` deep-copies untouched items, honours `take = max(take, index + skip)`, and accepts
the mapping ops with integer keys. `patch_string` flattens a line-based diff to a character diff
(`flatten_list_of_string_diff`) and patches the code point list. `patch_dict` asserts on invalid
combinations; port these as errors, not panics.

## 9. Performance rules (without breaking parity)

- Precompute per-item data once per diff (code point arrays, lengths, histograms, hashes) and
  compute predicates from it.
- `real_quick_ratio` from lengths, `quick_ratio` from histograms; build a full matcher only when
  both bounds pass. Memoize predicate results per `(i, j)` where the same pair is evaluated at
  several levels.
- LCS tables in flat `Vec<u32>`; the compare grid as a bitset.
- Shortcuts must provably give identical output. Record each one with its argument in the code
  (one line) and in `docs/findings.md` if it is non-obvious.

## 10. Python idioms → Rust

| Python | Rust |
|---|---|
| `dict` (ordered) | `IndexMap<String, Value>` |
| `a == b` on JSON values | `Value::py_eq` |
| `type(a) is type(b)` | `Value::same_type` |
| `bool(x)` on str/list/dict | `!is_empty()`; on numbers `!= 0`; `None` false |
| `sorted(set_of_str)` | collect into `Vec<&str>`, `sort_unstable()` (byte order = code point order) |
| `sorted(..., reverse=True)` (stable) | `sort_by(|a, b| b.cmp(a))` (stable, same tie order) |
| tuple ordering with mixed int/str | explicit enum with derived `Ord` mirroring Python's rules |
| `defaultdict2` of callables | data-driven rule table (§7) |
| `assert` used for validation | `Result` errors |
| sentinels `Missing`, `Deleted`, `ParentDeleted` | `Option`/dedicated enums |
| `copy.deepcopy` | `clone()` (only where ownership requires it) |
| `str.splitlines(True)` | `py::splitlines_keepends` |
| `re` with `IGNORECASE`/`UNICODE` | `regex` crate; check `$` semantics (Python `$` matches before a final `\n`) |
| `lru_cache` | not needed for semantics; use explicit memo tables where it pays off |

## 11. Known semantic traps (checklist for reviewers)

1. Python numeric equality across int/float/bool (§2.1).
2. Code point indexing everywhere in string diffs (§2.2).
3. `splitlines` boundaries (§6.5).
4. Builder ordering of addrange vs other ops at the same key (§2.4).
5. Brute-force LCS backtrack tie-breaking (§6.3).
6. difflib `find_longest_match` strict `>` and stack order in `get_matching_blocks` (§6.6).
7. `<` vs `>` thresholds in `compare_strings_approximate` (§6.7); the 10-code-point short-string rule.
8. Ratios computed in f64 in the same operation order.
9. Ignore flags change cell *alignment*, not just the output (§6.8, §7).
10. `diff_single_outputs` uses the default config for the non-data part.
11. `valuelist` is a `Str` in character diffs.
12. JSON output: `ensure_ascii`, float repr, key order (§4).
13. nbformat read normalizations: rejoin, strip transient, random ids (§5).
14. Python regex `$` and `re.IGNORECASE` on non-ASCII (Unicode case folding in Python vs `regex`).
15. Python `str.lower()` (used on mimetypes) is full Unicode lowercasing; Rust `to_lowercase` is
    close but not identical for a few characters (e.g. final sigma handling). Mimetypes are ASCII
    in practice; note any divergence.
16. `datetime.isoformat(" ")` omits microseconds when they are zero (pretty-print headers).
17. `pprint.pformat` for non-string values in pretty-print output (wraps at width 80, Python
    reprs: `True`, `None`, `'str'` with Python quoting rules).
