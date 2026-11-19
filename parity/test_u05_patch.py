"""U05: patching (`nbdime.patching`) and line-to-character diff flattening (`diff_utils`).

`patch {obj, diff}` returns `nbdime.patch(obj, diff)`, including the key order patch_dict
produces (keys named in the diff first, in diff order, then the untouched keys).
`flatten_list_of_string_diff {a, diff}` returns `diff_utils.flatten_list_of_string_diff`.

Diffs arrive as JSON. Malformed diffs must fail on both sides or agree. Deliberate deviation
(F-015): diff keys must be non-negative integers or strings; upstream also accepts `true`/`false`
and negative integers as list keys (Python slicing), which no nbdime differ produces.
"""

import copy
import json

from nbdime.diff_utils import flatten_list_of_string_diff, to_diffentry_dicts
from nbdime.diffing.sequences import diff_strings_linewise
from nbdime.patching import patch

from gen import json_pair, mutate_lines, multiline_text, rng_for, text_pair
from gen_nb import mutate_notebook, random_notebook
from harness import check_parity, generic_diff_fixed, scaled, upstream_test_notebooks
from test_u04_notebook_diff import o_diff_notebooks, random_pairs, read_without_id_repair, upstream_pairs


def plain(v):
    return json.loads(json.dumps(v, allow_nan=True))


def o_patch(obj, diff):
    return patch(obj, to_diffentry_dicts(diff))


def o_flatten(a, diff):
    return flatten_list_of_string_diff(a, to_diffentry_dicts(diff))


# ---------------------------------------------------------------------------- case sources


def generic_cases(name, n):
    rng = rng_for(name)
    cases = []
    while len(cases) < n:
        a, b = json_pair(rng)
        if not (isinstance(a, (dict, list)) and type(a) is type(b)):
            continue
        try:
            d = generic_diff_fixed(a, b)
        except Exception:
            continue
        cases.append({"obj": plain(a), "diff": plain(d)})
    return cases


def text_dict_cases(name, n):
    rng = rng_for(name)
    cases = []
    for _ in range(n):
        a, b = text_pair(rng)
        x, y = {"s": a, "k": 1, "t": "same"}, {"s": b, "k": rng.choice([1, 2]), "t": "same", "n": None}
        if rng.random() < 0.3:
            x, y = y, x
        cases.append({"obj": x, "diff": plain(generic_diff_fixed(x, y))})
        if a != b:
            cases.append({"obj": a, "diff": plain(diff_strings_linewise(a, b))})
    return cases


def notebook_cases(name, n):
    cases = []
    for c in upstream_pairs() + random_pairs(name, n):
        cases.append({"obj": c["a"], "diff": plain(o_diff_notebooks(c["a"], c["b"]))})
    return cases


def malformed_cases(name, n):
    """Diffs damaged in ways a hand-edited or stale diff file might be."""
    rng = rng_for(name)
    base = [c for c in generic_cases(name + "-src", 4 * n) if c["diff"]]
    out = []
    for c in base[:n]:
        diff = copy.deepcopy(c["diff"])
        e = rng.choice(diff)
        r = rng.random()
        if r < 0.2 and isinstance(e["key"], int):
            e["key"] = rng.choice([e["key"] + 1, e["key"] + 5, 0, 1000])
        elif r < 0.3 and isinstance(e["key"], str):
            e["key"] = e["key"] + "x"
        elif r < 0.45:
            e["op"] = rng.choice(["add", "remove", "replace", "addrange", "removerange", "patch", "bogus"])
        elif r < 0.55:
            for k in ("value", "valuelist", "length", "diff"):
                e.pop(k, None)
        elif r < 0.65:
            diff.reverse()
        elif r < 0.75:
            diff.append(copy.deepcopy(e))
        elif r < 0.85 and e["op"] == "removerange":
            e["length"] = rng.choice([0, e["length"] + 3, 100])
        else:
            e["key"] = 0 if isinstance(e["key"], str) else "k"
        out.append({"obj": c["obj"], "diff": diff})
    return out


def flatten_cases(name, n):
    rng = rng_for(name)
    cases = []
    for _ in range(n):
        a = multiline_text(rng)
        b = mutate_lines(rng, a)
        if a == b:
            continue
        d = plain(diff_strings_linewise(a, b))
        cases.append({"a": a, "diff": d})
        if rng.random() < 0.3:
            cases.append({"a": a.splitlines(True), "diff": d})
    return cases


# ---------------------------------------------------------------------------- tests


def test_u05_patch_generic(engine):
    cases = generic_cases("u05-generic", scaled(8000))
    check_parity(engine, "patch", cases, o_patch, min_cases=scaled(8000))


def test_u05_patch_strings(engine):
    cases = text_dict_cases("u05-strings", scaled(4000))
    check_parity(engine, "patch", cases, o_patch, min_cases=scaled(4000))


def test_u05_patch_notebooks(engine):
    cases = notebook_cases("u05-notebooks", scaled(1500))
    check_parity(engine, "patch", cases, o_patch, min_cases=scaled(1500))


def test_u05_patch_roundtrip(engine):
    """patch(a, diff(a, b)) == b on the upstream families, as plain JSON."""
    cases = notebook_cases("u05-roundtrip", 0)
    results = engine.call_many("patch", cases)
    for case, src, (ok, got) in zip(cases, upstream_pairs(), results):
        assert ok, got
        assert json.dumps(got, sort_keys=True) == json.dumps(src["b"], sort_keys=True)


def test_u05_patch_malformed(engine):
    cases = malformed_cases("u05-malformed", scaled(2000))
    check_parity(engine, "patch", cases, o_patch, min_cases=scaled(2000), max_error_fraction=1.0)


def test_u05_patch_rejects(engine):
    cases = [
        {"obj": 1, "diff": []},
        {"obj": None, "diff": []},
        {"obj": {"a": 1}, "diff": [{"op": "add", "key": "a", "value": 2}]},
        {"obj": {"a": 1}, "diff": [{"op": "remove", "key": "a"}, {"op": "patch", "key": "a", "diff": []}]},
        {"obj": [1], "diff": [{"op": "patch", "key": 3, "diff": []}]},
        {"obj": "abc", "diff": [{"op": "addrange", "key": 5, "valuelist": ["x"]}]},
        {"obj": [1], "diff": [{"op": "frobnicate", "key": 0}]},
        {"obj": {"a": 1}, "diff": [{"op": "replace", "key": 0, "value": 1}]},
    ]
    check_parity(engine, "patch", cases, o_patch, min_cases=len(cases), max_error_fraction=1.0)


def test_u05_flatten(engine):
    cases = flatten_cases("u05-flatten", scaled(5000))
    check_parity(engine, "flatten_list_of_string_diff", cases, o_flatten, min_cases=scaled(4000))


def test_u05_patch_upstream_notebooks_against_mutations(engine):
    cases = []
    rng = rng_for("u05-upstream-mut")
    for _, raw in upstream_test_notebooks():
        nb = read_without_id_repair(raw)
        nb["metadata"].setdefault("language_info", {})
        for _ in range(5):
            other = mutate_notebook(rng, nb) if rng.random() < 0.8 else random_notebook(rng)
            cases.append({"obj": nb, "diff": plain(o_diff_notebooks(copy.deepcopy(nb), other))})
    check_parity(engine, "patch", cases, o_patch, min_cases=150)
