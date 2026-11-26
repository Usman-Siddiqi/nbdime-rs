## Summary

Add an optional `nbdime_rs` backend for notebook diffing and merge-decision calculation. The
backend is loaded lazily when nbdime can use it and advertises compatibility with nbdime 4.0.4.
The public function signatures stay the same. The adapter restores Python diff and decision
objects before returning results to existing callers.

`NBDIME_ACCEL=0` selects the existing Python implementation without importing the extension.
Custom differ configuration, unsupported inputs and ambiguous merge payload types follow the
Python path. The proposed `accel` extra uses a prebuilt wheel, so nbdime's build requires no Rust
toolchain.

## Verification

- Run nbdime's non-web test suite with `NBDIME_ACCEL=0` and `NBDIME_ACCEL=1` against the same
  wheel and source checkout.
- Exercise disabled, missing and incompatible engines, custom configuration, list-source
  fallback, display-output order and Python result types.
- Install and import the x86-64 manylinux2014 `cp39-abi3` wheel in clean CPython 3.9 and 3.13
  environments.
