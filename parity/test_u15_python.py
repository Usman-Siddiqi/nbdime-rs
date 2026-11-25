"""U15: the `nbdime_rs` Python extension.

1. Builds the extension (`cargo build --release -p nbdime-py` against this interpreter) and
   imports it as the single module `nbdime_rs`.
2. Re-runs the generic diff, patch, notebook diff and merge suites with the engine answered by
   the extension (`tools/pyext_engine.py`): same cases, same oracle, through the Python API.
3. Checks the Python boundary itself: module attributes, object conversion (types the engine
   accepts and the ones it must reject with TypeError), errors, and the GIL being released.
"""

import json
import math
import os
import shutil
import subprocess
import sys
import threading
import time

import pytest

from harness import ROOT

DEV = os.environ.get("NBDIME_RS_DEV", str(ROOT / "target" / "release" / "nbdime-rs-dev"))
SUITES = ["test_pilot_generic_diff.py", "test_u05_patch.py", "test_u04_notebook_diff.py",
          "test_u10_merge.py"]


@pytest.fixture(scope="module")
def ext(tmp_path_factory):
    env = dict(os.environ, PYO3_PYTHON=sys.executable)
    subprocess.run(["cargo", "build", "--release", "-p", "nbdime-py"], cwd=ROOT, env=env,
                   check=True)
    lib = ROOT / "target" / "release" / ("libnbdime_rs.dylib" if sys.platform == "darwin"
                                         else "libnbdime_rs.so")
    pkg = tmp_path_factory.mktemp("pyext")
    shutil.copy(lib, pkg / "nbdime_rs.so")
    sys.path.insert(0, str(pkg))
    import nbdime_rs
    return pkg, nbdime_rs


def test_u15_suites_through_the_python_api(ext, tmp_path):
    pkg, _ = ext
    stats = tmp_path / "stats.jsonl"
    wrapper = tmp_path / "engine"
    wrapper.write_text("#!/bin/sh\nexec %s %s %s \"$@\"\n" % (
        sys.executable, ROOT / "parity" / "tools" / "pyext_engine.py", DEV))
    wrapper.chmod(0o755)
    env = dict(os.environ, NBDIME_RS_DEV=str(wrapper), NBDIME_RS_PYEXT_STATS=str(stats),
               PYTHONPATH=str(pkg) + os.pathsep + os.environ.get("PYTHONPATH", ""))
    p = subprocess.run([sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider"] + SUITES,
                       cwd=ROOT / "parity", env=env, capture_output=True, text=True)
    assert p.returncode == 0, p.stdout[-6000:] + p.stderr[-2000:]
    assert " passed" in p.stdout and " skipped" not in p.stdout, p.stdout[-2000:]
    counts = {}
    for line in stats.read_text().splitlines():
        for fn, n in json.loads(line).items():
            counts[fn] = counts.get(fn, 0) + n
    for fn in ("diff", "patch", "diff_notebooks", "decide_merge", "apply_decisions",
               "decide_notebook_merge", "merge_notebooks"):
        assert counts.get(fn, 0) >= 50, "the extension answered only %r" % counts


def test_u15_module_attributes(ext):
    _, m = ext
    assert m.NBDIME_COMPAT == "4.0.4"
    assert isinstance(m.ENGINE_VERSION, str) and m.ENGINE_VERSION
    assert issubclass(m.EngineError, Exception)


def test_u15_conversion_round_trip(ext):
    _, m = ext
    values = [None, True, False, 0, -1, 2**63, -(2**80), 10**400, 1.5, -0.0, 1e308, "", "é\U0001F600",
              [], {}, {"b": 1, "a": [1, 2.0, True, None]}, [[[]]], {"": {"x": "y"}}]
    for v in values:
        out = m.patch(v, [])
        assert json.dumps(out) == json.dumps(v)
        assert type(out) is type(v)
    for v in (float("inf"), float("-inf")):
        assert m.patch(v, []) == v
    assert math.isnan(m.patch(float("nan"), []))
    # Key order of dicts is kept, bool stays bool, int stays int.
    out = m.patch({"z": 1, "a": True, "m": 1.0}, [])
    assert list(out) == ["z", "a", "m"]
    assert [type(x) for x in out.values()] == [int, bool, float]


@pytest.mark.parametrize("bad", [b"bytes", (1, 2), {1, 2}, {1: "x"}, object(), [1, object()],
                                 {"a": {"b": frozenset()}}, 1j])
def test_u15_unsupported_types_raise_type_error(ext, bad):
    _, m = ext
    with pytest.raises(TypeError):
        m.diff(bad, None)
    with pytest.raises(TypeError):
        m.diff(None, bad)


def test_u15_engine_errors(ext):
    _, m = ext
    with pytest.raises(m.EngineError):
        m.patch({"a": 1}, [{"op": "patch", "key": "missing", "diff": []}])
    with pytest.raises((m.EngineError, ValueError, TypeError)):
        m.diff_notebooks({"cells": "nope"}, {"cells": []})
    with pytest.raises(TypeError):
        m.diff(1)


def test_u15_gil_is_released(ext):
    _, m = ext
    a = {"cells": [{"cell_type": "code", "source": "x = %d\n" % i * 20, "metadata": {},
                    "outputs": [], "execution_count": None} for i in range(1500)],
         "metadata": {}, "nbformat": 4, "nbformat_minor": 4}
    b = json.loads(json.dumps(a))
    for c in b["cells"][::7]:
        c["source"] += "y = 1\n"
    start = time.perf_counter()
    m.diff_notebooks(a, b)
    single = time.perf_counter() - start
    ticks = []
    stop = threading.Event()

    def ticker():
        while not stop.is_set():
            ticks.append(time.perf_counter())
            time.sleep(0.001)
    t = threading.Thread(target=ticker)
    t.start()
    m.diff_notebooks(a, b)
    stop.set()
    t.join()
    if single > 0.05:
        assert len(ticks) >= 0.3 * single / 0.002, (single, len(ticks))
