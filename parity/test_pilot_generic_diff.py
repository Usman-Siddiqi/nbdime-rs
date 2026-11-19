"""P1 pilot: the generic diff engine.

Covers difflib.SequenceMatcher (as nbdime uses it), character and line diffs of strings,
compare_strings_approximate, brute-force LCS with Python equality, multilevel snakes, and the
generic nbdime.diff on arbitrary JSON. Engine function names and argument shapes are fixed here;
the implementation must provide them in `nbdime-rs-dev`.
"""

import copy
import itertools
import operator
from difflib import SequenceMatcher

from nbdime import diff as nbdime_diff
from nbdime.diffing.generic import compare_strings_approximate, diff_lists  # noqa: F401
from nbdime.diffing.seq_bruteforce import diff_sequence_bruteforce
from nbdime.diffing.sequences import diff_strings_by_char, diff_strings_linewise
from nbdime.diffing.snakes import compute_snakes_multilevel

from gen import json_pair, rng_for, string_pair, text_pair, random_json, mutate_json
from harness import check_parity, generic_diff_fixed, notebook_families, scaled, upstream_test_notebooks

# ---------------------------------------------------------------------------- oracles


def o_opcodes(a, b):
    return [list(op) for op in SequenceMatcher(None, a, b, autojunk=False).get_opcodes()]


def o_matching_blocks(a, b):
    return [list(m) for m in SequenceMatcher(None, a, b, autojunk=False).get_matching_blocks()]


def o_ratios(a, b):
    s = SequenceMatcher(None, a, b, autojunk=False)
    return [s.ratio(), s.quick_ratio(), s.real_quick_ratio()]


def o_compare(x, y, threshold=0.7, maxlen=None):
    return compare_strings_approximate(x, y, threshold=threshold, maxlen=maxlen)


def o_snakes_lines(a, b):
    return [list(s) for s in compute_snakes_multilevel(a, b, [compare_strings_approximate, operator.__eq__])]


def o_bruteforce(a, b):
    return diff_sequence_bruteforce(a, b, operator.__eq__)


def o_diff(a, b):
    # Upstream with the F-001 fix (see docs/findings.md); test_pilot_f001_fix_is_neutral shows
    # it only differs where upstream raises the spurious error.
    return generic_diff_fixed(a, b)


# ---------------------------------------------------------------------------- case sources


def string_cases(name, n):
    rng = rng_for(name)
    cases = []
    for _ in range(n):
        a, b = string_pair(rng)
        if rng.random() < 0.5:
            a, b = b, a
        cases.append({"a": a, "b": b})
    cases += [{"a": a, "b": b} for a, b in [("", ""), ("", "x"), ("x", ""), ("abc", "abc"),
                                            ("aaaa", "aa"), ("abab", "baba"), ("\U0001F600a", "a\U0001F600")]]
    return cases


def text_cases(name, n):
    rng = rng_for(name)
    return [dict(zip(("a", "b"), text_pair(rng))) for _ in range(n)]


def joined_notebook(nb):
    """nbformat's rejoin_lines, without its id normalization (which is random)."""
    nb = copy.deepcopy(nb)
    for cell in nb.get("cells", []):
        if isinstance(cell.get("source"), list):
            cell["source"] = "".join(cell["source"])
        for out in cell.get("outputs", []) or []:
            if isinstance(out.get("text"), list):
                out["text"] = "".join(out["text"])
            for k, v in list((out.get("data") or {}).items()):
                if isinstance(v, list) and all(isinstance(x, str) for x in v) and not k.endswith("json"):
                    out["data"][k] = "".join(v)
    return nb


def notebook_string_pairs(limit_per_family=400):
    pairs = []
    for fam in notebook_families(upstream_test_notebooks()).values():
        texts = []
        for _, nb in fam:
            for cell in joined_notebook(nb).get("cells", []):
                texts.append(cell.get("source", ""))
                for out in cell.get("outputs", []) or []:
                    if isinstance(out.get("text"), str):
                        texts.append(out["text"])
        texts = list(dict.fromkeys(texts))
        fam_pairs = list(itertools.product(texts, repeat=2))[:limit_per_family]
        pairs += [{"a": a, "b": b} for a, b in fam_pairs]
    return pairs


# ---------------------------------------------------------------------------- difflib


def test_pilot_difflib_opcodes(engine):
    cases = string_cases("pilot-opcodes", scaled(5000)) + notebook_string_pairs(100)
    check_parity(engine, "difflib.opcodes", cases, o_opcodes, min_cases=scaled(5000))


def test_pilot_difflib_matching_blocks(engine):
    cases = string_cases("pilot-blocks", scaled(3000))
    check_parity(engine, "difflib.matching_blocks", cases, o_matching_blocks, min_cases=scaled(3000))


def test_pilot_difflib_ratios(engine):
    cases = string_cases("pilot-ratios", scaled(5000)) + notebook_string_pairs(100)
    check_parity(engine, "difflib.ratios", cases, o_ratios, min_cases=scaled(5000))


# ---------------------------------------------------------------------------- strings


