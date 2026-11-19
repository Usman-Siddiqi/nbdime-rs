"""Generate deterministic synthetic notebook pairs/triples for benchmarking.

Usage: python gen_notebooks.py OUTDIR N [--seed S] [--ids/--no-ids]

Writes OUTDIR/base.ipynb, local.ipynb, remote.ipynb with N cells. local and
remote each apply ~5% edits (source edits, output changes, inserts, deletes,
moves) to base, using disjoint cells so the merge is mostly conflict-free.
"""
import argparse
import json
import random
import string
import uuid


def words(rng, n):
    return " ".join("".join(rng.choices(string.ascii_lowercase, k=rng.randint(2, 9))) for _ in range(n))


def code_source(rng):
    lines = []
    for _ in range(rng.randint(2, 12)):
        name = "".join(rng.choices(string.ascii_lowercase, k=rng.randint(3, 8)))
        kind = rng.random()
        if kind < 0.4:
            lines.append(f"{name} = compute_{name}(x, y={rng.randint(0, 999)})")
        elif kind < 0.7:
            lines.append(f"for i in range({rng.randint(1, 50)}):\n    {name}.append(i * {rng.random():.4f})")
        else:
            lines.append(f"print({name!r}, {rng.randint(0, 10**6)})")
    return "\n".join(lines)


def markdown_source(rng):
    return "## " + words(rng, 4) + "\n\n" + words(rng, rng.randint(10, 40))


def outputs(rng, count):
    outs = []
    for _ in range(rng.randint(0, 2)):
        r = rng.random()
        if r < 0.5:
            outs.append({"output_type": "stream", "name": "stdout",
                         "text": "\n".join(words(rng, 6) for _ in range(rng.randint(1, 6))) + "\n"})
        elif r < 0.85:
            outs.append({"output_type": "execute_result", "execution_count": count, "metadata": {},
                         "data": {"text/plain": f"array([{', '.join(str(rng.randint(0, 99)) for _ in range(8))}])"}})
        else:
            outs.append({"output_type": "display_data", "metadata": {},
                         "data": {"text/plain": "<Figure size 640x480 with 1 Axes>",
                                  "image/png": "".join(rng.choices(string.ascii_letters + string.digits + "+/", k=2048))}})
    return outs


def new_cell(rng, count, ids):
    if rng.random() < 0.25:
        c = {"cell_type": "markdown", "metadata": {}, "source": markdown_source(rng)}
    else:
        c = {"cell_type": "code", "execution_count": count, "metadata": {},
             "outputs": outputs(rng, count), "source": code_source(rng)}
    if ids:
        c["id"] = uuid.UUID(int=rng.getrandbits(128)).hex[:8]
    return c


def notebook(cells, minor):
    return {"cells": cells, "metadata": {"kernelspec": {"display_name": "Python 3", "language": "python", "name": "python3"},
                                         "language_info": {"name": "python", "version": "3.11.0"}},
            "nbformat": 4, "nbformat_minor": minor}


def mutate(rng, cells, frac, ids, touched):
    cells = json.loads(json.dumps(cells))
    n = len(cells)
    k = max(1, int(n * frac))
    candidates = [i for i in range(n) if i not in touched]
    picks = rng.sample(candidates, min(k, len(candidates)))
    touched.update(picks)
    deletes = set()
    for i in picks:
        r = rng.random()
        c = cells[i]
        if r < 0.45:
            lines = c["source"].split("\n")
            j = rng.randrange(len(lines))
            lines[j] = lines[j] + "  # edited " + words(rng, 2)
            c["source"] = "\n".join(lines)
        elif r < 0.65 and c["cell_type"] == "code":
            c["execution_count"] = (c.get("execution_count") or 0) + 100
            c["outputs"] = outputs(rng, c["execution_count"])
        elif r < 0.8:
            deletes.add(i)
        else:
            c["source"] = c["source"] + "\n" + (code_source(rng) if c["cell_type"] == "code" else words(rng, 8))
    out = []
    for i, c in enumerate(cells):
        if i in deletes:
            continue
        out.append(c)
        if rng.random() < frac / 3:
            out.append(new_cell(rng, 1000 + i, ids))
    return out


def main():
    p = argparse.ArgumentParser()
    p.add_argument("outdir")
    p.add_argument("n", type=int)
    p.add_argument("--seed", type=int, default=0)
    p.add_argument("--no-ids", action="store_true")
    p.add_argument("--frac", type=float, default=0.05)
    a = p.parse_args()
    ids = not a.no_ids
    rng = random.Random(a.seed * 100003 + a.n)
    base = [new_cell(rng, i + 1, ids) for i in range(a.n)]
    touched = set()
    local = mutate(rng, base, a.frac, ids, touched)
    remote = mutate(rng, base, a.frac, ids, touched)
    import os
    os.makedirs(a.outdir, exist_ok=True)
    minor = 5 if ids else 4
    for name, cells in (("base", base), ("local", local), ("remote", remote)):
        with open(os.path.join(a.outdir, name + ".ipynb"), "w", encoding="utf-8") as f:
            json.dump(notebook(cells, minor), f, indent=1, ensure_ascii=False)
            f.write("\n")


if __name__ == "__main__":
    main()
