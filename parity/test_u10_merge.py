"""U10: the merge engine (`nbdime.merging`: decisions, chunks, generic, strategies, autoresolve,
notebooks).

Engine functions:
- `decide_merge {base, local, remote, strategies?: {path: name}, transients?: [path], fall_back?}`
- `decide_merge_with_diff {base, local, remote, local_diff, remote_diff, strategies?, transients?, fall_back?}`
- `apply_decisions {base, decisions}`
- `decide_notebook_merge {base, local, remote, args?, ignores?, targets?}` where `args` holds
  `merge_strategy` ("inline"), `input_strategy` (null), `output_strategy` (null),
  `ignore_transients` (true); `ignores`/`targets` configure the notebook differ as in
  test_u04 before the merge diffs are computed.
- `merge_notebooks {same args}` returns `[merged, decisions]`.

The oracle runs inside the same fixed upstream as the notebook diff suite (F-001, F-014, F-019).
Inline source conflicts are rendered by `git merge-file` upstream; the engine must produce the
same text (git 2.43 is installed here).

Deliberate deviation F-021: upstream gives inline-cells conflict marker cells random ids
(`nbformat.v4.new_markdown_cell`); nbdime-rs gives them deterministic ids. Notebook merge results
are compared with marker cell ids masked, and the engine's marker ids must be 8 hex chars and
distinct within one result.

Case sources: every top-level merge call made by upstream's own merge tests (recorded in
parity/data/upstream_merge_calls.jsonl by parity/tools/record_upstream_merge_calls.sh), the
upstream test notebook families, and seeded random JSON and notebook triples.
"""

import contextlib
import copy
import json
import logging
import re
from pathlib import Path
from types import SimpleNamespace

import nbformat

from nbdime.merging import decisions as nb_decisions
from nbdime.merging import generic as nb_generic
from nbdime.merging import notebooks as nb_merge
from nbdime.merging import strategies as nb_strategies
from nbdime.diffing import notebooks as nbm
from nbdime.diff_utils import to_diffentry_dicts
from nbdime.utils import Strategies

from gen import mutate_json, random_json, rng_for
from gen_nb import mutate_notebook, random_notebook
from harness import canon, check_parity, scaled, upstream_test_notebooks
from test_u04_notebook_diff import IGNORE_SAMPLES, random_targets, read_without_id_repair, upstream

logging.getLogger("nbdime").setLevel(logging.CRITICAL)

DATA = Path(__file__).resolve().parent / "data" / "upstream_merge_calls.jsonl"

ARG_DEFAULTS = {"merge_strategy": "inline", "input_strategy": None, "output_strategy": None,
                "ignore_transients": True}


def _plain(v):
    return json.loads(json.dumps(v, allow_nan=True))


def _strategies(strategies, transients, fall_back):
    s = Strategies(strategies or {}, transients=transients or [], fall_back=fall_back)
    return s


def _node(v):
    return nbformat.from_dict(v) if isinstance(v, (dict, list)) else v


def _decisions_in(decisions):
    out = []
    for d in decisions:
        d = nb_decisions.MergeDecision(to_diffentry_dicts(d))
        d["common_path"] = tuple(d["common_path"])
        out.append(d)
    return out


def _resolve_strategy_inline_outputs_fixed(base_path, outputs, decisions):
    """Upstream's resolver, except that a conflict appended at the end of a non-empty outputs
    list has no base output (upstream indexes `outputs[len(outputs)]`, F-027)."""
    S = nb_strategies
    decisions_by_index = S.bundle_decisions_by_index(base_path, decisions)
    decisions.decisions = []
    for key, decs in sorted(decisions_by_index.items()):
        if not any(d.conflict for d in decs):
            decisions.decisions.extend(decs)
        else:
            local_diff, remote_diff = S.collect_diffs(base_path, decs)
            base_output = outputs[key] if outputs and key < len(outputs) else None
            inlined_conflict, keep_base = S.make_inline_output_conflict(base_output, local_diff, remote_diff)
            custom_diff = [S.op_addrange(key, inlined_conflict)]
            if not keep_base:
                custom_diff += [S.op_removerange(key, 1)]
            decisions.custom(base_path, local_diff, remote_diff, custom_diff, conflict=True,
                             strategy="inline-outputs")


@contextlib.contextmanager
def merge_upstream(fix_f027=True):
    """The fixed upstream of the notebook diff suite, plus F-027."""
    with upstream():
        original = nb_strategies.resolve_strategy_inline_outputs
        if fix_f027:
            nb_strategies.resolve_strategy_inline_outputs = _resolve_strategy_inline_outputs_fixed
        try:
            yield
        finally:
            nb_strategies.resolve_strategy_inline_outputs = original