def test_pilot_compare_strings_approximate(engine):
    rng = rng_for("pilot-compare")
    cases = []
    for c in string_cases("pilot-compare-strings", scaled(6000)):
        case = {"x": c["a"], "y": c["b"]}
        r = rng.random()
        if r < 0.3:
            case["threshold"] = rng.choice([0.5, 0.7, 0.95, 0.0, 1.0])
        if rng.random() < 0.3:
            case["maxlen"] = rng.choice([0, 3, 10, 50])
        cases.append(case)
    # Ratios landing exactly on the threshold: 'ab' vs 'ac' has ratio 0.5.
    cases += [{"x": "ab", "y": "ac", "threshold": 0.5}, {"x": "abcdefghij", "y": "abcdefgxyz", "threshold": 0.7}]
    check_parity(engine, "compare_strings_approximate", cases, o_compare, min_cases=scaled(6000))


def test_pilot_diff_strings_by_char(engine):
    cases = string_cases("pilot-bychar", scaled(4000))
    check_parity(engine, "diff_strings_by_char", cases, diff_strings_by_char, min_cases=scaled(4000))


def test_pilot_diff_strings_linewise(engine):
    cases = text_cases("pilot-linewise", scaled(5000)) + notebook_string_pairs()
    check_parity(engine, "diff_strings_linewise", cases, diff_strings_linewise, min_cases=scaled(5000))


def test_pilot_snakes_multilevel_lines(engine):
    cases = []
    for c in text_cases("pilot-snakes", scaled(3000)):
        cases.append({"a": c["a"].splitlines(True), "b": c["b"].splitlines(True)})
    check_parity(engine, "snakes_multilevel_lines", cases, o_snakes_lines, min_cases=scaled(3000))


# ---------------------------------------------------------------------------- sequences and generic diff


def test_pilot_bruteforce_lcs_python_equality(engine):
    rng = rng_for("pilot-bruteforce")
    vocab = [0, 1, 1.0, True, False, 0.0, None, "a", "b", "1", [1], [1.0], {"a": 1}, {"a": True}, [], {}]
    cases = []
    for _ in range(scaled(4000)):
        a = [rng.choice(vocab) for _ in range(rng.randint(0, 9))]
        b = [rng.choice(vocab) for _ in range(rng.randint(0, 9))]
        cases.append({"a": a, "b": b})
    check_parity(engine, "diff_sequence_bruteforce", cases, o_bruteforce, min_cases=scaled(4000))


def test_pilot_generic_diff_random_json(engine):
    rng = rng_for("pilot-generic")
    cases = []
    for _ in range(scaled(6000)):
        a, b = json_pair(rng)
        cases.append({"a": a, "b": b})
    # Mismatched top-level types are an error upstream.
    cases += [{"a": [], "b": {}}, {"a": "x", "b": []}, {"a": 1, "b": 1}]
    check_parity(engine, "diff", cases, o_diff, min_cases=scaled(6000))


def test_pilot_generic_diff_f001_cases(engine):
    """Cases where upstream raises F-001; we produce what upstream intends.

    In generic diffs aligned items compare equal, so the intended result for the type-changed
    (but equal, e.g. 1 vs True) value is no entry at all.
    """
    cases = [
        {"a": [[1], {"k": 1}], "b": [[1], {"k": True}]},
        {"a": {"x": [[1], {"k": 1}]}, "b": {"x": [[1], {"k": 1.0}]}},
        {"a": [[1], {"k": 0, "j": [2]}], "b": [[1], {"k": False, "j": [2.0]}]},
    ]
    for c in cases:
        try:
            nbdime_diff(c["a"], c["b"])
            raise AssertionError("expected upstream to raise F-001 for %r" % (c,))
        except RuntimeError as e:
            assert "pointing to dict entry" in str(e)
    check_parity(engine, "diff", cases, o_diff, min_cases=3)


def test_pilot_generic_diff_upstream_notebooks(engine):
    cases = []
    for fam in notebook_families(upstream_test_notebooks()).values():
        for (_, a), (_, b) in itertools.product(fam, repeat=2):
            cases.append({"a": a, "b": b})
            cases.append({"a": joined_notebook(a), "b": joined_notebook(b)})
    rng = rng_for("pilot-generic-nb")
    for _, nb in upstream_test_notebooks():
        for _ in range(scaled(20)):
            a = joined_notebook(nb)
            b = mutate_json(rng, a)
            if isinstance(b, dict):  # a scalar root is just an error case on both sides
                cases.append({"a": a, "b": b})
    check_parity(engine, "diff", cases, o_diff, min_cases=1000)


def test_pilot_f001_fix_is_neutral():
    """The fixed oracle agrees with upstream on every case where upstream does not raise F-001."""
    rng = rng_for("pilot-generic")
    differing = 0
    for _ in range(scaled(3000)):
        a, b = json_pair(rng)
        try:
            expected = nbdime_diff(a, b)
        except RuntimeError as e:
            if "pointing to dict entry" in str(e):
                continue
            raise
        if expected != generic_diff_fixed(a, b):
            differing += 1
    assert differing == 0
