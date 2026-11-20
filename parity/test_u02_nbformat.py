"""U02: reading and writing notebooks as nbformat does (nbformat 5.11.1, v4 only).

`nbformat.reads {text}` returns the in-memory notebook that `nbformat.reads(text, as_version=4)`
returns: lines rejoined, transient keys stripped, cell ids repaired for nbformat >= 4.5.
`nbformat.writes {nb}` returns the file text that `nbformat.write(nb, fp)` writes.

Deliberate deviations (docs/findings.md): repaired cell ids are deterministic (F-003), so cases
with repaired ids are compared modulo those ids; a leading UTF-8 BOM is accepted (Q-002);
nbformat v1-v3 input is rejected (F-012).
"""

import copy
import json
import logging
import re
import warnings

import nbformat

from gen import rng_for, random_string
from gen_nb import dumps_disk, random_notebook, mutate_notebook, to_disk_form
from harness import canon, check_parity, scaled, upstream_test_notebooks, UPSTREAM

logging.getLogger("traitlets").setLevel(logging.CRITICAL)
logging.getLogger("nbformat").setLevel(logging.CRITICAL)

ID_RE = re.compile(r"^[0-9a-f]{8}$")


def o_reads(text):
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        return nbformat.reads(text, as_version=4)


def o_writes(nb):
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        s = nbformat.writes(nbformat.from_dict(nb))
    return s if s.endswith("\n") else s + "\n"


def repaired_positions(nb_dict):
    """Indices of cells whose id nbformat's validator replaces with a random one."""
    try:
        version = (nb_dict.get("nbformat", 1), nb_dict.get("nbformat_minor", 0))
        if version < (4, 5):
            return set()
    except TypeError:
        return set()
    cells = nb_dict.get("cells") or []
    out = {i for i, c in enumerate(cells) if isinstance(c, dict) and "id" not in c}
    seen = set()
    for i, c in enumerate(cells):
        if i in out or not isinstance(c, dict):
            continue
        if c["id"] in seen:
            out.add(i)
        seen.add(c["id"])
    return out


def mask_ids(nb, positions):
    nb = copy.deepcopy(nb)
    for i in positions:
        nb["cells"][i]["id"] = "<repaired>"
    return nb


def check_repaired(engine, fn, cases, inputs, oracle, decode, min_cases):
    """Compare modulo repaired ids; repaired ids must be 8 hex chars, unique and deterministic."""
    assert len(cases) >= min_cases
    first = engine.call_many(fn, cases)
    second = engine.call_many(fn, cases)
    failures = []
    for case, nb_in, (ok, got), (ok2, got2) in zip(cases, inputs, first, second):
        expected = oracle(**json.loads(json.dumps(case)))
        if not ok or not ok2:
            failures.append((case, "a result", got))
            continue
        if canon(got) != canon(got2):
            failures.append((case, "the same result twice", [got, got2]))
            continue
        got_nb, exp_nb = decode(got), decode(expected)
        positions = repaired_positions(nb_in)
        ids = [c.get("id") for c in got_nb["cells"]]
        if any(not (isinstance(ids[i], str) and ID_RE.match(ids[i])) for i in positions):
            failures.append((case, "8-hex repaired ids", ids))
        elif len(set(ids)) != len(ids):
            failures.append((case, "unique ids", ids))
        elif canon(mask_ids(got_nb, positions)) != canon(mask_ids(exp_nb, positions)):
            failures.append((case, mask_ids(exp_nb, positions), mask_ids(got_nb, positions)))
    assert not failures, "%d of %d differ, first: %r" % (len(failures), len(cases), failures[:3])


# ---------------------------------------------------------------------------- case sources


def upstream_file_texts():
    files = sorted((UPSTREAM / "nbdime" / "tests" / "files").glob("*.ipynb"))
    return [f.read_text(encoding="utf-8") for f in files]


