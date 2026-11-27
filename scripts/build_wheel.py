#!/usr/bin/env python3
"""Build the `nbdime-rs` wheel: the `nbdime_rs` extension module plus the command line tools.

The commands are native executables, not Python entry points, so `nbdiff` starts in a few
milliseconds. They go into the wheel's `.data/scripts` directory, which pip installs into the
environment's `bin` (`Scripts` on Windows) unchanged.

    python scripts/build_wheel.py [--target TRIPLE] [--zig] [--out DIR]

`--zig` links against glibc 2.17 (manylinux2014) with cargo-zigbuild and `maturin --zig`.
Needs maturin (and, with --zig, ziglang and cargo-zigbuild) on PATH.
"""

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PY_CRATE = ROOT / "crates" / "nbdime-py"
SCRIPTS = PY_CRATE / "wheel-data" / "scripts"
COMMANDS = ["nbdime", "nbdiff", "nbmerge", "nbshow", "nbpatch", "git-nbdiffdriver",
            "git-nbmergedriver", "git-nbdifftool", "git-nbmergetool"]
MANYLINUX = "manylinux2014"


def run(cmd):
    print("+", " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=ROOT, check=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--target", help="Rust target triple (default: the host)")
    ap.add_argument("--zig", action="store_true", help="link for manylinux2014 with Zig")
    ap.add_argument("--out", default="dist", help="output directory (default: dist)")
    args = ap.parse_args()

    cargo = ["cargo", "zigbuild"] if args.zig else ["cargo", "build"]
    cargo += ["--release", "--locked", "-p", "nbdime-cli"]
    for name in COMMANDS:
        cargo += ["--bin", name]
    if args.target:
        # cargo-zigbuild takes the glibc version as a suffix of the target.
        cargo += ["--target", args.target + (".2.17" if args.zig and "linux-gnu" in args.target
                                              else "")]
    run(cargo)

    release = ROOT / "target" / (args.target or "") / "release"
    windows = "windows" in args.target if args.target else sys.platform == "win32"
    exe = ".exe" if windows else ""
    if SCRIPTS.exists():
        shutil.rmtree(SCRIPTS)
    SCRIPTS.mkdir(parents=True)
    for name in COMMANDS:
        dest = SCRIPTS / (name + exe)
        shutil.copy2(release / (name + exe), dest)
        os.chmod(dest, 0o755)

    pyproject = PY_CRATE / "pyproject.toml"
    original = pyproject.read_text()
    assert original.endswith("\n[tool.maturin]\n"), "[tool.maturin] must stay the last table"
    pyproject.write_text(original.replace("\n[tool.maturin]\n",
                                          '\n[tool.maturin]\ndata = "wheel-data"\n'))
    maturin = ["maturin", "build", "--release", "--locked", "-m", str(PY_CRATE / "Cargo.toml"),
               "--out", str(ROOT / args.out)]
    if args.target:
        maturin += ["--target", args.target]
    if args.zig:
        maturin += ["--zig", "--compatibility", MANYLINUX]
    try:
        run(maturin)
    finally:
        pyproject.write_text(original)


if __name__ == "__main__":
    main()
