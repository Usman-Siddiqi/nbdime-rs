## Summary

Add an optional `nbdime_rs` backend for notebook diffing and merge-decision calculation. The
backend is loaded lazily when nbdime can use it and advertises compatibility with nbdime 4.0.4.
The public function signatures stay the same. The adapter restores Python diff and decision
objects before returning results to existing callers.

`NBDIME_ACCEL=0` selects the existing Python implementation without importing the extension.
`NBDIME_ACCEL=force` raises when the extension is missing, incompatible or reports an error,
instead of falling back, so CI can show that the engine runs. With the default value, an engine
error is logged at debug level and the Python implementation is used. Loading the extension is
guarded by a lock.
Custom differ configuration, unsupported inputs and ambiguous merge payload types follow the
Python path. The proposed `accel` extra uses a prebuilt wheel, so nbdime's build requires no Rust
toolchain.

## Verification

- Run nbdime's non-web test suite with `NBDIME_ACCEL=0`, the default and `NBDIME_ACCEL=force`
  against the same wheel and source checkout. `test_accel.py` also compares diffs and merges of
  nbdime's own test notebooks between the extension and pure Python when the extension is
  installed.
- Exercise disabled, missing and incompatible engines, custom configuration, list-source
  fallback, display-output order and Python result types.
- Install and import the x86-64 manylinux2014 `cp39-abi3` wheel in clean CPython 3.9 and 3.13
  environments.

## Performance

On generated notebooks of 100 to 1600 code cells with source edits in about 5% of the cells,
the median in-process diff on CPython 3.13 was 4x faster at 100 cells (2.8 ms to 0.7 ms), 26x
faster at 800 cells (135 ms to 5.2 ms) and 56x faster at 1600 cells (584 ms to 10.4 ms). Inputs
that fall back to Python see no change.
