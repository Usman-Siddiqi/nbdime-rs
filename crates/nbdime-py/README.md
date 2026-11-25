# nbdime-rs

The `nbdime_rs` Python extension: the diff and merge engine of
[nbdime-rs](https://github.com/Usman-Siddiqi/nbdime-rs), a Rust port of nbdime 4.0.4.

```python
import nbdime_rs

diff = nbdime_rs.diff_notebooks(a, b)          # a and b are notebook dicts
merged, decisions = nbdime_rs.merge_notebooks(base, local, remote)
```

Arguments and results are plain `dict`, `list`, `str`, `int`, `float`, `bool` and `None`
(subclasses such as `nbformat.NotebookNode` are accepted); anything else raises `TypeError`.
Engine failures raise `nbdime_rs.EngineError`. The GIL is released while the engine runs.
