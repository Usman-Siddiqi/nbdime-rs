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
import sysconfig
import textwrap
import threading
import time
from collections import OrderedDict

import pytest

from harness import ROOT

EXE = ".exe" if os.name == "nt" else ""
DEV = os.environ.get("NBDIME_RS_DEV", str(ROOT / "target" / "release" / ("nbdime-rs-dev" + EXE)))
SUITES = ["test_pilot_generic_diff.py", "test_u05_patch.py", "test_u04_notebook_diff.py",
          "test_u10_merge.py"]


@pytest.fixture(scope="module")
def ext(tmp_path_factory):
    env = dict(os.environ, PYO3_PYTHON=sys.executable)
    subprocess.run(["cargo", "build", "--release", "-p", "nbdime-py"], cwd=ROOT, env=env,
                   check=True)
    built, module = {"darwin": ("libnbdime_rs.dylib", "nbdime_rs.so"),
                     "win32": ("nbdime_rs.dll", "nbdime_rs.pyd")}.get(
        sys.platform, ("libnbdime_rs.so", "nbdime_rs.so"))
    lib = ROOT / "target" / "release" / built
    pkg = tmp_path_factory.mktemp("pyext")
    shutil.copy(lib, pkg / module)
    sys.path.insert(0, str(pkg))
    import nbdime_rs
    return pkg, nbdime_rs


def test_u15_suites_through_the_python_api(ext, tmp_path):
    pkg, _ = ext
    stats = tmp_path / "stats.jsonl"
    args = (sys.executable, ROOT / "parity" / "tools" / "pyext_engine.py", DEV)
    if os.name == "nt":
        wrapper = tmp_path / "engine.cmd"
        wrapper.write_text('@"%s" "%s" "%s" %%*\n' % args)
    else:
        wrapper = tmp_path / "engine"
        wrapper.write_text("#!/bin/sh\nexec %s %s %s \"$@\"\n" % args)
        wrapper.chmod(0o755)
    env = dict(os.environ, NBDIME_RS_DEV=str(wrapper), NBDIME_RS_PYEXT_STATS=str(stats),
               PYTHONPATH=str(pkg) + os.pathsep + os.environ.get("PYTHONPATH", ""))
    p = subprocess.run([sys.executable, "-m", "pytest", "-p", "no:cacheprovider"] + SUITES,
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


def test_u15_rust_checks():
    env = dict(os.environ, PYO3_PYTHON=sys.executable)
    libdir = sysconfig.get_config_var("LIBDIR")
    if libdir:
        for name in ("LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH"):
            env[name] = os.pathsep.join(filter(None, (libdir, env.get(name))))
    subprocess.run(["cargo", "clippy", "-p", "nbdime-py", "--all-targets", "--", "-D", "warnings"],
                   cwd=ROOT, env=env, check=True)
    subprocess.run(["cargo", "test", "-p", "nbdime-py", "--no-default-features"],
                   cwd=ROOT, env=env, check=True)


def test_u15_conversion_round_trip(ext):
    _, m = ext
    values = [None, True, False, 0, -1, 2**63, -(2**80), 10**400, 1.5, -0.0, 1e308, "", "é\U0001F600",
              [], {}, {"b": 1, "a": [1, 2.0, True, None]}, [[[]]], {"": {"x": "y"}}]
    # patch() itself only takes dict, list or str (as upstream), so scalars travel inside a dict.
    for v in values:
        out = m.patch({"v": v}, [])["v"]
        assert json.dumps(out) == json.dumps(v)
        assert type(out) is type(v)
    for v in (float("inf"), float("-inf")):
        assert m.patch({"v": v}, [])["v"] == v
    assert math.isnan(m.patch({"v": float("nan")}, [])["v"])
    with pytest.raises(m.EngineError):
        m.patch(None, [])
    # Key order of dicts is kept, bool stays bool, int stays int.
    out = m.patch({"z": 1, "a": True, "m": 1.0}, [])
    assert list(out) == ["z", "a", "m"]
    assert [type(x) for x in out.values()] == [int, bool, float]


def test_u15_circular_references_and_shared_values(ext):
    _, m = ext
    circular = {str(i): i for i in range(32)}
    circular["self"] = [circular]
    loop = []
    loop.append(loop)
    for value in (circular, loop):
        with pytest.raises(ValueError, match="^Circular reference detected$"):
            m.diff(value, {})
    child = {"nested": [1]}
    shared = {"a": child, "b": [child, child]}
    assert m.patch(shared, []) == shared


def test_u15_container_subclass_protocols(ext):
    _, m = ext

    class Reversed(list):
        def __iter__(self):
            return reversed(self)

    ordered = OrderedDict([("a", 1), ("b", Reversed([2, 3])), ("c", 4)])
    ordered.move_to_end("a")
    assert json.dumps(m.patch(ordered, [])) == json.dumps(ordered)


def test_u15_dict_size_change_raises_runtime_error(ext):
    _, m = ext

    class MutatingList(list):
        def __iter__(self):
            source["new"] = None
            return super().__iter__()

    source = {"list": MutatingList([0])}
    with pytest.raises(RuntimeError, match="dictionary changed size during iteration"):
        m.diff(source, {})


def test_u15_patch_from_a_small_stack(ext):
    pkg, _ = ext
    env = dict(os.environ, PYTHONPATH=str(pkg))
    script = textwrap.dedent("""\
        import threading
        import nbdime_rs

        def patch():
            source = {}
            for _ in range(23):
                source = {"a": source}
            results.append(nbdime_rs.patch(source, []) == source)

        results = []
        threading.stack_size(32 * 1024)
        thread = threading.Thread(target=patch)
        thread.start()
        thread.join()
        assert results == [True], results
        """)
    result = subprocess.run([sys.executable, "-c", script], env=env, capture_output=True,
                            text=True, timeout=20)
    assert result.returncode == 0, result.stdout + result.stderr


@pytest.mark.parametrize("keys", [["ignored"], ("ignored",), {"ignored"}])
def test_u15_ignores_accept_upstream_key_collections(ext, keys):
    _, m = ext
    base = {"cells": [], "metadata": {"ignored": 1, "shown": 1}, "nbformat": 4, "nbformat_minor": 5}
    remote = dict(base, metadata={"ignored": 2, "shown": 2})
    expected = [{"op": "patch", "key": "metadata", "diff": [
        {"op": "replace", "key": "shown", "value": 2},
    ]}]
    assert m.diff_notebooks(base, remote, ignores={"/metadata": keys}) == expected
    assert m.diff_notebooks(base, remote, ignores={"/metadata": False}) != expected


def test_u15_option_errors_name_python_types_and_unrecognized_args(ext):
    _, m = ext
    nb = {"cells": [], "metadata": {}, "nbformat": 4, "nbformat_minor": 5}
    with pytest.raises(m.EngineError, match="ignores must be a dict"):
        m.diff_notebooks(nb, nb, ignores=[])
    with pytest.raises(m.EngineError, match="unexpected merge argument"):
        m.merge_notebooks(nb, nb, nb, {"merge_stratgey": "union"})


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
    with pytest.raises(m.EngineError):
        m.decide_merge("", "", "")  # an AssertionError upstream
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