def o_decide_merge(base, local, remote, strategies=None, transients=None, fall_back=None, fix_f027=True):
    with merge_upstream(fix_f027):
        return _plain(nb_generic.decide_merge(_node(base), _node(local), _node(remote),
                                              _strategies(strategies, transients, fall_back)))


def o_decide_merge_with_diff(base, local, remote, local_diff, remote_diff, strategies=None,
                             transients=None, fall_back=None, fix_f027=True):
    with merge_upstream(fix_f027):
        return _plain(nb_generic.decide_merge_with_diff(
            _node(base), _node(local), _node(remote), to_diffentry_dicts(local_diff),
            to_diffentry_dicts(remote_diff), _strategies(strategies, transients, fall_back)))


def o_apply_decisions(base, decisions, fix_f027=True):
    with merge_upstream(fix_f027):
        return _plain(nb_decisions.apply_decisions(_node(base), _decisions_in(decisions)))


def _ns(args):
    if args is None:
        return None
    return SimpleNamespace(log_level="INFO", **dict(ARG_DEFAULTS, **args))


def o_decide_notebook_merge(base, local, remote, args=None, ignores=None, targets=None, fix_f027=True):
    with merge_upstream(fix_f027):
        if ignores is not None:
            nbm.set_notebook_diff_ignores(ignores)
        if targets is not None:
            nbm.set_notebook_diff_targets(**targets)
        return _plain(nb_merge.decide_notebook_merge(_node(base), _node(local), _node(remote), _ns(args)))


def o_merge_notebooks(base, local, remote, args=None, ignores=None, targets=None, fix_f027=True):
    with merge_upstream(fix_f027):
        if ignores is not None:
            nbm.set_notebook_diff_ignores(ignores)
        if targets is not None:
            nbm.set_notebook_diff_targets(**targets)
        merged, decisions = nb_merge.merge_notebooks(_node(base), _node(local), _node(remote), _ns(args))
        return _plain([merged, decisions])


MARKER_PREFIX = '<span style="color:red"><b>'


def _marker_ids(v, out):
    stack = [v]
    while stack:
        x = stack.pop()
        if isinstance(x, dict):
            if (x.get("cell_type") == "markdown" and isinstance(x.get("source"), str)
                    and x["source"].startswith(MARKER_PREFIX) and "id" in x):
                out.append(x)
            stack.extend(x.values())
        elif isinstance(x, list):
            stack.extend(x)
    return out


def _masked(v):
    v = copy.deepcopy(v)
    for cell in _marker_ids(v, []):
        cell["id"] = "<marker>"
    return v


def compare_modulo_marker_ids(expected, got):
    if canon(_masked(expected)) != canon(_masked(got)):
        return False
    ids = [c["id"] for c in _marker_ids(got, [])]
    if not all(isinstance(i, str) and re.match(r"^[0-9a-f]{8}$", i) for i in ids):
        return False
    # The same marker cell object may appear both in a decision and in the merged notebook.
    return True


# ---------------------------------------------------------------------------- case sources


def recorded(fn):
    cases = []
    for line in DATA.read_text(encoding="utf-8").splitlines():
        r = json.loads(line)
        if r["fn"] != fn:
            continue
        a = r["args"]
        if fn in ("decide_merge", "decide_merge_with_diff"):
            case = {k: a[k] for k in ("base", "local", "remote")}
            if fn == "decide_merge_with_diff":
                case["local_diff"], case["remote_diff"] = a["local_diff"], a["remote_diff"]
            if a.get("strategies"):
                case["strategies"] = a["strategies"]
        elif fn == "apply_decisions":
            case = {"base": a["base"], "decisions": a["decisions"]}
        else:
            case = {k: a[k] for k in ("base", "local", "remote")}
            if a.get("args") is not None:
                case["args"] = {k: a["args"][k] for k in ARG_DEFAULTS if k in a["args"]}
        cases.append(case)
    return cases


GENERIC_STRATEGIES = [None, "clear", "remove", "clear-all", "fail", "mergetool", "take-max", "union",
                      "use-base", "use-local", "use-remote"]
STRATEGY_PATHS = ["/", "/*", "/a", "/a/*", "/b", "/*/*", "/c/*/d"]


def _random_strategies(rng):
    if rng.random() < 0.3:
        return None
    out = {}
    for _ in range(rng.randint(1, 3)):
        s = rng.choice(GENERIC_STRATEGIES)
        if s is not None:
            out[rng.choice(STRATEGY_PATHS)] = s
    return out or None


def generic_cases(name, n):
    rng = rng_for(name)
    cases = []
    while len(cases) < n:
        base = random_json(rng)
        if not isinstance(base, (dict, list, str)):
            continue
        if isinstance(base, str) and rng.random() < 0.7:
            continue
        local, remote = mutate_json(rng, base), mutate_json(rng, base)
        if rng.random() < 0.2:
            remote = copy.deepcopy(local)
        if type(local) is not type(base) or type(remote) is not type(base):
            continue
        case = {"base": base, "local": local, "remote": remote}
        s = _random_strategies(rng)
        if s:
            case["strategies"] = s
        cases.append(case)
    return cases


