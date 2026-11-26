# Optional Rust engine for notebook diffs and merges

nbdime aligns notebook cells and outputs with configurable matching rules. That work can dominate
runtime when notebooks are large or nearly identical. `nbdime-rs` implements those rules in Rust
and exposes them through a Python extension, `nbdime_rs`.

We propose letting nbdime use the extension for notebook diffs and merge decisions when a
compatible wheel is installed. The existing Python code handles configurations and inputs that
the extension cannot represent. nbdime would continue to install as a pure Python package. The
proposed `accel` extra would install the optional wheel, and `NBDIME_ACCEL=0` would disable it
before the native module is imported.

The prototype tracks changes made by the notebook ignore and target setters, including changes
to caller-owned lists and sets. It keeps custom predicates and differs on the Python path. It
restores `DiffEntry`, `MergeDecision`, tuple paths and `NotebookNode` values at the Python
boundary. Mixed container types, output order that relies on Python's in-place behavior and
custom merge payloads with ambiguous types also use Python.

In one local CPython 3.13 run, the median in-process diff for an 800-cell notebook with stable
cell IDs and source edits in 43 cells was 227 ms in Python and 13 ms with the extension. The
result was identical in both modes. This measures a source-edit workload that can use the
extension. Notebook reads, CLI startup and cases that fall back have different costs.

The compatibility target is nbdime 4.0.4. The Rust project records intentional behavior
differences, including cases where the current Python implementation raises an error on an
otherwise valid notebook. Before enabling the engine for users, we would agree which of those
cases should keep the Python behavior and which belong in separate fixes. The prototype has a
verified x86-64 manylinux wheel. The additional platform wheels would need their own build and
test coverage before offering the extra on those platforms.
