#!/usr/bin/env python3
"""Smoke test of an installed `nbdime-rs` wheel: run it with the environment's Python.

Checks that every command is on PATH next to this interpreter and works, that git picks up the
drivers after `nbdime config-git`, and that the `nbdime_rs` module imports and diffs.
"""

import json
import re
import os
import shutil
import subprocess
import sys
import sysconfig
import tempfile
from pathlib import Path

COMMANDS = ["nbdime", "nbdiff", "nbmerge", "nbshow", "nbpatch", "git-nbdiffdriver",
            "git-nbmergedriver", "git-nbdifftool", "git-nbmergetool"]


def notebook(*sources):
    return {"cells": [{"cell_type": "code", "execution_count": None, "id": "c%d" % i,
                       "metadata": {}, "outputs": [], "source": s}
                      for i, s in enumerate(sources)],
            "metadata": {"language_info": {"name": "python"}}, "nbformat": 4, "nbformat_minor": 5}


def plain(text):
    return re.sub(r"\x1b\[[0-9;]*m", "", text)


def run(*cmd, cwd=None, ok=(0,)):
    p = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, encoding="utf-8")
    if p.returncode not in ok:
        sys.exit("%s exited %d\n%s%s" % (" ".join(cmd), p.returncode, p.stdout, p.stderr))
    return p.stdout


def main():
    scripts = Path(sysconfig.get_path("scripts"))
    os.environ["PATH"] = str(scripts) + os.pathsep + os.environ["PATH"]
    for name in COMMANDS:
        found = shutil.which(name)
        assert found and Path(found).parent == scripts, (name, found)
        # `nbdime --help` prints usage and exits 1, as upstream does.
        run(name, "--help", ok=(0, 1) if name == "nbdime" else (0,))
    assert "nbdime 4.0.4" in run("nbdiff", "--version"), "unexpected version"

    import nbdime_rs
    assert nbdime_rs.NBDIME_COMPAT == "4.0.4"
    assert nbdime_rs.diff_notebooks(notebook("x = 1\n"), notebook("x = 2\n"))

    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        files = {}
        for name, nb in {"base": notebook("x = 1\n", "y = 1\n"),
                         "local": notebook("x = 2\n", "y = 1\n"),
                         "remote": notebook("x = 1\n", "y = 2\n")}.items():
            files[name] = str(tmp / (name + ".ipynb"))
            Path(files[name]).write_text(json.dumps(nb), encoding="utf-8")
        out = run("nbdiff", "--no-color", files["base"], files["local"])
        assert "modified /cells/0/source" in out, out
        merged = tmp / "merged.ipynb"
        run("nbmerge", files["base"], files["local"], files["remote"], "--out", str(merged))
        sources = ["".join(c["source"]) for c in json.loads(merged.read_text(encoding="utf-8"))["cells"]]
        assert sources == ["x = 2\n", "y = 2\n"], sources
        assert "x = 2" in plain(run("nbshow", str(merged)))
        patch = tmp / "patch.json"
        run("nbdiff", "--out", str(patch), files["base"], files["local"])
        assert "x = 2" in plain(run("nbpatch", files["base"], str(patch)))

        repo = tmp / "repo"
        repo.mkdir()
        git = ["git", "-c", "user.name=t", "-c", "user.email=t@example.com"]
        run("git", "init", "-q", cwd=repo)
        run("nbdime", "config-git", "--enable", cwd=repo)
        shutil.copy(files["base"], repo / "nb.ipynb")
        run(*git, "add", "nb.ipynb", cwd=repo)
        run(*git, "commit", "-q", "-m", "base", cwd=repo)
        shutil.copy(files["local"], repo / "nb.ipynb")
        out = run("git", "diff", "--no-color", cwd=repo)
        assert "modified /cells/0/source" in out, out
    print("nbdime-rs wheel OK:", sys.version.split()[0], sys.platform)


if __name__ == "__main__":
    main()
