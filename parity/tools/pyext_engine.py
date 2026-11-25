"""A batch engine that answers the core functions through the `nbdime_rs` Python extension.

Speaks the `nbdime-rs-dev batch` protocol, so the existing suites can run unchanged against the
Python API (unit U15). Functions the extension provides are called with the same arguments the
dev tool takes; every other function is forwarded to the dev binary. Calls answered by the
extension are counted in the file named by NBDIME_RS_PYEXT_STATS, so the caller can check the
extension was really exercised.

    pyext_engine.py DEV_BINARY batch
"""

import json
import os
import sys
from collections import Counter

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

import nbdime_rs  # noqa: E402
from harness import Engine  # noqa: E402


def _merge_kwargs(args):
    return {"args": args.get("args"), "ignores": args.get("ignores"), "targets": args.get("targets")}


PY_FNS = {
    "diff": lambda a: nbdime_rs.diff(a["a"], a["b"]),
    "patch": lambda a: nbdime_rs.patch(a["obj"], a["diff"]),
    "diff_notebooks": lambda a: nbdime_rs.diff_notebooks(
        a["a"], a["b"], ignores=a.get("ignores"), targets=a.get("targets")),
    "decide_merge": lambda a: nbdime_rs.decide_merge(
        a["base"], a["local"], a["remote"], strategies=a.get("strategies"),
        transients=a.get("transients"), fall_back=a.get("fall_back")),
    "apply_decisions": lambda a: nbdime_rs.apply_decisions(a["base"], a["decisions"]),
    "decide_notebook_merge": lambda a: nbdime_rs.decide_notebook_merge(
        a["base"], a["local"], a["remote"], **_merge_kwargs(a)),
    "merge_notebooks": lambda a: list(nbdime_rs.merge_notebooks(
        a["base"], a["local"], a["remote"], **_merge_kwargs(a))),
}


def main():
    dev = Engine(sys.argv[1])
    stats = Counter()
    out = sys.stdout
    try:
        for line in sys.stdin:
            req = json.loads(line)
            fn, args = req["fn"], req["args"]
            if fn in PY_FNS:
                stats[fn] += 1
                try:
                    resp = {"id": req["id"], "ok": True, "result": PY_FNS[fn](args)}
                except (ValueError, TypeError, nbdime_rs.EngineError) as e:
                    resp = {"id": req["id"], "ok": False, "error": "%s: %s" % (type(e).__name__, e)}
            else:
                ok, res = dev.call(fn, args)
                resp = {"id": req["id"], "ok": ok, ("result" if ok else "error"): res}
            out.write(json.dumps(resp, allow_nan=True) + "\n")
            out.flush()
    finally:
        dev.close()
        path = os.environ.get("NBDIME_RS_PYEXT_STATS")
        if path:
            with open(path, "a") as f:
                f.write(json.dumps(stats) + "\n")


if __name__ == "__main__":
    main()