def _random_args(rng):
    if rng.random() < 0.2:
        return None
    strategies = ["inline", "use-base", "use-local", "use-remote", "union"]
    args = {"merge_strategy": rng.choice(strategies), "ignore_transients": rng.random() < 0.7}
    if rng.random() < 0.4:
        args["input_strategy"] = rng.choice(strategies)
    if rng.random() < 0.4:
        args["output_strategy"] = rng.choice(strategies + ["remove", "clear-all"])
    return args


def notebook_cases(name, n, max_cells=12):
    rng = rng_for(name)
    cases = []
    for _ in range(n):
        base = random_notebook(rng, n_cells=rng.choice([0, 1, 2, 3, 5, 8, max_cells]))
        local = mutate_notebook(rng, base, intensity=rng.choice([0, 1, 2, 3, 5]))
        remote = mutate_notebook(rng, base, intensity=rng.choice([0, 1, 2, 3, 5]))
        if rng.random() < 0.15:
            remote = copy.deepcopy(local)
        case = {"base": base, "local": local, "remote": remote}
        args = _random_args(rng)
        if args is not None:
            case["args"] = args
        r = rng.random()
        if r < 0.1:
            case["targets"] = random_targets(rng)
        elif r < 0.2:
            case["ignores"] = copy.deepcopy(rng.choice(IGNORE_SAMPLES))
        cases.append(case)
    return cases


def family_triples():
    nbs = {name: read_without_id_repair(raw) for name, raw in upstream_test_notebooks()}
    triples = []
    for name in nbs:
        if name.endswith("--1") and name[:-3] + "--2" in nbs and name[:-3] + "--3" in nbs:
            p = name[:-3]
            triples.append((nbs[p + "--1"], nbs[p + "--2"], nbs[p + "--3"]))
    for p in ("multi_cell_nb", "cellids", "multilevel-test"):
        base = nbs.get(p) or nbs.get(p + "--base") or nbs.get(p + "-base")
        local = nbs.get(p + "--local") or nbs.get(p + "-local")
        remote = nbs.get(p + "--remote") or nbs.get(p + "-remote")
        if base and local and remote:
            triples.append((base, local, remote))
    assert len(triples) >= 6, len(triples)
    cases = []
    for b, l, r in triples:
        for args in [None, {"merge_strategy": "inline"}, {"merge_strategy": "use-local"},
                     {"merge_strategy": "use-remote"}, {"merge_strategy": "use-base"},
                     {"merge_strategy": "union"}, {"merge_strategy": "inline", "ignore_transients": False},
                     {"merge_strategy": "inline", "output_strategy": "remove"},
                     {"merge_strategy": "inline", "output_strategy": "clear-all"},
                     {"merge_strategy": "inline", "input_strategy": "use-local", "output_strategy": "use-remote"}]:
            for x, y in [(l, r), (r, l)]:
                case = {"base": b, "local": x, "remote": y}
                if args is not None:
                    case["args"] = args
                cases.append(case)
    return cases


# ---------------------------------------------------------------------------- tests


def test_u10_recorded_decide_merge(engine):
    cases = recorded("decide_merge")
    check_parity(engine, "decide_merge", cases, o_decide_merge, min_cases=30, max_error_fraction=0.1)


def test_u10_recorded_decide_merge_with_diff(engine):
    cases = recorded("decide_merge_with_diff")
    check_parity(engine, "decide_merge_with_diff", cases, o_decide_merge_with_diff, min_cases=80)


def test_u10_recorded_apply_decisions(engine):
    cases = recorded("apply_decisions")
    check_parity(engine, "apply_decisions", cases, o_apply_decisions, min_cases=35)


def test_u10_recorded_decide_notebook_merge(engine):
    cases = recorded("decide_notebook_merge")
    check_parity(engine, "decide_notebook_merge", cases, o_decide_notebook_merge, compare=compare_modulo_marker_ids, min_cases=250)


def test_u10_recorded_merge_notebooks(engine):
    cases = recorded("decide_notebook_merge")
    check_parity(engine, "merge_notebooks", cases, o_merge_notebooks, compare=compare_modulo_marker_ids, min_cases=250)


def test_u10_generic_random(engine):
    cases = generic_cases("u10-generic", scaled(6000))
    check_parity(engine, "decide_merge", cases, o_decide_merge, min_cases=scaled(6000),
                 max_error_fraction=0.2)


def test_u10_generic_apply(engine):
    cases = []
    for c in generic_cases("u10-generic-apply", scaled(3000)):
        try:
            cases.append({"base": c["base"], "decisions": o_decide_merge(**copy.deepcopy(c))})
        except Exception:
            continue
    check_parity(engine, "apply_decisions", cases, o_apply_decisions, min_cases=scaled(2000))


