"""Engine driver and comparison helpers for the parity suites."""

import json
import os
import subprocess
import sys
import threading
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
UPSTREAM = Path(os.environ.get("NBDIME_UPSTREAM", "/home/claude/ref/nbdime-upstream"))
SCALE = float(os.environ.get("PARITY_SCALE", "1"))
SEED = int(os.environ.get("PARITY_SEED", "0"))


def scaled(n):
    return max(1, int(n * SCALE))


def canon(value):
    """Canonical JSON text: types, list order and key order are all significant."""
    return json.dumps(value, separators=(",", ":"), ensure_ascii=True, allow_nan=True)


class EngineError(Exception):
    pass


class Engine:
    """A long-running `nbdime-rs-dev batch` process."""

    def __init__(self, binary):
        self.binary = str(binary)
        self.proc = subprocess.Popen(
            [self.binary, "batch"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=None,
            text=True,
            encoding="utf-8",
            bufsize=1,
        )
        self.next_id = 0

    def call(self, fn, args):
        return self.call_many(fn, [args])[0]

    def call_many(self, fn, arglist, chunk=500):
        """Returns one (ok, result_or_error) tuple per args, in order."""
        out = []
        for start in range(0, len(arglist), chunk):
            batch = arglist[start:start + chunk]
            ids = list(range(self.next_id + 1, self.next_id + 1 + len(batch)))
            self.next_id += len(batch)
            payload = "".join(
                json.dumps({"id": i, "fn": fn, "args": args}, allow_nan=True) + "\n"
                for i, args in zip(ids, batch))
            # Write from a thread: large responses would otherwise fill the stdout pipe while we
            # are still blocked writing requests.
            writer = threading.Thread(target=self._write, args=(payload,))
            writer.start()
            for expected_id in ids:
                line = self.proc.stdout.readline()
                if not line:
                    writer.join()
                    raise EngineError("engine exited (code %r) during %s" % (self.proc.wait(), fn))
                resp = json.loads(line)
                if resp.get("id") != expected_id:
                    raise EngineError("response id mismatch: %r != %r" % (resp.get("id"), expected_id))
                out.append((resp["ok"], resp["result"] if resp["ok"] else resp["error"]))
            writer.join()
        return out

    def _write(self, payload):
        try:
            self.proc.stdin.write(payload)
            self.proc.stdin.flush()
        except BrokenPipeError:
            pass

    def close(self):
        if self.proc.poll() is None:
            self.proc.stdin.close()
            self.proc.wait(timeout=30)


def check_parity(engine, fn, cases, oracle, min_cases, show=5, compare=None):
    """Run `fn` on every case in the engine and compare with `oracle(**case)`.

    `cases` is a list of argument dicts. The oracle may raise; the engine must then also report
    an error (error messages are not compared). Fails with a report of the first mismatches.
    """
    assert len(cases) >= min_cases, "only %d cases generated, need %d" % (len(cases), min_cases)
    results = engine.call_many(fn, cases)
    failures = []
    for case, (ok, got) in zip(cases, results):
        try:
            expected = oracle(**case)
            oracle_ok = True
        except Exception as e:  # the oracle's own failure is part of its behavior
            expected = "%s: %s" % (type(e).__name__, e)
            oracle_ok = False
        if oracle_ok != ok:
            failures.append((case, expected if oracle_ok else "ERROR " + expected,
                             got if ok else "ERROR " + got))
            continue
        if not oracle_ok:
            continue
        same = compare(expected, got) if compare else canon(expected) == canon(got)
        if not same:
            failures.append((case, expected, got))
    if failures:
        lines = ["%d of %d cases differ for %s" % (len(failures), len(cases), fn)]
        for case, expected, got in failures[:show]:
            lines.append("  case:     " + _short(case))
            lines.append("  expected: " + _short(expected))
            lines.append("  got:      " + _short(got))
        raise AssertionError("\n".join(lines))
    return len(cases)


def _short(v, limit=1500):
    s = canon(v) if not isinstance(v, str) else v
    return s if len(s) <= limit else s[:limit] + "...(%d chars)" % len(s)


def upstream_test_notebooks():
    """(name, raw JSON) for every upstream test notebook."""
    files = sorted((UPSTREAM / "nbdime" / "tests" / "files").glob("*.ipynb"))
    if not files:
        raise RuntimeError("upstream test notebooks not found under %s" % UPSTREAM)
    out = []
    for f in files:
        with open(f, encoding="utf-8") as fh:
            out.append((f.stem, json.load(fh)))
    return out


def notebook_families(notebooks):
    """Group notebooks by the prefix before '--' (or the whole name)."""
    fam = {}
    for name, nb in notebooks:
        fam.setdefault(name.split("--")[0], []).append((name, nb))
    return fam
