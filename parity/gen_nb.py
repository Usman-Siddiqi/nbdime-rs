"""Seeded generators of notebooks and notebook mutations for notebook-level parity suites.

Notebooks are produced in nbformat's in-memory form (multi-line strings joined), as
`nbformat.reads` returns them. Use `to_disk_form` for the on-disk form with split lines.
"""

import base64
import copy
import json

from gen import LINE_VOCAB, mutate_lines, mutate_string, multiline_text, random_string

MIMES_TEXT = ["text/plain", "text/html", "text/markdown", "text/latex"]


def _b64(rng, n):
    return base64.b64encode(bytes(rng.getrandbits(8) for _ in range(n))).decode()


def _b64_lines(rng, n):
    s = _b64(rng, n)
    return "\n".join(s[i:i + 76] for i in range(0, len(s), 76)) + "\n"


def _cell_id(rng):
    return "%08x" % rng.getrandbits(32)


def random_text(rng, short=False):
    if short or rng.random() < 0.2:
        return rng.choice(["", "1", "x", "42\n", "ok", "<object at 0x7f3a2b1c4d5e>", "True"])
    return multiline_text(rng, rng.randint(1, 6))


def random_output(rng, count):
    r = rng.random()
    if r < 0.3:
        return {"output_type": "stream", "name": rng.choice(["stdout", "stdout", "stderr"]),
                "text": random_text(rng)}
    if r < 0.6:
        data = {"text/plain": random_text(rng)}
        if rng.random() < 0.3:
            data["text/html"] = "<b>%s</b>" % random_string(rng, "abc <>/", 20)
        if rng.random() < 0.15:
            data["application/json"] = {"a": rng.randint(0, 3), "b": [1, 2.5, None]}
        if rng.random() < 0.1:
            data["image/svg+xml"] = "<svg>\n<rect width='%d'/>\n</svg>\n" % rng.randint(1, 9)
        return {"output_type": "execute_result", "execution_count": count, "metadata": {}, "data": data}
    if r < 0.85:
        data = {"text/plain": rng.choice(["<Figure size 640x480 with 1 Axes>", "<IPython.core.display.Image object>"])}
        mime = rng.choice(["image/png", "image/jpeg", "application/pdf", "image/gif"])
        data[mime] = _b64_lines(rng, rng.choice([10, 48, 300])) if rng.random() < 0.7 else _b64(rng, rng.choice([10, 48, 300]))
        md = {} if rng.random() < 0.7 else {mime: {"width": rng.randint(100, 900)}}
        return {"output_type": "display_data", "metadata": md, "data": data}
    ename = rng.choice(["ValueError", "KeyError", "ZeroDivisionError"])
    tb = ["\x1b[0;31m-----\x1b[0m", "Traceback (most recent call last):", random_string(rng, None, 30),
          "%s: %s" % (ename, random_string(rng, "abc ", 10))][: rng.randint(1, 4)]
    return {"output_type": "error", "ename": ename, "evalue": random_string(rng, "abc 0123", 12), "traceback": tb}


def random_cell(rng, ids, count):
    r = rng.random()
    if r < 0.6:
        cell = {"cell_type": "code", "execution_count": count if rng.random() < 0.9 else None,
                "metadata": {} if rng.random() < 0.7 else {"tags": ["t%d" % rng.randint(0, 3)], "scrolled": True},
                "outputs": [random_output(rng, count) for _ in range(rng.choice([0, 0, 1, 1, 2, 3]))],
                "source": "".join(rng.choice(LINE_VOCAB) for _ in range(rng.randint(0, 8))).rstrip("\n")}
    elif r < 0.92:
        cell = {"cell_type": "markdown", "metadata": {},
                "source": "# " + random_string(rng, "abcdef ", 15) + "\n\n" + random_text(rng)}
        if rng.random() < 0.1:
            name = "img%d.png" % rng.randint(0, 2)
            cell["attachments"] = {name: {"image/png": _b64(rng, 48)}}
            cell["source"] += "\n![x](attachment:%s)" % name
    else:
        cell = {"cell_type": "raw", "metadata": {}, "source": random_text(rng)}
    if ids:
        cell["id"] = _cell_id(rng)
    return cell


def random_notebook(rng, n_cells=None, ids=None):
    ids = rng.random() < 0.5 if ids is None else ids
    n = rng.choice([0, 1, 2, 3, 5, 8, 12, 20]) if n_cells is None else n_cells
    nb = {"cells": [random_cell(rng, ids, i + 1) for i in range(n)],
          "metadata": {"kernelspec": {"display_name": "Python 3", "language": "python", "name": "python3"},
                       "language_info": {"name": "python", "version": "3.%d.0" % rng.randint(8, 12)}},
          "nbformat": 4, "nbformat_minor": 5 if ids else 4}
    return nb