def add_transients(rng, nb):
    nb = copy.deepcopy(nb)
    if rng.random() < 0.5:
        nb["metadata"]["signature"] = "sha256:" + random_string(rng, "0123456789abcdef", 16)
    if rng.random() < 0.3:
        nb["metadata"]["orig_nbformat"] = 3
        nb["metadata"]["orig_nbformat_minor"] = 0
    for cell in nb["cells"]:
        if rng.random() < 0.3:
            cell["metadata"]["trusted"] = rng.random() < 0.5
    return nb


def odd_layouts(rng, nb):
    """Disk-form variants that exercise rejoin_lines branches."""
    d = to_disk_form(nb)
    for cell in d["cells"]:
        r = rng.random()
        if r < 0.2 and isinstance(cell.get("source"), list):
            cell["source"] = "".join(cell["source"])
        for out in cell.get("outputs", []):
            data = out.get("data")
            if data is not None and rng.random() < 0.3:
                q = rng.random()
                if q < 0.3:
                    data["application/json"] = ["not", "joined"]
                elif q < 0.5:
                    data["application/vnd.custom+json"] = ["x\n", "y"]
                elif q < 0.7:
                    data["text/plain"] = ["a\n", 1, "b"]
                elif q < 0.85:
                    data["Text/Plain"] = ["upper\n", "case"]
                else:
                    data["application/x-other"] = ["joined\n", "too"]
            if out.get("output_type") == "error" and rng.random() < 0.2:
                out["text"] = ["error outputs with text\n", "are joined"]
    if d["cells"] and rng.random() < 0.2:
        d["cells"].append({"cell_type": "markdown", "metadata": {}, "source": ["md\n", "x"],
                           "outputs": [{"output_type": "stream", "name": "stdout", "text": ["not\n", "joined"]}]})
    return d


def text_cases(name, n):
    rng = rng_for(name)
    texts = []
    for _ in range(n):
        nb = random_notebook(rng, ids=True if rng.random() < 0.5 else False)
        if rng.random() < 0.3:
            nb = mutate_notebook(rng, nb)
        nb = add_transients(rng, nb)
        if rng.random() < 0.3:
            texts.append(json.dumps(odd_layouts(rng, nb), indent=rng.choice([None, 1, 2])))
        else:
            texts.append(dumps_disk(nb, rng))
    return [{"text": t} for t in texts]


def without_repairs(cases):
    return [c for c in cases if not repaired_positions(json.loads(c["text"]))]


def malformed_cases(name, n):
    """Structurally broken v4 notebooks: both sides must fail, or agree."""
    rng = rng_for(name)
    out = []
    for _ in range(n):
        nb = to_disk_form(random_notebook(rng, n_cells=rng.randint(1, 4), ids=False))
        r = rng.random()
        if r < 0.2:
            del nb[rng.choice(["metadata", "cells"])]
        elif r < 0.4:
            del nb["cells"][0]["metadata"]
        elif r < 0.55:
            nb["cells"][0].pop("cell_type")
        elif r < 0.7:
            nb["metadata"] = rng.choice([[], "x", None, 3])
        elif r < 0.8:
            nb["cells"] = rng.choice([{}, "x", None])
        elif r < 0.9:
            nb["cells"].append(rng.choice([1, "x", [], None]))
        else:
            cell = nb["cells"][0]
            cell["outputs"] = [{"text": ["no\n", "type"]}, {"output_type": "stream", "text": 5}]
            cell["cell_type"] = "code"
        out.append({"text": json.dumps(nb)})
    return out


# ---------------------------------------------------------------------------- reads


def test_u02_reads_upstream_files(engine):
    cases = without_repairs([{"text": t} for t in upstream_file_texts()])
    check_parity(engine, "nbformat.reads", cases, o_reads, min_cases=40)


def test_u02_reads_random(engine):
    cases = without_repairs(text_cases("u02-reads", scaled(3000)))
    check_parity(engine, "nbformat.reads", cases, o_reads, min_cases=scaled(2000))


