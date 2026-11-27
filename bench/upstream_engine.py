"""Time nbdime's notebook diff and merge with and without the optional Rust engine.

Run from an nbdime checkout with docs/upstream/optional-rust-engine.patch applied and the
nbdime_rs wheel installed:

    python /path/to/nbdime-rs/bench/upstream_engine.py [--repeat 3]

Notebooks come from bench/gen_notebooks.py (seed 0, about 5% of cells edited on each side).
Each figure is the median of --repeat in-process calls; the first call is not timed.
"""
import argparse
import json
import os
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import nbformat

HERE = Path(__file__).resolve().parent
CASES = [(100, True), (400, True), (1600, True), (400, False), (1600, False)]


def timed(func, args, repeat):
    func(*args)
    samples = []
    for _ in range(repeat):
        start = time.perf_counter()
        func(*args)
        samples.append(time.perf_counter() - start)
    return statistics.median(samples)


def run_mode(mode, nbdir, repeat):
    code = f"""
import json, sys
sys.path.insert(0, {str(HERE)!r})
from upstream_engine import timed
import nbformat
from nbdime import diff_notebooks, merge_notebooks
nbs = [nbformat.read({str(nbdir)!r} + '/' + n + '.ipynb', as_version=4) for n in ('base', 'local', 'remote')]
print(json.dumps({{
    'diff': timed(diff_notebooks, nbs[:2], {repeat}),
    'merge': timed(merge_notebooks, nbs, {repeat}),
}}))
"""
    env = dict(os.environ, NBDIME_ACCEL=mode)
    out = subprocess.run([sys.executable, "-c", code], env=env, check=True,
                         capture_output=True, text=True).stdout
    return json.loads(out.strip().splitlines()[-1])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repeat", type=int, default=3)
    opts = parser.parse_args()
    results = []
    with tempfile.TemporaryDirectory() as tmp:
        for cells, ids in CASES:
            nbdir = Path(tmp) / f"{cells}-{ids}"
            subprocess.run([sys.executable, str(HERE / "gen_notebooks.py"), str(nbdir), str(cells),
                            "--seed", "0"] + ([] if ids else ["--no-ids"]), check=True)
            python = run_mode("0", nbdir, opts.repeat)
            engine = run_mode("force", nbdir, opts.repeat)
            row = {"cells": cells, "ids": ids, "python": python, "engine": engine}
            results.append(row)
            print(json.dumps(row), flush=True)


if __name__ == "__main__":
    main()
