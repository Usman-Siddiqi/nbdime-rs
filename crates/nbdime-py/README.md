# nbdime-rs

[nbdime-rs](https://github.com/Usman-Siddiqi/nbdime-rs) is a Rust port of
[nbdime](https://github.com/jupyter/nbdime) 4.0.4. It diffs and merges Jupyter notebooks from
the terminal and from git, with the same output as nbdime and much faster.

```sh
pip install nbdime-rs
nbdiff before.ipynb after.ipynb
nbdime config-git --enable --global   # git diff and git merge understand notebooks
```

The wheel installs native `nbdiff`, `nbmerge`, `nbshow`, `nbpatch` and `nbdime` commands, plus
the git drivers and tools (`git-nbdiffdriver`, `git-nbmergedriver`, `git-nbdifftool`,
`git-nbmergetool`). They take the same options as nbdime's. The web tools (`nbdiff-web` and
friends) are not included. Installing nbdime-rs in the same environment as nbdime replaces
nbdime's commands of the same names.

It also installs the `nbdime_rs` Python module, the diff and merge engine:

```python
import nbdime_rs

diff = nbdime_rs.diff_notebooks(a, b)          # a and b are notebook dicts
merged, decisions = nbdime_rs.merge_notebooks(base, local, remote)
```

Arguments and results are plain `dict`, `list`, `str`, `int`, `float`, `bool` and `None`
(subclasses such as `nbformat.NotebookNode` are accepted); anything else raises `TypeError`.
Engine failures raise `nbdime_rs.EngineError`. The GIL is released while the engine runs.

Wheels are published for Linux (x86-64 and arm64, glibc 2.17 or later), macOS (Intel and Apple
silicon) and Windows (x86-64), for CPython 3.9 and later. On other platforms pip builds the
source distribution, which needs Rust and contains the module only. In that case, install the
commands with `cargo install --git https://github.com/Usman-Siddiqi/nbdime-rs nbdime-cli`.