def test_u02_reads_malformed(engine):
    cases = malformed_cases("u02-malformed", scaled(600))
    check_parity(engine, "nbformat.reads", cases, o_reads, min_cases=scaled(600), max_error_fraction=1.0)


def test_u02_reads_not_json(engine):
    cases = [{"text": t} for t in ["", " ", "{", "[]", "null", "1", '"x"', "{'a': 1}", "﻿{}"]]
    for (ok, _), case in zip(engine.call_many("nbformat.reads", cases), cases):
        assert not ok, case


def repair_cases(name, n):
    rng = rng_for(name)
    cases, inputs = [], []
    for _ in range(n):
        nb = random_notebook(rng, n_cells=rng.choice([1, 2, 3, 5, 8]), ids=True)
        cells = nb["cells"]
        for c in cells:
            if rng.random() < 0.4:
                del c["id"]
        if len(cells) > 1 and rng.random() < 0.5:
            i, j = rng.sample(range(len(cells)), 2)
            if "id" in cells[i]:
                cells[j]["id"] = cells[i]["id"]
        if len(cells) > 1 and rng.random() < 0.2:
            cells.append(copy.deepcopy(cells[0]))
        d = to_disk_form(nb)
        cases.append({"text": json.dumps(d, indent=1, sort_keys=True)})
        inputs.append(d)
    return cases, inputs


def test_u02_reads_repairs_ids(engine):
    cases, inputs = repair_cases("u02-ids", scaled(800))
    files = upstream_file_texts()
    for t in files:
        d = json.loads(t)
        if repaired_positions(d):
            cases.append({"text": t})
            inputs.append(d)
    assert any(repaired_positions(d) for d in inputs)
    check_repaired(engine, "nbformat.reads", cases, inputs, o_reads, lambda nb: nb, min_cases=scaled(800))


def test_u02_reads_ids_are_content_stable(engine):
    """The same cell gets the same repaired id wherever it sits (F-003)."""
    cell = {"cell_type": "markdown", "metadata": {}, "source": "# same"}
    other = {"cell_type": "code", "metadata": {}, "source": "x = 1", "outputs": [], "execution_count": None}
    a = {"cells": [cell], "metadata": {}, "nbformat": 4, "nbformat_minor": 5}
    b = {"cells": [other, cell], "metadata": {}, "nbformat": 4, "nbformat_minor": 5}
    (ok_a, ra), (ok_b, rb) = engine.call_many("nbformat.reads", [{"text": json.dumps(a)}, {"text": json.dumps(b)}])
    assert ok_a and ok_b
    assert ra["cells"][0]["id"] == rb["cells"][1]["id"]


def test_u02_reads_accepts_bom(engine):
    """Deviation Q-002: a leading BOM is ignored."""
    texts = upstream_file_texts()[:10] + [c["text"] for c in without_repairs(text_cases("u02-bom", 50))]
    texts = [t for t in texts if not repaired_positions(json.loads(t))]
    results = engine.call_many("nbformat.reads", [{"text": "﻿" + t} for t in texts])
    for t, (ok, got) in zip(texts, results):
        assert ok, got
        assert canon(got) == canon(o_reads(t))


def test_u02_reads_rejects_old_versions(engine):
    """Deviation F-012: nbformat v1-v3 are rejected with an error that names the version."""
    v3 = {"metadata": {"name": ""}, "nbformat": 3, "nbformat_minor": 0,
          "worksheets": [{"cells": [{"cell_type": "code", "input": "1", "language": "python",
                                     "metadata": {}, "outputs": [], "prompt_number": 1}], "metadata": {}}]}
    cases = [{"text": json.dumps(dict(v3, nbformat=v))} for v in (1, 2, 3)]
    cases.append({"text": json.dumps({"cells": [], "metadata": {}})})  # no nbformat key: v1
    cases.append({"text": json.dumps({"cells": [], "metadata": {}, "nbformat": 5, "nbformat_minor": 0})})
    for (ok, got), case in zip(engine.call_many("nbformat.reads", cases), cases):
        assert not ok, case
        assert "nbformat" in got.lower() or "version" in got.lower(), got


