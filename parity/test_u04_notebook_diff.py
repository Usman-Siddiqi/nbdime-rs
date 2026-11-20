"""U04: notebook diff (`nbdime.diffing.notebooks`), including the predicates and diff targets.

`diff_notebooks {a, b, ignores?, targets?}` diffs two in-memory notebooks (as nbformat.reads
returns them). `ignores` is applied first with `set_notebook_diff_ignores`, then `targets` with
`set_notebook_diff_targets(**targets)`, which is the order nbdiff applies config-file ignores
and command-line flags in. Both start from `reset_notebook_differ()`.

`notebook.compare {pred, x, y}` evaluates one predicate with the default configuration.

The oracle is upstream with three fixes, each shown to be neutral where upstream does not
misbehave:
- F-001: predicate lookups do not register paths (no spurious RuntimeError).
- F-014: `diff_single_outputs` does not reorder the caller's output dicts (upstream pops and
  re-adds `data`, which moves it to the end of outputs that later appear as values in the diff).
- F-019: `add_mime_diff` does not crash on mime values the generic diff cannot recurse into
  (an `application/json` number, bool or null, or values of different kinds); they are replaced.
"""

import contextlib
import copy
import json
import operator

import nbformat
from nbformat import NotebookNode

from nbdime.diffing import generic as nb_generic
from nbdime.diffing import notebooks as nbm

from gen import rng_for
from gen_nb import mutate_cell, mutate_notebook, mutate_output, random_cell, random_notebook, random_output
from harness import _NonRecordingDefaultDict, canon, check_parity, scaled, upstream_test_notebooks


class _NonRecordingDefaultDict2(dict):
    """defaultdict2 whose lookups of missing keys do not insert them (F-001)."""

    def __init__(self, factory, default_values):
        super().__init__()
        self.default_factory = factory
        self.default_values = default_values

    def __missing__(self, key):
        if key in self.default_values:
            return self.default_values[key]
        return self.default_factory()

    def copy(self):
        c = _NonRecordingDefaultDict2(self.default_factory, self.default_values.copy())
        c.update(self)
        return c


def _non_mutating(differ):
    def diff_single_outputs(a, b, path="/cells/*/outputs/*", config=None):
        return differ(NotebookNode(a), NotebookNode(b), path=path, config=config)
    return diff_single_outputs


_DSO = nbm.notebook_differs.default_values["/cells/*/outputs/*"]
_ADD_MIME_DIFF = nbm.add_mime_diff


def _add_mime_diff_fixed(key, avalue, bvalue, diffbuilder):
    """add_mime_diff without the F-019 crash: values the generic diff cannot recurse into
    (a JSON scalar, or two values of different kinds) are replaced when they differ."""
    kinds = (list, dict, str)
    if not any(isinstance(avalue, k) and isinstance(bvalue, k) for k in kinds):
        if avalue != bvalue:
            diffbuilder.replace(key, bvalue)
        return
    _ADD_MIME_DIFF(key, avalue, bvalue, diffbuilder)


@contextlib.contextmanager
def upstream(fix_f014=True, fix_f019=True):
    saved_predicates = nbm.notebook_config.predicates
    saved_default_predicates = nb_generic.default_predicates
    nbm.notebook_config.predicates = _NonRecordingDefaultDict2(
        lambda: [operator.__eq__], nbm.notebook_predicates.default_values)
    nb_generic.default_predicates = lambda: _NonRecordingDefaultDict(lambda: (operator.__eq__,))
    if fix_f014:
        nbm.notebook_differs.default_values["/cells/*/outputs/*"] = _non_mutating(_DSO)
    if fix_f019:
        nbm.add_mime_diff = _add_mime_diff_fixed
    nbm.reset_notebook_differ()
    try:
        yield
    finally:
        nbm.notebook_config.predicates = saved_predicates
        nb_generic.default_predicates = saved_default_predicates
        nbm.notebook_differs.default_values["/cells/*/outputs/*"] = _DSO
        nbm.add_mime_diff = _ADD_MIME_DIFF
        nbm.reset_notebook_differ()


