# U15 hostile review (branch unit/u15 at 745a449): SIGN-OFF no

Fix in this order. Turn each into a case in `parity/test_u15_python.py` first.

1. **MAJOR, circular references** (crates/nbdime-py/src/convert.rs:103-113, :160-171). The only
   thing that stops a cycle is the 10,000-level depth limit, and before it trips every level
   deep-copies all the siblings again. Repro: `d = {str(i): i for i in range(2000)}; d["~self"] = d;
   nbdime_rs.diff(d, {})` reaches 2.9 GB and then SIGABRT. Expected: an immediate ValueError
   ("Circular reference detected", the same as json.dumps). Fix: keep a set of the `as_ptr()` of
   the containers currently on the stack.
2. **Dict mutated during conversion** (convert.rs:183). When a GC `__del__` adds a key, PyO3
   panics and the caller gets PanicException, which is a BaseException. Fix: iterate the dict
   yourself and raise `RuntimeError("dictionary changed size during iteration")`, or use
   catch_unwind.
3. **dict/list subclasses**: these are read in storage order. json reads them through `.items()`
   or iteration. Repro: `OrderedDict` followed by `move_to_end("a")`; upstream and json.dumps give
   b, c, a. Fix: keep the fast path for exact dict and list only.
4. Ctrl-C is not seen until the engine returns. Document this in F-041.
5. The wheel has no LICENSE file (pyproject needs `license-files`), and its tag is
   manylinux_2_39. Build manylinux2014 for PyPI.
6. Nothing runs nbdime-py's own clippy or unit tests. Add `cargo clippy -p nbdime-py --all-targets
   -- -D warnings` and `cargo test -p nbdime-py --no-default-features` (LD_LIBRARY_PATH set to the
   venv libdir) to the suite.
7. At a 32 KB thread stack, an inline call segfaults (run.rs:409, INLINE_DEPTH 24). F-041's
   "512 KB" figure is wrong.
8. F-041's timings are optimistic: patch of a 25-deep dict takes 115-830 µs.
9. When the worker thread can't start (run.rs:430), shallow calls should fall back to running
   inline.
10. Option errors are worded in JSON terms ("object"). Upstream accepts tuples and sets in
    `ignores` values; ours raises TypeError.
11. Unknown `args` keys are silently ignored, but `targets` rejects unknown keys.
12. Every string conversion error is reported as "lone surrogate" (convert.rs:200).
13. `#![allow(unsafe_code)]` applies to the whole crate (lib.rs:7). Narrow it.

Docs: correct F-040 (the json-equivalence and cycle claims) and F-041 (panics, minimum stack,
timings). Drop F-043's third bullet, which is already fixed.