def mutate_output(rng, out):
    out = copy.deepcopy(out)
    ot = out["output_type"]
    r = rng.random()
    if ot == "stream":
        out["text"] = mutate_lines(rng, out["text"]) if r < 0.7 else random_text(rng)
    elif ot == "error":
        if r < 0.5 and out["traceback"]:
            i = rng.randrange(len(out["traceback"]))
            out["traceback"][i] = mutate_string(rng, out["traceback"][i])
        else:
            out["evalue"] = mutate_string(rng, out["evalue"])
    else:
        data = out["data"]
        key = rng.choice(sorted(data))
        v = data[key]
        if isinstance(v, str):
            if "0x" in v and rng.random() < 0.5:
                data[key] = v.replace("0x7f3a2b1c4d5e", "0x7f00deadbeef")
            elif key.startswith("image/") and key != "image/svg+xml":
                data[key] = _b64(rng, 48) if rng.random() < 0.5 else v
            else:
                data[key] = mutate_lines(rng, v) if rng.random() < 0.6 else mutate_string(rng, v)
        elif rng.random() < 0.5:
            data[key] = {"a": 9}
        if rng.random() < 0.15:
            data["text/html"] = "<i>new</i>"
        if rng.random() < 0.1 and len(data) > 1:
            del data[sorted(data)[0]]
        if "execution_count" in out and rng.random() < 0.3:
            out["execution_count"] = (out["execution_count"] or 0) + 1
        if rng.random() < 0.1:
            out["metadata"] = {"isolated": True}
    return out


def mutate_cell(rng, cell):
    cell = copy.deepcopy(cell)
    r = rng.random()
    if r < 0.45:
        cell["source"] = mutate_lines(rng, cell["source"]) if rng.random() < 0.8 else random_text(rng)
    elif r < 0.75 and cell["cell_type"] == "code":
        outs = cell["outputs"]
        q = rng.random()
        if q < 0.5 and outs:
            i = rng.randrange(len(outs))
            outs[i] = mutate_output(rng, outs[i])
        elif q < 0.7:
            outs.insert(rng.randint(0, len(outs)), random_output(rng, cell.get("execution_count")))
        elif q < 0.85 and outs:
            del outs[rng.randrange(len(outs))]
        elif len(outs) > 1:
            outs.reverse()
        if rng.random() < 0.4:
            cell["execution_count"] = (cell.get("execution_count") or 0) + 10
    elif r < 0.85:
        cell["metadata"] = {"tags": ["changed"]} if rng.random() < 0.5 else {}
        if rng.random() < 0.3:
            cell["metadata"]["collapsed"] = rng.random() < 0.5
    elif r < 0.9 and "id" in cell:
        cell["id"] = "%08x" % rng.getrandbits(32)
    elif r < 0.95 and "attachments" in cell:
        name = rng.choice(sorted(cell["attachments"]))
        if rng.random() < 0.5:
            cell["attachments"][name] = {"image/png": _b64(rng, 48)}
        else:
            del cell["attachments"][name]
    else:
        cell["source"] = cell["source"] + "\n" + random_string(rng, None, 20)
    return cell


def mutate_notebook(rng, nb, intensity=None):
    nb = copy.deepcopy(nb)
    cells = nb["cells"]
    ids = any("id" in c for c in cells) or nb.get("nbformat_minor", 0) >= 5
    for _ in range(intensity if intensity is not None else rng.randint(0, 4)):
        q = rng.random()
        if q < 0.55 and cells:
            i = rng.randrange(len(cells))
            cells[i] = mutate_cell(rng, cells[i])
        elif q < 0.7:
            cells.insert(rng.randint(0, len(cells)), random_cell(rng, ids, rng.randint(1, 99)))
        elif q < 0.82 and cells:
            del cells[rng.randrange(len(cells))]
        elif q < 0.9 and len(cells) > 1:
            i, j = rng.randrange(len(cells)), rng.randrange(len(cells))
            cells.insert(j, cells.pop(i))
        elif q < 0.95 and cells:
            i = rng.randrange(len(cells))
            cells.insert(i, copy.deepcopy(cells[i]))
        else:
            nb["metadata"]["language_info"]["version"] = "3.13.0"
            if rng.random() < 0.5:
                nb["metadata"]["new_key"] = {"x": [1, 2]}
    return nb


def to_disk_form(nb):
    """Split multi-line strings into lists of lines, as nbformat writes them."""
    nb = copy.deepcopy(nb)
    for cell in nb["cells"]:
        if isinstance(cell.get("source"), str):
            cell["source"] = cell["source"].splitlines(True)
        for att in (cell.get("attachments") or {}).values():
            _split_bundle(att)
        for out in cell.get("outputs", []):
            if out["output_type"] in ("execute_result", "display_data"):
                _split_bundle(out.get("data", {}))
            elif out["output_type"] == "stream" and isinstance(out.get("text"), str):
                out["text"] = out["text"].splitlines(True)
    return nb


def _split_bundle(bundle):
    for k, v in list(bundle.items()):
        if isinstance(v, str) and not (k == "application/json" or (k.startswith("application/") and k.endswith("+json"))):
            bundle[k] = v.splitlines(True)


def dumps_disk(nb, rng=None):
    """Serialize like nbformat.writes (indent 1, sorted keys), sometimes in other layouts."""
    style = rng.random() if rng else 0.0
    if style < 0.7:
        return json.dumps(to_disk_form(nb), indent=1, sort_keys=True, ensure_ascii=False) + "\n"
    if style < 0.85:
        return json.dumps(nb, ensure_ascii=True)
    return json.dumps(to_disk_form(nb), indent=2)
