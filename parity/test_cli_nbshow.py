"""U08: `nbshow` against upstream's console script (same harness as test_cli.py).

nbshow always highlights: markdown cells with Pygments' MarkdownLexer, code cells with the lexer
named by the notebook's `language_info` (F-032: IPython lexers are absent, as in a plain nbdime
install; lexers we do not port are not used here). Python sources are slices of CPython's own
standard library, so the Python lexer sees real code.
"""

import copy
import json
import sysconfig
from pathlib import Path

from gen import rng_for
from gen_nb import dumps_disk, random_notebook
from test_cli import _usable, check_cli, sandbox, upstream_files  # noqa: F401 (fixture)

STDLIB = Path(sysconfig.get_paths()["stdlib"])
STDLIB_FILES = sorted(p for p in STDLIB.glob("*.py"))[:200]
LANGS = [
    {"name": "python", "pygments_lexer": "ipython3"},
    {"name": "python", "pygments_lexer": "python3"},
    {"name": "python"},
    {"name": "python", "pygments_lexer": "py"},
    {"name": "python", "pygments_lexer": "nosuchlexer"},
    {"name": "nosuchlanguage"},
    None,
]
FENCES = ["python", "py", "python3", "", "nosuchlexer", "ipython3"]


def _python_snippet(rng):
    f = rng.choice(STDLIB_FILES)
    lines = f.read_text(encoding="utf-8", errors="replace").splitlines(keepends=True)
    if not lines:
        return "pass\n"
    start = rng.randrange(len(lines))
    return "".join(lines[start:start + rng.choice([1, 3, 10, 40])])


def _markdown(rng):
    parts = []
    for _ in range(rng.randint(1, 4)):
        k = rng.random()
        if k < 0.4:
            parts.append("```%s\n%s```\n" % (rng.choice(FENCES), _python_snippet(rng)))
        elif k < 0.6:
            parts.append("    " + _python_snippet(rng).replace("\n", "\n    ").rstrip(" ") + "\n")
        else:
            parts.append(rng.choice(["# Title\n", "Some *emph* and **strong** and `code`.\n",
                                     "- item\n- [link](http://x.y)\n", "> quote\n", "plain text\n",
                                     "1. one\n2. two\n", "| a | b |\n|---|---|\n"]))
    return "\n".join(parts)


def _nbshow_notebooks(root, n):
    rng = rng_for("cli-nbshow")
    dest = root / "gen-nbshow"
    dest.mkdir(exist_ok=True)
    out = []
    i = 0
    while len(out) < n:
        i += 1
        nb = random_notebook(rng, n_cells=rng.choice([1, 3, 6]))
        nb = copy.deepcopy(nb)
        lang = rng.choice(LANGS)
        if lang is None:
            nb["metadata"].pop("language_info", None)
        else:
            nb["metadata"]["language_info"] = dict(lang)
        for cell in nb["cells"]:
            if cell["cell_type"] == "code" and rng.random() < 0.8:
                cell["source"] = _python_snippet(rng)
            elif cell["cell_type"] == "markdown" and rng.random() < 0.8:
                cell["source"] = _markdown(rng)
        text = dumps_disk(nb, rng)
        if not _usable(text):
            continue
        p = dest / ("%d.ipynb" % i)
        p.write_text(text, encoding="utf-8")
        out.append(str(p))
    return out


FLAG_SETS = [[], ["-s"], ["-o"], ["-O"], ["-a"], ["-m"], ["-i"], ["-d"], ["-S", "-M"], ["-s", "-o"],
             ["--log-level", "DEBUG"]]


def test_cli_nbshow_upstream_files(sandbox):
    root, env = sandbox
    files = [f for fam in upstream_files(root).values() for f in fam]
    cases = []
    for n, f in enumerate(files):
        cases.append({"cmd": "nbshow", "args": FLAG_SETS[n % len(FLAG_SETS)] + [f]})
        cases.append({"cmd": "nbdime show", "args": [f]})
    cases.append({"cmd": "nbshow", "args": files[:3]})
    check_cli(cases, root, env, min_cases=60)


def test_cli_nbshow_generated(sandbox):
    root, env = sandbox
    rng = rng_for("cli-nbshow-flags")
    cases = [{"cmd": "nbshow", "args": rng.choice(FLAG_SETS) + [f]}
             for f in _nbshow_notebooks(root, 150)]
    check_cli(cases, root, env, min_cases=150)


def test_cli_nbshow_special_inputs(sandbox):
    root, env = sandbox
    files = [f for fam in upstream_files(root).values() for f in fam]
    some = files[0]
    empty = root / "nbshow-empty.ipynb"
    empty.write_text("")
    garbage = root / "nbshow-garbage.ipynb"
    garbage.write_text("{not json")
    v3 = root / "nbshow-v3.ipynb"
    v3.write_text(json.dumps({"nbformat": 3, "nbformat_minor": 0, "metadata": {}, "worksheets": []}))
    cases = [
        {"cmd": "nbshow", "args": []},
        {"cmd": "nbshow", "args": ["--help"]},
        {"cmd": "nbdime show", "args": ["--help"]},
        {"cmd": "nbshow", "args": ["-"], "stdin": Path(some).read_bytes()},
        {"cmd": "nbshow", "args": ["-"], "stdin": b""},
        {"cmd": "nbshow", "args": [some, str(root / "missing.ipynb")]},
        {"cmd": "nbshow", "args": [str(empty)]},
        {"cmd": "nbshow", "args": [str(garbage)]},
        {"cmd": "nbshow", "args": [str(v3)]},
        {"cmd": "nbshow", "args": ["-s", "-S", some]},
        {"cmd": "nbshow", "args": ["--bogus", some]},
    ]
    check_cli(cases, root, env, min_cases=len(cases))
