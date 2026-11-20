"""U06: the terminal pretty-printer for diffs and merges (`nbdime.prettyprint`).

Engine functions (each returns the text upstream writes to `config.out`):
- `pp.notebook_diff {afn, bfn, a, di, include?, use_color, color_words, use_git, use_diff}`:
  `pretty_print_notebook_diff(afn, bfn, a, di, config)`.
- `pp.merge_decisions {base, decisions, include?, use_color, use_git, use_diff}`:
  `pretty_print_merge_decisions(base, decisions, config)` (what `nbmerge` prints).
- `pp.value {value, path, prefix, include?, use_color}`: `pretty_print_value_at(value, path,
  prefix, config)`, the building block that exercises `pprint.pformat`, base64 snipping and the
  cell/output/attachment formatters on arbitrary JSON.

`include` maps the six `diff_ignorables` (sources, outputs, attachments, metadata, id, details)
to booleans; absent means all true. Multi-line string diffs are rendered by spawning
`git diff --no-index` (or `diff`, or difflib, depending on `use_git`/`use_diff` and what is
installed), exactly as upstream does (findings F-022); the session pins git's configuration in
`conftest.py`. `a` and `base` are notebooks as `nbformat.read` returns them; `di` and
`decisions` come from the (fixed) upstream differ and merger, so this suite does not depend on
U04/U10 parity.
"""

import copy
import io
import logging
import os
from types import SimpleNamespace

import nbformat

from nbdime import prettyprint as pp
from nbdime.diff_utils import to_diffentry_dicts

from gen import mutate_json, random_json, random_string, rng_for
from gen_nb import random_cell, random_output
from harness import ROOT, check_parity, scaled
from test_u04_notebook_diff import flagged_pairs, o_diff_notebooks, random_pairs, upstream_pairs
from test_u10_merge import _decisions_in, family_triples, notebook_cases, o_decide_notebook_merge

logging.getLogger("nbdime").setLevel(logging.CRITICAL)

IGNORABLES = ("sources", "outputs", "attachments", "metadata", "id", "details")


def _config(include, use_color=True, color_words=False, use_git=True, use_diff=True):
    out = io.StringIO()
    inc = None if include is None else SimpleNamespace(**include)
    config = pp.PrettyPrintConfig(out=out, include=inc, color_words=color_words, use_git=use_git,
                                  use_diff=use_diff, use_color=use_color)
    return config, out


def _node(v):
    return nbformat.from_dict(v) if isinstance(v, (dict, list)) else v


def o_notebook_diff(afn, bfn, a, di, include=None, use_color=True, color_words=False, use_git=True,
                    use_diff=True):
    config, out = _config(include, use_color, color_words, use_git, use_diff)
    pp.pretty_print_notebook_diff(afn, bfn, _node(a), to_diffentry_dicts(di), config)
    return out.getvalue()


def o_merge_decisions(base, decisions, include=None, use_color=True, use_git=True, use_diff=True):
    config, out = _config(include, use_color, False, use_git, use_diff)
    pp.pretty_print_merge_decisions(_node(base), _decisions_in(decisions), config)
    return out.getvalue()


def o_value(value, path, prefix, include=None, use_color=True):
    config, out = _config(include, use_color)
    pp.pretty_print_value_at(_node(value), path, prefix, config)
    return out.getvalue()


# ---------------------------------------------------------------------------- case sources

MODES = [
    {"use_color": True, "color_words": False, "use_git": True, "use_diff": True},
    {"use_color": False, "color_words": False, "use_git": True, "use_diff": True},
    {"use_color": True, "color_words": True, "use_git": True, "use_diff": True},
    {"use_color": True, "color_words": False, "use_git": False, "use_diff": True},
    {"use_color": True, "color_words": False, "use_git": False, "use_diff": False},
    {"use_color": False, "color_words": False, "use_git": False, "use_diff": False},
]
MODE_WEIGHTS = [5, 2, 2, 1, 1, 1]

# Real files, so the header timestamps (local time, `datetime.isoformat(" ")`) are exercised.
STAMPED = ROOT / "parity" / "data" / "stamped.ipynb"


def _stamped_file():
    if not STAMPED.exists():
        STAMPED.write_text("{}\n")
    os.utime(STAMPED, (1700000000.25, 1700000000.25))
    return str(STAMPED)


