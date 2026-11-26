# Optional Rust engine for notebook diffs and merges

nbdime aligns notebook cells and outputs with configurable matching rules. That work can dominate
runtime when notebooks are large or nearly identical. `nbdime-rs` implements those rules in Rust
and exposes them through a Python extension, `nbdime_rs`.

We propose letting nbdime use the extension for notebook diffs and merge decisions when a
compatible wheel is installed. The existing Python code handles configurations and inputs that
the extension cannot represent. nbdime would continue to install as a pure Python package. The
proposed `accel` extra would install the optional wheel. `NBDIME_ACCEL` has three values: unset
or `1` uses the extension when a compatible one imports, `0` disables it before the native module
is imported, and `force` raises an error instead of falling back when the extension is missing,
incompatible or fails, so CI can prove that the engine runs. When the extension reports an error,
the default mode logs it at debug level and uses the Python implementation.

The prototype tracks changes made by the notebook ignore and target setters, including changes
to caller-owned lists and sets. It keeps custom predicates and differs on the Python path. It
restores `DiffEntry`, `MergeDecision`, tuple paths and `NotebookNode` values at the Python
boundary. Mixed container types, output order that relies on Python's in-place behavior and
custom merge payloads with ambiguous types also use Python.

Measured in-process on CPython 3.13, using generated notebooks of 100 to 1600 code cells with
stable cell IDs and source edits in about 5% of the cells, the median of seven diffs went from
2.8 ms to 0.7 ms (100 cells, 4x), from 135 ms to 5.2 ms (800 cells, 26x) and from 584 ms to
10.4 ms (1600 cells, 56x). The result was identical in both modes. The speedup grows with
notebook size because the Python cell alignment is superlinear. These notebooks are a
source-edit workload that can use the extension. Notebook reads, CLI startup and cases that fall
back to Python have different costs, and the figures came from a machine that was running other
jobs.

The compatibility target is nbdime 4.0.4. The Rust project records intentional behavior
differences, including cases where the current Python implementation raises an error on an
otherwise valid notebook. Before enabling the engine for users, we would agree which of those
cases should keep the Python behavior and which belong in separate fixes. The prototype has a
verified x86-64 manylinux wheel. The additional platform wheels would need their own build and
test coverage before offering the extra on those platforms.