def test_u10_notebook_families(engine):
    cases = family_triples()
    check_parity(engine, "merge_notebooks", cases, o_merge_notebooks, compare=compare_modulo_marker_ids, min_cases=100)


def test_u10_notebook_random(engine):
    cases = notebook_cases("u10-notebooks", scaled(2000))
    check_parity(engine, "merge_notebooks", cases, o_merge_notebooks, compare=compare_modulo_marker_ids, min_cases=scaled(2000))


def test_u10_notebook_random_decisions(engine):
    cases = notebook_cases("u10-notebooks-decisions", scaled(1000))
    check_parity(engine, "decide_notebook_merge", cases, o_decide_notebook_merge, compare=compare_modulo_marker_ids, min_cases=scaled(1000))


def test_u10_notebook_apply(engine):
    cases = []
    for c in notebook_cases("u10-notebooks-apply", scaled(800)) + family_triples():
        try:
            cases.append({"base": c["base"], "decisions": o_decide_notebook_merge(**copy.deepcopy(c))})
        except Exception:
            continue
    check_parity(engine, "apply_decisions", cases, o_apply_decisions, min_cases=scaled(700),
                 compare=compare_modulo_marker_ids)


def test_u10_larger_notebooks(engine):
    cases = notebook_cases("u10-larger", scaled(80), max_cells=60)
    check_parity(engine, "merge_notebooks", cases, o_merge_notebooks, compare=compare_modulo_marker_ids, min_cases=scaled(80))


def test_u10_marker_ids_are_distinct(engine):
    """F-021: marker cells get distinct, deterministic ids within one merge."""
    cases = []
    for c in family_triples():
        if (c.get("args") or {}).get("merge_strategy", "inline") != "inline":
            continue
        try:
            o_merge_notebooks(**c)
        except Exception:  # F-024: upstream crashes on some clear-all merges; parity covers those
            continue
        cases.append(c)
    first = engine.call_many("merge_notebooks", cases)
    second = engine.call_many("merge_notebooks", cases)
    seen_markers = 0
    for case, (ok, got), (ok2, got2) in zip(cases, first, second):
        assert ok and ok2, (case, got)
        assert canon(got) == canon(got2)
        ids = [c["id"] for c in got[0]["cells"] if "id" in c]
        assert len(ids) == len(set(ids)), ids
        seen_markers += len(_marker_ids(got[0], []))
    assert seen_markers > 0


def _outputs_nb(outputs):
    return {"cells": [{"cell_type": "code", "execution_count": 1, "id": "c1", "metadata": {},
                       "outputs": outputs, "source": "x = 1\n"}],
            "metadata": {}, "nbformat": 4, "nbformat_minor": 5}


def _stream(text):
    return {"name": "stdout", "output_type": "stream", "text": text}


def test_u10_f027_appended_output_conflicts(engine):
    """F-027: both sides append different outputs to a non-empty outputs list (default nbmerge)."""
    base = [_stream("a\n")]
    cases = []
    for args in (None, {}, {"output_strategy": "inline-outputs"}, {"merge_strategy": "inline"}):
        for extra_l, extra_r in ((["L\n"], ["R\n"]), (["L\n", "M\n"], ["R\n"]), (["L\n"], []), (["x\n"], ["x\n"])):
            c = {"base": _outputs_nb(base), "local": _outputs_nb(base + [_stream(t) for t in extra_l]),
                 "remote": _outputs_nb(base + [_stream(t) for t in extra_r])}
            if args is not None:
                c["args"] = args
            cases.append(c)
    check_parity(engine, "merge_notebooks", cases, o_merge_notebooks, min_cases=len(cases),
                 compare=compare_modulo_marker_ids)
    check_parity(engine, "decide_notebook_merge", cases, o_decide_notebook_merge, min_cases=len(cases),
                 compare=compare_modulo_marker_ids)


def test_u10_f027_fix_is_neutral():
    """The F-027 oracle fix changes nothing where upstream does not raise, and it is exercised."""
    cases = family_triples() + notebook_cases("u10-f027", 400)
    base = [_stream("a\n")]
    cases.append({"base": _outputs_nb(base), "local": _outputs_nb(base + [_stream("L\n")]),
                  "remote": _outputs_nb(base + [_stream("R\n")])})
    fixed_only = 0
    for c in cases:
        try:
            unfixed = o_merge_notebooks(**c, fix_f027=False)
        except IndexError:
            o_merge_notebooks(**c)
            fixed_only += 1
            continue
        except Exception:
            continue
        assert compare_modulo_marker_ids(unfixed, o_merge_notebooks(**c)), c
    assert fixed_only >= 1