def _filenames(rng):
    r = rng.random()
    if r < 0.7:
        return "a.ipynb", "b.ipynb"
    if r < 0.85:
        return "dir with space/ä.ipynb", "-"
    return _stamped_file(), "missing/" + random_string(rng, max_len=6) + ".ipynb"


def _include(rng):
    if rng.random() < 0.6:
        return None
    return {k: rng.random() < 0.7 for k in IGNORABLES if rng.random() < 0.8}


def _diff_case(rng, pair, mode=None):
    a = pair["a"]
    di = o_diff_notebooks(a, pair["b"], pair.get("ignores"), pair.get("targets"))
    di = to_plain(di)
    afn, bfn = _filenames(rng)
    m = mode or rng.choices(MODES, MODE_WEIGHTS)[0]
    case = dict(m, afn=afn, bfn=bfn, a=a, di=di)
    inc = _include(rng)
    if inc is not None:
        case["include"] = inc
    return case


def to_plain(v):
    import json
    return json.loads(json.dumps(v, allow_nan=True))


def diff_cases(name, pairs):
    rng = rng_for(name)
    return [_diff_case(rng, p) for p in pairs]


def merge_cases(name, triples):
    rng = rng_for(name)
    cases = []
    for t in triples:
        try:
            decisions = to_plain(o_decide_notebook_merge(**t))
        except Exception:
            continue
        m = rng.choices(MODES, MODE_WEIGHTS)[0]
        case = {"base": t["base"], "decisions": decisions, "use_color": m["use_color"],
                "use_git": m["use_git"], "use_diff": m["use_diff"]}
        inc = _include(rng)
        if inc is not None:
            case["include"] = inc
        cases.append(case)
    return cases


PATHS = [None, "/metadata", "/metadata/x", "/cells/0/metadata", "/cells/3/source", "/x/y",
         "/nbformat", "/cells/0/outputs/0/data", "/cells/0/outputs/0/metadata"]


def _long_values(rng):
    b64 = "".join(rng.choice("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/")
                  for _ in range(rng.choice([60, 64, 68, 72, 400])))
    return [
        b64, b64 + "\n", b64[:-2] + "==", "\n".join(b64[i:i + 76] for i in range(0, len(b64), 76)),
        "x" * rng.randint(60, 200), "word " * rng.randint(10, 40), ["line\n"] * rng.randint(1, 8),
        {"k%d" % i: "v" * rng.randint(0, 30) for i in range(rng.randint(1, 8))},
        [rng.randint(-10**20, 10**20) for _ in range(rng.randint(1, 30))],
        [1.5e300, -0.0, 1e-7, 123456789.0, float("inf"), float("nan")],
        "quote's \"both\"", "tab\there", "uni é中\U0001F600 ​  \x7f\x85", "\\n literal",
        [[], {}, "", None, True, False], {"nested": {"deeper": [{"a": [1, [2, [3]]]}]}},
    ]


def value_cases(name, n):
    rng = rng_for(name)
    cases = []
    for _ in range(n):
        r = rng.random()
        if r < 0.15:
            path = "/cells/%d" % rng.randint(0, 3)
            value = random_cell(rng, rng.random() < 0.5, rng.randint(1, 6))
        elif r < 0.2:
            path = "/cells"
            value = [random_cell(rng, True, 2) for _ in range(rng.randint(0, 3))]
        elif r < 0.3:
            path = "/cells/1/outputs/%d" % rng.randint(0, 2)
            value = random_output(rng, rng.randint(1, 6))
        elif r < 0.35:
            path = "/cells/1/outputs"
            value = [random_output(rng, 2) for _ in range(rng.randint(0, 3))]
        elif r < 0.4:
            path = "/cells/2/attachments"
            value = {"f%d.png" % i: {"image/png": rng.choice(_long_values(rng)[:4])} for i in range(rng.randint(1, 3))}
        elif r < 0.7:
            path = rng.choice(PATHS)
            value = rng.choice(_long_values(rng))
        else:
            path = rng.choice(PATHS)
            value = random_json(rng)
            if rng.random() < 0.3:
                value = mutate_json(rng, value)
        prefix = rng.choice(["", "   ", "+  ", "-  ", "\x1b[32m+  ", "\x1b[31m-  ", "  ", "## "])
        case = {"value": value, "path": path, "prefix": prefix, "use_color": rng.random() < 0.7}
        inc = _include(rng)
        if inc is not None:
            case["include"] = inc
        cases.append(case)
    return cases


