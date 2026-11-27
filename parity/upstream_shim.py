"""Pytest plugin: run upstream nbdime's own test files against the nbdime-rs binaries.

Loaded with `-p upstream_shim` when pytest runs upstream's tests (see test_upstream_suite.py).
Every entry point the tests call (`main` of each app, `main_dispatch`, and `main_show`,
`main_diff`, `main_merge` on a parsed namespace) is replaced by a function that runs the
corresponding nbdime-rs binary with the same argv in the same cwd and environment, copies its
stdout and stderr to `sys.stdout`/`sys.stderr` (so `capsys` sees them) and returns its exit
code. Our binaries come first on PATH, so git finds our drivers and filters too.

Log lines on stderr (`[W module:123] message`) become records of the `nbdime` logger, so
`caplog` sees them as it sees upstream's. A namespace remembers the cwd it was parsed in, because
upstream reads configuration files while parsing. Help and usage errors raise SystemExit, as
argparse does inside upstream's entry points.
"""

import argparse
import logging
import os
import re
import subprocess
import sys
from pathlib import Path

OUR_BIN = Path(os.environ["NBDIME_RS_BIN"])

# module -> {function name: binary (with leading subcommand args)}
ENTRY_POINTS = {
    "nbdime.nbshowapp": {"main": ["nbshow"], "main_show": ["nbshow"]},
    "nbdime.nbdiffapp": {"main": ["nbdiff"], "main_diff": ["nbdiff"]},
    "nbdime.nbmergeapp": {"main": ["nbmerge"], "main_merge": ["nbmerge"]},
    "nbdime.nbpatchapp": {"main": ["nbpatch"]},
    "nbdime.__main__": {"main_dispatch": ["nbdime"], "main": ["nbdime"]},
    "nbdime.vcs.git.diffdriver": {"main": ["git-nbdiffdriver"]},
    "nbdime.vcs.git.mergedriver": {"main": ["git-nbmergedriver"]},
    "nbdime.vcs.git.difftool": {"main": ["git-nbdifftool"]},
    "nbdime.vcs.git.mergetool": {"main": ["git-nbmergetool"]},
}
ARGV_ATTR = "_nbdime_rs_argv"
CWD_ATTR = "_nbdime_rs_cwd"
LEVELS = {"D": logging.DEBUG, "I": logging.INFO, "W": logging.WARNING, "E": logging.ERROR,
          "C": logging.CRITICAL}
LOG_LINE = re.compile(r"\[([DIWEC]) ([\w.]+):(\d+)\] (.*)")


def _replay_stderr(text):
    logger = logging.getLogger("nbdime")
    pending = None
    rest = []

    def flush():
        if pending is not None:
            level, module, line, msg = pending
            logger.handle(logger.makeRecord(logger.name, level, module + ".py", line, msg, (), None))

    for line in text.splitlines(keepends=True):
        m = LOG_LINE.match(line)
        if m:
            flush()
            pending = (LEVELS[m.group(1)], m.group(2), int(m.group(3)), m.group(4))
        elif pending is not None and not line.startswith("Traceback"):
            pending = pending[:3] + (pending[3] + "\n" + line.rstrip("\n"),)
        else:
            flush()
            pending = None
            rest.append(line)
    flush()
    sys.stderr.write("".join(rest))


def run_binary(cmd, argv, cwd=None):
    exe = OUR_BIN / (cmd[0] + (".exe" if os.name == "nt" else ""))
    if not exe.exists():
        raise RuntimeError("nbdime-rs binary not built: %s" % exe)
    sys.stdout.flush()
    sys.stderr.flush()
    p = subprocess.run([str(exe)] + cmd[1:] + list(argv), stdin=subprocess.DEVNULL,
                       capture_output=True, cwd=cwd)
    out = p.stdout.decode("utf-8", "surrogateescape")
    err = p.stderr.decode("utf-8", "surrogateescape")
    sys.stdout.write(out)
    _replay_stderr(err)
    # argparse leaves through SystemExit (help, usage errors) where the app would return a code.
    if any(a in ("-h", "--help") for a in argv) or (p.returncode == 2 and "usage:" in err):
        raise SystemExit(p.returncode)
    return p.returncode


def _argv_main(cmd):
    def main(args=None):
        return run_binary(cmd, sys.argv[1:] if args is None else args)
    return main


def _namespace_main(cmd):
    def main(args):
        argv = getattr(args, ARGV_ATTR, None)
        if argv is None:
            raise RuntimeError("namespace was not parsed by a shimmed parser")
        return run_binary(cmd, argv, getattr(args, CWD_ATTR, None))
    return main


_orig_parse_known_args = argparse.ArgumentParser.parse_known_args


def _parse_known_args(self, args=None, namespace=None):
    argv = list(sys.argv[1:] if args is None else args)
    ns, rest = _orig_parse_known_args(self, args, namespace)
    if getattr(ns, ARGV_ATTR, None) is None:
        setattr(ns, ARGV_ATTR, argv)
        setattr(ns, CWD_ATTR, os.getcwd())
    return ns, rest


def pytest_configure(config):
    import importlib
    argparse.ArgumentParser.parse_known_args = _parse_known_args
    for modname, fns in ENTRY_POINTS.items():
        mod = importlib.import_module(modname)
        for fn, cmd in fns.items():
            if not hasattr(mod, fn):
                continue
            setattr(mod, fn, _namespace_main(cmd) if fn.startswith("main_") and fn != "main_dispatch"
                    else _argv_main(cmd))
    os.environ["PATH"] = str(OUR_BIN) + os.pathsep + os.environ["PATH"]
