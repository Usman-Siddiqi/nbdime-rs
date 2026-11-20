# Unit U05: patch and diff utilities

Unit spec for implementer, reviewers and fixer. The orchestrator owns it.

## Scope

Port `nbdime/patching.py` (`patch`, `patch_list`, `patch_dict`, `patch_string`,
`patch_singleline_string`, `patch_notebook`) to `crates/nbdime-core/src/patch.rs`, and the rest
of `nbdime/diff_utils.py` that later units need (`flatten_list_of_string_diff` with
`_overlaps`/`_combine_ops`, `offset_op`, `source_as_string`; `count_consumed_symbols` already
exists) to `diff_utils.rs`. `to_json_patch` is out of scope (unused, prints a warning upstream).
Read the upstream files yourself. Porting guide: sections 2, 8, 10, 11.

```rust
pub fn patch(obj: &Value, diff: &[DiffEntry]) -> Result<Value, PatchError>;
pub fn flatten_list_of_string_diff(a: StrOrLines, diff: &[DiffEntry]) -> Result<Diff, DiffFormatError>;
```

## Semantics to get exactly right

- `patch` dispatches on dict / list / str; anything else is an error. A string is **always**
  patched as a line-based diff (`patch_string` flattens first), whatever produced the diff.
- `patch_list`: `take = max(take, index + skip)`; `obj[take:index]` with Python slice clamping
  (an index past the end takes nothing more, it is not an error); `patch` entries index
  `obj[index]` (an error past the end); `add`/`remove`/`replace` are still supported.
- `patch_dict`: the result's **key order** is: keys named by `add`/`replace`/`patch` entries, in
  diff order, then the untouched keys in the original order. The assertions (`add` of an
  existing key, an entry for a key already set, `replace`/`patch` after `remove`, non-string
  key) are errors.
- `flatten_list_of_string_diff`: `line_to_char` offsets from `splitlines(True)` of `a` (or the
  given lines), code point counts; `_overlaps` only looks at the last combined op; the
  `RuntimeError` on inconsistent removerange is an error; `_combine_ops` turns `add` into
  `addrange`, and a string `valuelist` grows by string concatenation; the final stable sort by
  key.
- Diffs come in as JSON (`diff_format::diff_from_value`). For `patch`, missing fields, unknown ops
  and wrong types are errors, as they are upstream (`AttributeError`/`KeyError`/
  `NBDiffFormatError`); `flatten_list_of_string_diff` is stricter (see F-015).
- **F-015 (deliberate deviation, new)**: list keys and `removerange` lengths must be non-negative
  integers in `i64` range; booleans, negatives, floats and larger integers are rejected, and
  merged lengths beyond `u64` are an error. See F-015 in `docs/findings.md`.

## Performance

Linear in the size of the object plus the diff. No recursion beyond document depth. Avoid
deep-copying untouched subtrees more than once.

## Dev tool functions (fixed by the parity suite)

| fn | args | returns |
|---|---|---|
| `patch` | `{obj, diff}` | patched value |
| `flatten_list_of_string_diff` | `{a: str \| [str], diff}` | diff |

## Acceptance

`scripts/gate.sh parity/test_json_io.py parity/test_pilot_generic_diff.py parity/test_u05_patch.py`
prints `GATE PASSED`, plus one hostile review (B, lookalike hunt) signed off.
