import os
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

from harness import ROOT, Engine  # noqa: E402

_skipped = []


@pytest.fixture(scope="session")
def engine():
    binary = Path(os.environ.get("NBDIME_RS_DEV", ROOT / "target" / "release" / "nbdime-rs-dev"))
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