def _as_node(v):
    return nbformat.from_dict(v) if isinstance(v, dict) else v


def o_diff_notebooks(a, b, ignores=None, targets=None, fix_f014=True, fix_f019=True):
    with upstream(fix_f014, fix_f019):
        if ignores is not None:
            nbm.set_notebook_diff_ignores(ignores)
        if targets is not None:
            nbm.set_notebook_diff_targets(**targets)
        return nbm.diff_notebooks(_as_node(a), _as_node(b))


PREDICATES = {
    "cell_approximate": nbm.compare_cell_approximate,
    "cell_moderate": nbm.compare_cell_moderate,
    "cell_strict": nbm.compare_cell_strict,
    "cell_by_ids": nbm.compare_cell_by_ids,
    "output_approximate": nbm.compare_output_approximate,
    "output_strict": nbm.compare_output_strict,
}


def o_compare(pred, x, y):
    with upstream():
        return PREDICATES[pred](_as_node(x), _as_node(y))


# ---------------------------------------------------------------------------- case sources


def read_without_id_repair(raw):
    """nbformat's read without validation, so missing ids stay missing (no randomness)."""
    return json.loads(json.dumps(nbformat.reader.reads(json.dumps(raw))))


def upstream_pairs():
    fams = {}
    for name, raw in upstream_test_notebooks():
        fam = name.split("--")[0]
        if fam.startswith("multilevel-test"):
            fam = "multilevel-test"
        fams.setdefault(fam, []).append(read_without_id_repair(raw))
    cases = []
    for nbs in fams.values():
        for a in nbs:
            for b in nbs:
                cases.append({"a": a, "b": b})
    return cases


def random_pairs(name, n, max_cells=20):
    rng = rng_for(name)
    cases = []
    for _ in range(n):
        size = rng.choice([0, 1, 2, 3, 5, 8, 12, max_cells])
        base = random_notebook(rng, n_cells=size)
        r = rng.random()
        if r < 0.75:
            other = mutate_notebook(rng, base, intensity=rng.choice([0, 1, 1, 2, 3, 5, 8]))
        elif r < 0.9:
            other = random_notebook(rng, n_cells=size, ids="id" in (base["cells"] or [{}])[0])
        else:
            other = copy.deepcopy(base)
            for c in other["cells"]:
                c.pop("id", None)
            other["nbformat_minor"] = 4
        a, b = (base, other) if rng.random() < 0.5 else (other, base)
        cases.append({"a": a, "b": b})
    return cases


TARGET_NAMES = ["sources", "outputs", "attachments", "metadata", "identifier", "details"]

IGNORE_SAMPLES = [
    {"/cells/*/metadata": True},
    {"/metadata": True},
    {"/metadata": ["language_info"]},
    {"/cells/*": ["execution_count"]},
    {"/cells/*/outputs/*": ["execution_count"]},
    {"/cells/*/outputs/*/metadata": True},
    {"/cells/*/outputs": True},
    {"/cells/*/source": True, "/cells/*/attachments": True},
    {"/cells/*/id": True},
    {"/cells/*/outputs": False},
    {"/cells/*": ["metadata", "execution_count"], "/metadata": ["kernelspec"]},
    {"/metadata/language_info": ["version"]},
]


def random_targets(rng):
    if rng.random() < 0.5:
        on = set(rng.sample(TARGET_NAMES, rng.randint(1, 3)))
        return {k: k in on for k in TARGET_NAMES}
    off = set(rng.sample(TARGET_NAMES, rng.randint(1, 3)))
    return {k: k not in off for k in TARGET_NAMES}


def flagged_pairs(name, n):
    rng = rng_for(name)
    cases = random_pairs(name + "-pairs", n, max_cells=12)
    for case in cases:
        r = rng.random()
        if r < 0.4:
            case["targets"] = random_targets(rng)
        elif r < 0.8:
            case["ignores"] = copy.deepcopy(rng.choice(IGNORE_SAMPLES))
        else:
            case["ignores"] = copy.deepcopy(rng.choice(IGNORE_SAMPLES))
            case["targets"] = random_targets(rng)
    return cases