# ---------------------------------------------------------------------------- writes


def write_cases(name, n):
    rng = rng_for(name)
    out = []
    for _ in range(n):
        nb = random_notebook(rng, ids=rng.random() < 0.5)
        if rng.random() < 0.4:
            nb = mutate_notebook(rng, nb)
        nb = add_transients(rng, nb)
        for cell in nb["cells"]:
            for o in cell.get("outputs", []):
                if "data" in o and rng.random() < 0.2:
                    o["data"][rng.choice(["Text/Plain", "application/javascript", "image/svg+xml",
                                          "application/x-thing", "text/x-weird"])] = \
                        "line one\r\nline two\x0bthree four\x85five\n"
            if rng.random() < 0.1:
                cell["source"] = "tab\there\r\nunicode é\U0001F600\x1c sep\n\n"
        if rng.random() < 0.2:
            nb["metadata"]["floats"] = [0.1, 1e-7, 1e22, -0.0, 3.0, 12345678901234567890]
            nb["metadata"]["ünicode key"] = {"z": 1, "a": 2, "\U0001F600": 3}
        out.append({"nb": nb})
    return out


def test_u02_writes_random(engine):
    cases = [c for c in write_cases("u02-writes", scaled(3000)) if not repaired_positions(c["nb"])]
    check_parity(engine, "nbformat.writes", cases, o_writes, min_cases=scaled(2000))


def test_u02_writes_upstream_files(engine):
    cases = []
    for name, raw in upstream_test_notebooks():
        if not repaired_positions(raw):
            cases.append({"nb": json.loads(json.dumps(o_reads(json.dumps(raw))))})
    check_parity(engine, "nbformat.writes", cases, o_writes, min_cases=40)


def test_u02_writes_repairs_ids(engine):
    cases, inputs = [], []
    for c in write_cases("u02-writes-ids", scaled(800)):
        nb = c["nb"]
        if nb["nbformat_minor"] < 5:
            continue
        for cell in nb["cells"]:
            if cell.get("id") and int(cell["id"], 16) % 3 == 0:
                del cell["id"]
        cases.append({"nb": nb})
        inputs.append(nb)
    check_repaired(engine, "nbformat.writes", cases, inputs, o_writes, json.loads, min_cases=scaled(300))


def test_u02_roundtrip(engine):
    """reads(writes(nb)) == nb for notebooks nbformat itself would produce."""
    cases = [c for c in write_cases("u02-roundtrip", scaled(500)) if not repaired_positions(c["nb"])]
    written = engine.call_many("nbformat.writes", cases)
    assert all(ok for ok, _ in written)
    back = engine.call_many("nbformat.reads", [{"text": t} for _, t in written])
    for (ok, nb), (_, text) in zip(back, written):
        assert ok, nb
        assert canon(nb) == canon(o_reads(text))


def test_u02_written_ids_do_not_depend_on_spelling(engine):
    """F-003 on write: a repaired id depends on the cell as it reads back, not on how it was given."""
    variants = [{"cell_type": "raw", "metadata": {}, "source": "s\nt"},
                {"cell_type": "raw", "metadata": {}, "source": ["s\n", "t"]},
                {"cell_type": "raw", "metadata": {"trusted": True}, "source": "s\nt"}]
    nbs = [{"cells": [c], "metadata": {}, "nbformat": 4, "nbformat_minor": 5} for c in variants]
    written = engine.call_many("nbformat.writes", [{"nb": nb} for nb in nbs])
    assert all(ok for ok, _ in written), written
    assert len({t for _, t in written}) == 1
    (ok, back), = engine.call_many("nbformat.reads", [{"text": written[0][1]}])
    assert ok and back["cells"][0]["id"] == json.loads(written[0][1])["cells"][0]["id"]
