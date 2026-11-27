"""Upstream nbdime's own CLI and git tests, run against the nbdime-rs binaries.

Each file in UPSTREAM_FILES is run twice in a subprocess with an isolated HOME and git
configuration: once as is (the baseline) and once with `upstream_shim` replacing every entry point
by our binaries. Every test that passes in the baseline must pass with the shim, except those in
NOT_APPLICABLE, each with the reason it cannot apply to a separate binary (out-of-scope web UI,
in-process state of the Python package). Nothing else may be excluded.
"""

import os
import re
import subprocess
import sys

from harness import ROOT, UPSTREAM

REF_PY = os.environ.get("NBDIME_REF_PYTHON", "/home/claude/ref/venv/bin/python")
OUR_BIN = os.environ.get("NBDIME_RS_BIN", str(ROOT / "target" / "release"))

UPSTREAM_FILES = [
    "nbdime/tests/test_cli_apps.py",
    "nbdime/tests/test_git_diffdriver.py",
    "nbdime/tests/test_git_filter_integration.py",
]

NOT_APPLICABLE = {
    "nbdime/tests/test_git_diffdriver.py::test_git_web_diff_driver":
        "web UI (nbdiff-web) is out of scope",
    # Both drive `git difftool`/`git mergetool` into nbdime's web tools and poll their HTTP
    # server; they pass locally only when upstream's own `nbdime` answers on PATH.
    "nbdime/tests/test_cli_apps.py::test_git_difftool": "web UI (nbdiff-web) is out of scope",
    "nbdime/tests/test_cli_apps.py::test_git_mergetool": "web UI (nbmerge-web) is out of scope",
}

RESULT = re.compile(r"^(PASSED|FAILED|ERROR|SKIPPED|XFAIL|XPASS) (\S+)", re.M)


def _run(tmp_path, shim):
    home = tmp_path / ("home-shim" if shim else "home-base")
    home.mkdir()
    gitconfig = home / ".gitconfig"
    gitconfig.write_text("[user]\n\tname = t\n\temail = t@example.com\n")
    env = {
        "PATH": os.environ["PATH"],
        "HOME": str(home),
        "JUPYTER_CONFIG_DIR": str(home / "jupyter"),
        "JUPYTER_PLATFORM_DIRS": "0",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": str(gitconfig),
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "TZ": "UTC",
        "PYTHONDONTWRITEBYTECODE": "1",
        "PYTHONPATH": str(ROOT / "parity"),
        "NBDIME_RS_BIN": OUR_BIN,
    }
    # Windows processes need these to start at all (Python's site and sockets, git, the C runtime).
    for name in ("SYSTEMROOT", "SYSTEMDRIVE", "COMSPEC", "PATHEXT", "TEMP", "TMP", "WINDIR"):
        if os.name == "nt" and name in os.environ:
            env[name] = os.environ[name]
    args = [REF_PY, "-m", "pytest", "-p", "no:cacheprovider", "-q", "-rA",
            "--basetemp", str(tmp_path / ("tmp-shim" if shim else "tmp-base"))]
    if shim:
        args += ["-p", "upstream_shim"]
    p = subprocess.run(args + UPSTREAM_FILES, cwd=UPSTREAM, env=env, capture_output=True,
                       text=True, timeout=1800)
    results = {}
    for outcome, nodeid in RESULT.findall(p.stdout):
        results[nodeid] = outcome
    assert results, "no results parsed:\n" + p.stdout[-3000:] + p.stderr[-3000:]
    return results, p.stdout


def test_upstream_cli_and_git_tests(tmp_path):
    base, _ = _run(tmp_path, shim=False)
    ours, out = _run(tmp_path, shim=True)
    passing = sorted(t for t, o in base.items() if o == "PASSED")
    assert len(passing) >= 40, "baseline ran only %d passing tests" % len(passing)
    unknown = set(NOT_APPLICABLE) - set(base)
    assert not unknown, "NOT_APPLICABLE names tests that do not exist: %s" % sorted(unknown)
    broken = [t for t in passing if t not in NOT_APPLICABLE and ours.get(t) != "PASSED"]
    if broken:
        details = []
        for t in broken[:5]:
            name = t.split("::")[-1]
            m = re.search(r"_{5,} %s _{5,}\n(.*?)(?=\n_{5,} |\n=+ )" % re.escape(name), out, re.S)
            details.append("%s: %s\n%s" % (t, ours.get(t), (m.group(1)[-2500:] if m else "")))
        raise AssertionError("%d of %d upstream tests fail against nbdime-rs:\n%s\n\n%s" % (
            len(broken), len(passing), "\n".join(broken), "\n\n".join(details)))
    print("%d upstream tests pass against nbdime-rs (%d not applicable)" % (
        len(passing) - len(NOT_APPLICABLE), len(NOT_APPLICABLE)))