def structural_cases():
    nb = {"cells": [], "metadata": {}, "nbformat": 4, "nbformat_minor": 4}
    code = {"cell_type": "code", "execution_count": 1, "metadata": {}, "outputs": [], "source": "x"}
    cases = [
        {"a": nb, "b": nb},
        {"a": nb, "b": {"metadata": {}, "nbformat": 4, "nbformat_minor": 4}},
        {"a": dict(nb, cells=[code]), "b": dict(nb, cells=[dict(code, source="y"), code])},
        {"a": dict(nb, metadata={"k": [1, {"x": 1}]}), "b": dict(nb, metadata={"k": [1, {"x": True}]})},
        {"a": dict(nb, metadata={"k": [[1], {"x": 1}]}), "b": dict(nb, metadata={"k": [[1], {"x": True}]})},
        {"a": dict(nb, metadata={"x": [1, 2]}), "b": dict(nb, metadata={"x": {"a": 1}})},
        {"a": dict(nb, cells=[dict(code, outputs=[{"output_type": "stream", "name": "stdout", "text": "a",
                                                   "metadata": {"m": 1}, "cells": [1, 2]}])]),
         "b": dict(nb, cells=[dict(code, outputs=[{"output_type": "stream", "name": "stdout", "text": "a",
                                                   "metadata": {"m": 2}, "cells": [2, 1]}])])},
    ]
    stream_meta = cases[-1]
    cases.append(dict(stream_meta, ignores={"/metadata": True}))
    cases.append(dict(stream_meta, targets={k: k != "metadata" for k in TARGET_NAMES}))
    return cases


def error_cases():
    nb = {"cells": [], "metadata": {}, "nbformat": 4, "nbformat_minor": 4}
    no_source = {"cell_type": "code", "execution_count": 1, "metadata": {}, "outputs": []}
    code = dict(no_source, source="x = 1\n")
    return [
        {"a": [], "b": nb},
        {"a": nb, "b": "x"},
        {"a": dict(nb, cells=[code]), "b": dict(nb, cells=[no_source])},
        {"a": nb, "b": nb, "ignores": {"/cells": "yes"}},
    ]


# ---------------------------------------------------------------------------- tests


def test_u04_upstream_families(engine):
    cases = upstream_pairs()
    check_parity(engine, "diff_notebooks", cases, o_diff_notebooks, min_cases=150)


def test_u04_random_pairs(engine):
    cases = random_pairs("u04-random", scaled(5000))
    check_parity(engine, "diff_notebooks", cases, o_diff_notebooks, min_cases=scaled(5000))


def test_u04_larger_pairs(engine):
    cases = random_pairs("u04-larger", scaled(300), max_cells=60)
    check_parity(engine, "diff_notebooks", cases, o_diff_notebooks, min_cases=scaled(300))


def test_u04_targets_and_ignores(engine):
    cases = flagged_pairs("u04-flags", scaled(2500))
    for case in upstream_pairs()[:200]:
        for t in TARGET_NAMES:
            cases.append(dict(case, targets={k: k != t for k in TARGET_NAMES}))
    check_parity(engine, "diff_notebooks", cases, o_diff_notebooks, min_cases=scaled(2500))


def test_u04_structural(engine):
    check_parity(engine, "diff_notebooks", structural_cases(), o_diff_notebooks, min_cases=9)


def test_u04_errors(engine):
    check_parity(engine, "diff_notebooks", error_cases(), o_diff_notebooks, min_cases=4,
                 max_error_fraction=1.0)


def predicate_cases(name, n):
    rng = rng_for(name)
    cases = []
    for _ in range(n):
        ids = rng.random() < 0.5
        if rng.random() < 0.6:
            x = random_cell(rng, ids, rng.randint(1, 9))
            r = rng.random()
            y = mutate_cell(rng, x) if r < 0.6 else (copy.deepcopy(x) if r < 0.7 else random_cell(rng, ids, 3))
            pred = rng.choice(["cell_approximate", "cell_moderate", "cell_strict", "cell_by_ids"])
        else:
            x = random_output(rng, rng.randint(1, 9))
            r = rng.random()
            y = mutate_output(rng, x) if r < 0.6 else (copy.deepcopy(x) if r < 0.7 else random_output(rng, 2))
            pred = rng.choice(["output_approximate", "output_strict"])
        if rng.random() < 0.5:
            x, y = y, x
        cases.append({"pred": pred, "x": x, "y": y})
    return cases


