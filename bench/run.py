"""Benchmark nbdime (upstream) against nbdime-rs on generated notebooks.

usage: python bench/run.py [--sizes 100,200,400] [--variants ids,noids] [--engines ...]
                           [--repeat 3] [--timeout 900] [--out bench/results/NAME.json]

Engines:
  upstream-inproc  nbdime.diff_notebooks only (no start-up, no printing)
  upstream-cli     `nbdiff --no-color A B` end to end
  rs-cli           `nbdiff` from nbdime-rs end to end (when built)

Run with the reference venv's python (it needs nbdime and nbformat). Results include the
hardware and software versions; never edit them by hand.
"""

import argparse
import datetime
import json
import os
import platform
import shutil
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
WORK = HERE / ".work"


def hardware():
    info = {"machine": platform.machine(), "cpus": os.cpu_count(), "platform": platform.platform()}
    try:
        for line in open("/proc/cpuinfo"):
            if line.startswith("model name"):
                info["cpu"] = line.split(":", 1)[1].strip()
                break
        for line in open("/proc/meminfo"):
            if line.startswith("MemTotal"):
                info["mem_kb"] = int(line.split()[1])
                break
    except OSError:
        pass
    return info


def versions():
    import nbdime
    import nbformat
    v = {"python": platform.python_version(), "nbdime": nbdime.__version__, "nbformat": nbformat.__version__}
    try:
        v["nbdime_rs_git"] = subprocess.check_output(["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"],
                                                     text=True).strip()
    except Exception:
        pass
    return v


def generate(n, variant):
    d = WORK / ("%s-%d" % (variant, n))
    if not (d / "local.ipynb").exists():
        args = [sys.executable, str(HERE / "gen_notebooks.py"), str(d), str(n)]
        if variant == "noids":
            args.append("--no-ids")
        subprocess.check_call(args)
    return d / "base.ipynb", d / "local.ipynb"


def time_cmd(cmd, timeout):
    t = time.perf_counter()
    p = subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=timeout)
    elapsed = time.perf_counter() - t
    if p.returncode not in (0, 1):
        raise RuntimeError("%s failed: %s" % (cmd, p.stderr.decode()[-500:]))
    return elapsed


def run_engine(engine, a, b, timeout, rs_bin):
    if engine == "upstream-inproc":
        out = subprocess.run([sys.executable, str(HERE / "_upstream_inproc.py"), str(a), str(b)],
                             capture_output=True, text=True, timeout=timeout, check=True).stdout
        return json.loads(out)["seconds"]
    if engine == "upstream-cli":
        nbdiff = shutil.which("nbdiff", path=str(Path(sys.executable).parent)) or "nbdiff"
        return time_cmd([nbdiff, "--no-color", str(a), str(b)], timeout)
    if engine == "rs-cli":
        return time_cmd([rs_bin, "--no-color", str(a), str(b)], timeout)
    raise ValueError(engine)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--sizes", default="100,200,400,800")
    p.add_argument("--variants", default="ids,noids")
    p.add_argument("--engines", default="upstream-inproc,upstream-cli")
    p.add_argument("--repeat", type=int, default=3)
    p.add_argument("--timeout", type=int, default=900)
    p.add_argument("--rs-bin", default=str(ROOT / "target" / "release" / "nbdiff"))
    p.add_argument("--out")
    a = p.parse_args()

    results = {"date": datetime.datetime.now().isoformat(timespec="seconds"), "hardware": hardware(),
               "versions": versions(), "runs": []}
    for variant in a.variants.split(","):
        for n in map(int, a.sizes.split(",")):
            base, local = generate(n, variant)
            for engine in a.engines.split(","):
                times = []
                for _ in range(a.repeat):
                    try:
                        times.append(run_engine(engine, base, local, a.timeout, a.rs_bin))
                    except subprocess.TimeoutExpired:
                        times.append(None)
                        break
                ok = [t for t in times if t is not None]
                row = {"variant": variant, "cells": n, "engine": engine, "times": times,
                       "best": min(ok) if ok else None, "timed_out": len(ok) < len(times)}
                results["runs"].append(row)
                print("%-6s %5d %-16s best=%s" % (variant, n, engine,
                                                  "%.3fs" % row["best"] if ok else "timeout"), flush=True)
    out = Path(a.out) if a.out else HERE / "results" / ("%s.json" % datetime.date.today().isoformat())
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(results, indent=1) + "\n")
    print("wrote", out)


if __name__ == "__main__":
    main()
