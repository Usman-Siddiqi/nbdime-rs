"""Targeted benchmarks for the Phase 4 hot spots, end to end through `nbdiff`.

usage: python bench/micro.py [--engines rs-cli,upstream-cli] [--repeat 3] [--timeout 900]
                             [--rs-bin PATH] [--cases NAME,...] [--out bench/results/NAME.json]

Cases:
  img-add        an empty notebook against 100 cells with a 200 kB PNG each, diff written with --out
  img-identical  that notebook against an identical copy
  reindent-N     one N-line code cell against the same lines indented by four spaces, without
                 cell ids (so the cells are aligned by comparing their sources); `lines` are
                 generated statements, `repeat` is one statement repeated

Run with the reference venv's python. Results include the hardware and software versions;
never edit them by hand.
"""

import argparse
import datetime
import json
import random
import shutil
import string
import subprocess
import sys
from pathlib import Path

from run import HERE, ROOT, hardware, time_cmd, versions

WORK = HERE / ".work" / "micro"


def notebook(cells, minor=5):
    return {"cells": cells, "metadata": {}, "nbformat": 4, "nbformat_minor": minor}


def code_cell(source, outputs=(), cell_id=None):
    cell = {"cell_type": "code", "execution_count": None, "metadata": {}, "outputs": list(outputs),
            "source": source}
    if cell_id is not None:
        cell["id"] = cell_id
    return cell


def statement(rng):
    name = "".join(rng.choices(string.ascii_lowercase, k=rng.randint(3, 8)))
    kind = rng.random()
    if kind < 0.4:
        return f"{name} = compute_{name}(x, y={rng.randint(0, 999)})"
    if kind < 0.7:
        return f"{name}.append(i * {rng.random():.4f})"
    return f"print({name!r}, {rng.randint(0, 10**6)})"


def write(path, nb):
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w") as f:
        json.dump(nb, f, indent=1)


def make_cases(names):
    rng = random.Random(1)
    cases = {}
    if any(n.startswith("img-") for n in names):
        png = lambda: "".join(rng.choices(string.ascii_letters + string.digits + "+/", k=200_000))
        cells = [code_cell("plot()", [{"output_type": "display_data", "metadata": {},
                                        "data": {"image/png": png(), "text/plain": "<Figure>"}}],
                           "c%d" % i) for i in range(100)]
        write(WORK / "img.ipynb", notebook(cells))
        shutil.copy(WORK / "img.ipynb", WORK / "img-copy.ipynb")
        write(WORK / "empty.ipynb", notebook([]))
        cases["img-add"] = (WORK / "empty.ipynb", WORK / "img.ipynb", ["--out", str(WORK / "img-add.json")])
        cases["img-identical"] = (WORK / "img.ipynb", WORK / "img-copy.ipynb", [])
    for name in names:
        if not name.startswith("reindent-"):
            continue
        _, kind, n = name.split("-")
        lines = [statement(rng) if kind == "lines" else "x = x + 1" for _ in range(int(n))]
        a, b = WORK / (name + "-a.ipynb"), WORK / (name + "-b.ipynb")
        write(a, notebook([code_cell("\n".join("    " + line for line in lines))], minor=4))
        write(b, notebook([code_cell("\n".join(lines))], minor=4))
        cases[name] = (a, b, [])
    return {n: cases[n] for n in names}


def command(engine, rs_bin):
    if engine == "upstream-cli":
        return [shutil.which("nbdiff", path=str(Path(sys.executable).parent)) or "nbdiff"]
    if engine == "rs-cli":
        return [rs_bin]
    raise ValueError(engine)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--cases", default="img-add,img-identical,reindent-lines-1000,"
                                      "reindent-repeat-250,reindent-repeat-500,reindent-repeat-1000")
    p.add_argument("--engines", default="rs-cli")
    p.add_argument("--repeat", type=int, default=3)
    p.add_argument("--timeout", type=int, default=900)
    p.add_argument("--rs-bin", default=str(ROOT / "target" / "release" / "nbdiff"))
    p.add_argument("--out")
    a = p.parse_args()

    results = {"date": datetime.datetime.now().isoformat(timespec="seconds"), "hardware": hardware(),
               "versions": versions(), "rs_bin": a.rs_bin, "runs": []}
    for name, (base, remote, extra) in make_cases(a.cases.split(",")).items():
        for engine in a.engines.split(","):
            cmd = command(engine, a.rs_bin) + ["--no-color", str(base), str(remote)] + extra
            times = []
            for _ in range(a.repeat):
                try:
                    times.append(time_cmd(cmd, a.timeout))
                except subprocess.TimeoutExpired:
                    times.append(None)
                    break
            ok = [t for t in times if t is not None]
            row = {"case": name, "engine": engine, "times": times, "best": min(ok) if ok else None,
                   "timed_out": len(ok) < len(times)}
            results["runs"].append(row)
            print("%-22s %-14s best=%s" % (name, engine, "%.3fs" % row["best"] if ok else "timeout"),
                  flush=True)
    out = Path(a.out) if a.out else HERE / "results" / ("%s-micro.json" % datetime.date.today().isoformat())
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(results, indent=1) + "\n")
    print("wrote", out)


if __name__ == "__main__":
    main()
