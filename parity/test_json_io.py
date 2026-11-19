"""F3: JSON parsing and serialization must match Python's json module byte for byte."""

import json
import math
import random
import struct

from gen import random_json, rng_for, multiline_text
from harness import check_parity, scaled, upstream_test_notebooks

OPTION_SETS = [
    {},
    {"separators": [",", ":"]},
    {"indent": 2, "separators": [",", ": "]},
    {"indent": 1, "sort_keys": True, "ensure_ascii": False},
    {"indent": 0},
]


def oracle_loads_dumps(text, options):
    kw = dict(options)
    if "separators" in kw:
        kw["separators"] = tuple(kw["separators"])
    return json.dumps(json.loads(text), **kw)


def test_json_random_documents(engine):
    rng = rng_for("json-docs")
    cases = []
    for i in range(scaled(5000)):
        doc = random_json(rng, max_depth=5)
        text = json.dumps(doc, ensure_ascii=rng.random() < 0.5, indent=rng.choice([None, 1, 2]))
        cases.append({"text": text, "options": OPTION_SETS[i % len(OPTION_SETS)]})
    check_parity(engine, "json.loads_dumps", cases, oracle_loads_dumps, min_cases=scaled(5000))


def _random_float(rng):
    r = rng.random()
    if r < 0.4:
        return struct.unpack("<d", struct.pack("<Q", rng.getrandbits(64)))[0]
    if r < 0.7:
        return rng.uniform(-1e6, 1e6)
    if r < 0.85:
        return float(rng.randint(-10**17, 10**17)) * 10.0 ** rng.randint(-30, 30)
    return rng.choice([0.0, -0.0, 1e16, 1e15, 9999999999999998.0, 1e-4, 1e-5, 5e-324,
                       1.7976931348623157e308, 0.1, 0.2, 0.30000000000000004, 123456789.125])


def test_json_float_repr(engine):
    rng = rng_for("json-floats")
    floats = [_random_float(rng) for _ in range(scaled(100_000))]
    cases = []
    for start in range(0, len(floats), 1000):
        chunk = [f for f in floats[start:start + 1000] if not math.isnan(f)]
        cases.append({"text": json.dumps(chunk), "options": {}})
    cases.append({"text": "[NaN, Infinity, -Infinity, 1E400, -1e-400]", "options": {}})
    check_parity(engine, "json.loads_dumps", cases, oracle_loads_dumps, min_cases=scaled(100))


def test_json_numbers_and_strings_edge_cases(engine):
    texts = [
        "0", "-0", "-0.0", "1e5", "1E+5", "1e-5", "123456789012345678901234567890",
        "-123456789012345678901234567890", "[1.0, 1, true]", '"\\u00e9\\ud83d\\ude00"',
        '"\\u007f\\u0000\\u001f"', '"tab\\there"', '{"a": 1, "a": 2, "b": 3}', '{"": ""}',
        '"\\/"', "[]", "{}", "[[], {}, [[]]]", " \t\n[1]\r\n", '"  \x85"',
        "1.5e300", "4.9e-324", "2.2250738585072014e-308",
    ]
    cases = [{"text": t, "options": o} for t in texts for o in OPTION_SETS]
    check_parity(engine, "json.loads_dumps", cases, oracle_loads_dumps, min_cases=len(texts) * 5)


def test_json_rejects_what_python_rejects(engine):
    bad = ["", "01", "1.", ".5", "+1", "[1,]", '{"a":1,}', "'a'", '"\t"', "[1 2]", "nul",
           "﻿{}", '{"a" 1}', '"\\x"', '"\\u12"', "1 2", "[", '{"a":', "-", "-Inf", "nan"]
    cases = [{"text": t, "options": {}} for t in bad]
    check_parity(engine, "json.loads_dumps", cases, oracle_loads_dumps, min_cases=len(bad),
                 max_error_fraction=1.0)


def test_json_upstream_notebooks(engine):
    nbs = upstream_test_notebooks()
    cases = []
    with_options = [{"indent": 1, "sort_keys": True, "ensure_ascii": False}, {"indent": 2, "separators": [",", ": "]}]
    for name, nb in nbs:
        text = json.dumps(nb, ensure_ascii=False, indent=1)
        for o in with_options:
            cases.append({"text": text, "options": o})
    check_parity(engine, "json.loads_dumps", cases, oracle_loads_dumps, min_cases=2 * 50)


def test_splitlines(engine):
    rng = rng_for("splitlines")
    cases = [{"s": multiline_text(rng)} for _ in range(scaled(3000))]
    cases += [{"s": s} for s in ["", "\n", "\r\n", "\r\r\n\n", "a\x1c\x1d\x1eb", "\x85  ", "no end"]]
    check_parity(engine, "splitlines", cases, lambda s: s.splitlines(True), min_cases=scaled(3000))