def test_u04_predicates(engine):
    cases = predicate_cases("u04-predicates", scaled(8000))
    check_parity(engine, "notebook.compare", cases, o_compare, min_cases=scaled(8000))


def test_u04_predicates_on_upstream_cells(engine):
    cases = []
    for case in upstream_pairs():
        xa, xb = case["a"].get("cells", []), case["b"].get("cells", [])
        for x in xa[:6]:
            for y in xb[:6]:
                for pred in ["cell_approximate", "cell_moderate", "cell_strict", "cell_by_ids"]:
                    cases.append({"pred": pred, "x": x, "y": y})
                for xo in (x.get("outputs") or [])[:2]:
                    for yo in (y.get("outputs") or [])[:2]:
                        for pred in ["output_approximate", "output_strict"]:
                            cases.append({"pred": pred, "x": xo, "y": yo})
    check_parity(engine, "notebook.compare", cases, o_compare, min_cases=2000)


def test_u04_f014_fix_is_neutral():
    """The F-014 oracle fix changes only the key order of output dicts inside the diff."""
    cases = upstream_pairs() + random_pairs("u04-f014", scaled(400)) + flagged_pairs("u04-f014-flags", scaled(200))
    reordered = 0
    for case in cases:
        fixed = o_diff_notebooks(**copy.deepcopy(case))
        raw = o_diff_notebooks(**copy.deepcopy(case), fix_f014=False)
        assert json.dumps(fixed, sort_keys=True) == json.dumps(raw, sort_keys=True), case
        reordered += canon(fixed) != canon(raw)
    assert reordered > 0, "no case exercises F-014"


def _json_mime_cases():
    nb = {"cells": [], "metadata": {}, "nbformat": 4, "nbformat_minor": 4}

    def with_data(data, att=None):
        cell = {"cell_type": "code", "execution_count": 1, "metadata": {}, "source": "x",
                "outputs": [{"data": dict(data, **{"text/plain": "x"}), "metadata": {}, "output_type": "display_data"}]}
        cells = [cell]
        if att is not None:
            cells.append({"cell_type": "markdown", "metadata": {}, "source": "![a](attachment:a.json)",
                          "attachments": {"a.json": att}})
        return dict(nb, cells=cells)

    values = [1, 2, 2.5, True, False, None, "s", [1], {"a": 1}, "t\n"]
    cases = []
    for x in values:
        for y in values:
            cases.append({"a": with_data({"application/json": x}), "b": with_data({"application/json": y})})
            cases.append({"a": with_data({"application/vnd.x+json": x}), "b": with_data({"application/vnd.x+json": y})})
            cases.append({"a": with_data({}, att={"application/json": x}), "b": with_data({}, att={"application/json": y})})
    cases.append({"a": with_data({"text/html": "a"}), "b": with_data({"text/html": ["a", 1]})})
    return cases


def test_u04_f019_json_mime_scalars(engine):
    check_parity(engine, "diff_notebooks", _json_mime_cases(), o_diff_notebooks, min_cases=300)


def test_u04_f019_fix_is_neutral():
    """Where upstream does not raise, the F-019 fix changes nothing; on these cases it raises."""
    raised = 0
    cases = _json_mime_cases() + upstream_pairs() + random_pairs("u04-f019", scaled(300))
    for case in cases:
        try:
            raw = o_diff_notebooks(**copy.deepcopy(case), fix_f019=False)
        except RuntimeError as e:
            raised += "Can currently only diff" in str(e)
            continue
        except Exception:
            continue
        assert canon(o_diff_notebooks(**copy.deepcopy(case))) == canon(raw), case
    assert raised > 0
