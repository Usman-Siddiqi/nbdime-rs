import os
import sys
from pathlib import Path

import pytest

# The reference venv may carry unrelated pytest plugins; parity needs none of them.
pytest_plugins = []

sys.path.insert(0, str(Path(__file__).resolve().parent))

# Upstream renders text diffs and merges with the installed git; isolate it from user and system
# configuration so the oracle sees git's defaults (findings F-022).
os.environ["GIT_CONFIG_NOSYSTEM"] = "1"
os.environ["GIT_CONFIG_GLOBAL"] = os.devnull

from harness import ROOT, Engine  # noqa: E402

_skipped = []


@pytest.fixture(scope="session")
def engine():
    exe = ".exe" if os.name == "nt" else ""
    binary = Path(os.environ.get("NBDIME_RS_DEV",
                                 ROOT / "target" / "release" / ("nbdime-rs-dev" + exe)))
    if not binary.exists():
        raise RuntimeError("engine binary not found: %s (cargo build --release -p nbdime-cli)" % binary)
    eng = Engine(binary)
    yield eng
    eng.close()


def pytest_runtest_logreport(report):
    if report.skipped:
        _skipped.append(report.nodeid)


def pytest_sessionfinish(session, exitstatus):
    if _skipped:
        sys.stderr.write("\nParity tests must not be skipped or xfailed: %s\n" % ", ".join(_skipped))
        session.exitstatus = 1
