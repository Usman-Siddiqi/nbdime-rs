# Unit U02: reading and writing notebooks (nbformat semantics)

Unit spec for implementer, reviewers and fixer. The orchestrator owns it.

## Scope

`crates/nbdime-core/src/nbformat.rs` (a directory module if it grows): what nbdime gets from
`nbformat` 5.11.1 for **v4 notebooks**, reading and writing. Read the sources yourself, in
`/home/claude/ref/venv/lib/python3.13/site-packages/nbformat/`: `__init__.py` (`reads`, `writes`,
`write`), `reader.py`, `v4/nbjson.py`, `v4/rwbase.py` (`rejoin_lines`, `split_lines`,
`strip_transient`, `_is_json_mime`, `_split_mimebundle`), `validator.py` (`validate`,
`_validate`, `_normalize`), `converter.py`, `v4/nbbase.py` (`new_notebook`). Also nbdime's
`utils.read_notebook` (`/home/claude/ref/nbdime-upstream/nbdime/utils.py`).

Porting guide: sections 2, 3, 4, 5, 10, 11.

## API (suggested; keep it small)

```rust
pub fn reads(text: &str) -> Result<Value, NbformatError>;   // nbformat.reads(text, as_version=4)
pub fn writes(nb: &Value) -> Result<String, NbformatError>; // what nbformat.write(nb, fp) writes
pub fn new_notebook() -> Value;                              // nbformat.v4.new_notebook(), key order kept
```

plus whatever `read_notebook(path, on_null, on_empty)` needs later (the CLI unit adds file I/O;
keep this module free of file I/O).

## Semantics

Reading, in upstream order: JSON parse (existing `json` module, Python semantics) → version check
→ `from_dict` (no-op for us) → `rejoin_lines` → `strip_transient` → (convert: no-op for v4) →
`validate`, of which only `_normalize`'s id repair has an effect (schema errors are only logged
upstream; we do not log them, findings F-013). Any exception upstream raises along the way is an
error for us; where upstream succeeds on odd input (for example `"cells": {}`), we succeed with
the same result. `parity/test_u02_nbformat.py::test_u02_reads_malformed` pins this down.

Writing, in upstream order: version check → `validate` (id repair, mutating) → deep copy →
`split_lines` → `strip_transient` → `json.dumps(indent=1, sort_keys=True, separators=(",", ": "),
ensure_ascii=False)` → append `"\n"` if the text does not end with one (`nbformat.write`).
Add a `DumpOptions` preset for this; key sorting is by code point (Rust `str` order is the same).

Details that are easy to get wrong:
- `_rejoin_mimebundle` joins a value only if the key is not a JSON mime type and the value is a
  list of **only** strings. `_split_mimebundle` splits only `text/*` (case-sensitive) and
  `application/javascript`, `image/svg+xml`. Reading rejoins any output type's list `text`
  except `execute_result`/`display_data` (including `error` outputs that have one); writing
  splits only `stream` text. Only `code` cells' outputs are touched on read.
- `splitlines(True)` boundaries (`py::splitlines_keepends`).
- Generated ids are appended at the end of the cell's keys; replaced duplicate ids keep their
  position.

## Deliberate deviations (record nothing new; they are already in docs/findings.md)

- **F-003, deterministic ids.** Repair applies when `(nbformat, nbformat_minor) >= (4, 5)`.
  1. For each cell in order that has no `"id"`: `id = gen(cell)` where the hash input is the
     cell's canonical JSON (compact, keys sorted, `ensure_ascii=True`) as it stands at that
     point (after rejoin and strip), and
     `gen = first 8 chars of format!("{:016x}", fnv1a64(input + suffix))`, suffix `""` then
     `":1"`, `":2"`, ... until the id equals no string id anywhere in the notebook (explicit or
     already generated).
  2. Then scan the cells in order with a seen-set, using Python set semantics
     (`Value::py_eq_item`, so `1`, `1.0` and `true` are the same id); a cell whose id was seen
     gets a new id from `gen(cell without its id)` with the same suffix rule, in place.
  The same cell content therefore gets the same id wherever it sits, which keeps diffs of two
  such notebooks free of spurious id changes. Applies to writing too.
  3. Writing repairs ids before it splits lines and strips transient metadata, but the hash input
     is always the cell as `reads` returns it: on a copy of the cell, `source`, attachments and
     outputs are rejoined and `metadata.trusted` is removed first (best effort: a cell that
     cannot be rejoined cannot be read back, and any deterministic form will do). So `writes` of
     the same cell given with `"source": "s"`, `"source": ["s"]` or `"metadata": {"trusted":
     true}` generates the same id, equal to what `reads` generates for the written file.
  4. The suffix sequence depends only on the hash of the content, and the set of taken ids only
     grows, so implementations remember the next suffix to try per content hash: repairing n
     identical cells is O(n), not O(n^2).
- **Q-002, BOM**: `reads` ignores one leading U+FEFF. The JSON layer still rejects it.
- **F-012, versions**: only `nbformat == 4` (an integer, not `4.0` or `true`) is supported.
  Anything else, including a missing `nbformat` key (upstream treats that as v1), is an error
  whose message names the version. For 1 to 3 it suggests converting with
  `jupyter nbconvert --to notebook`; for 5 and above it says the notebook is newer; for the
  rest (`4.0`, `true`, a string, a missing key, ...) it says `"nbformat"` must be the integer 4.

## Dev tool functions (fixed by the parity suite)

| fn | args | returns |
|---|---|---|
| `nbformat.reads` | `{text: str}` | the notebook |
| `nbformat.writes` | `{nb}` | the file text |

## Performance

Linear in the input. No recursion beyond document depth. Do not clone the notebook more than
once per call.

## Acceptance

`scripts/gate.sh parity/test_json_io.py parity/test_pilot_generic_diff.py parity/test_u02_nbformat.py`
prints `GATE PASSED`.