# ---------------------------------------------------------------------------- tests


def test_u06_diff_upstream_families(engine):
    cases = diff_cases("u06-families", upstream_pairs()) + diff_cases("u06-families-2", upstream_pairs())
    check_parity(engine, "pp.notebook_diff", cases, o_notebook_diff, min_cases=400)


def test_u06_diff_upstream_families_all_modes(engine):
    rng = rng_for("u06-modes")
    pairs = upstream_pairs()
    cases = [_diff_case(rng, p, m) for p in pairs for m in MODES]
    check_parity(engine, "pp.notebook_diff", cases, o_notebook_diff, min_cases=1200)


def test_u06_diff_random(engine):
    cases = diff_cases("u06-random", random_pairs("u06-random-pairs", scaled(1500)))
    check_parity(engine, "pp.notebook_diff", cases, o_notebook_diff, min_cases=scaled(1500))


def test_u06_diff_flagged(engine):
    cases = diff_cases("u06-flagged", flagged_pairs("u06-flagged-pairs", scaled(800)))
    check_parity(engine, "pp.notebook_diff", cases, o_notebook_diff, min_cases=scaled(800))


def test_u06_diff_empty_and_errors(engine):
    nb = upstream_pairs()[0]["a"]
    cases = [
        {"afn": "a", "bfn": "b", "a": nb, "di": [], "use_color": True, "color_words": False,
         "use_git": True, "use_diff": True},
        {"afn": "a", "bfn": "b", "a": nb, "di": [{"op": "patch", "key": "nbformat", "diff": []}],
         "use_color": True, "color_words": False, "use_git": True, "use_diff": True},
        {"afn": "a", "bfn": "b", "a": nb, "di": [{"op": "replace", "key": "nbformat", "value": "4"}],
         "use_color": False, "color_words": False, "use_git": True, "use_diff": True},
        {"afn": "a", "bfn": "b", "a": nb, "di": [{"op": "add", "key": "zzz", "value": {"q": [1, 2]}}],
         "use_color": False, "color_words": False, "use_git": True, "use_diff": True},
    ]
    check_parity(engine, "pp.notebook_diff", cases, o_notebook_diff, min_cases=len(cases),
                 max_error_fraction=1.0)


def test_u06_merge_decisions_families(engine):
    triples = family_triples()
    cases = merge_cases("u06-merge-families", triples) + merge_cases("u06-merge-families-2", triples)
    check_parity(engine, "pp.merge_decisions", cases, o_merge_decisions, min_cases=300)


def test_u06_merge_decisions_random(engine):
    cases = merge_cases("u06-merge-random", notebook_cases("u06-merge-random-triples", scaled(800)))
    check_parity(engine, "pp.merge_decisions", cases, o_merge_decisions, min_cases=scaled(700))


def test_u06_values(engine):
    cases = value_cases("u06-values", scaled(6000))
    check_parity(engine, "pp.value", cases, o_value, min_cases=scaled(6000))


def test_u06_pformat_widths(engine):
    """pprint wraps containers at width 80 and splits long strings inside them."""
    cases = []
    for n in range(1, 30):
        for prefix in ("", "+  ", "      "):
            cases.append({"value": ["ab" * n] * 3, "path": None, "prefix": prefix, "use_color": False})
            cases.append({"value": {"key": "x" * (n * 3), "k2": list(range(n))}, "path": "/metadata",
                          "prefix": prefix, "use_color": False})
            cases.append({"value": [list(range(n)), "s" * (5 * n), {"z": n}], "path": None,
                          "prefix": prefix, "use_color": False})
            cases.append({"value": ["one two three four five six " * (n // 3 + 1)], "path": None,
                          "prefix": prefix, "use_color": False})
            cases.append({"value": [n * 10 ** 15, -1.0 / n, True, None], "path": None,
                          "prefix": prefix, "use_color": False})
    check_parity(engine, "pp.value", cases, o_value, min_cases=400)
