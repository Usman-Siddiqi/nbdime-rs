"""CLI parity: `nbdiff`, `nbmerge` and `nbdime diff|merge` against upstream's entry points.

Each case runs upstream's console script from the reference venv and ours, on the same input
files, in the same directory and environment, and compares:
- the exit code;
- stdout, byte for byte;
- every output file named with `{out}` in the arguments (each side writes its own copy);
- stderr, byte for byte, unless upstream printed a Python traceback (then only "both non-empty"),
  ran an argparse usage error (exit 2: only the exit code and "both non-empty"), or the case runs
  at `--log-level DEBUG` (internal debug messages are not part of the interface).

Deliberate differences (findings F-029): `--version` text, `--help` text, and the `--config`
listing are not compared byte for byte.

Our binaries come from `NBDIME_RS_BIN` (default `target/release`): `nbdime`, `nbdiff`,
`nbmerge`. The environment is isolated: a temporary HOME and JUPYTER_CONFIG_DIR, no
JUPYTER_CONFIG_PATH, git pinned to its default configuration, `LANG=C.UTF-8`, `TZ=UTC`,
`COLUMNS=80`.

Notebooks that upstream would give random ids on read (F-003: missing or duplicate ids at
nbformat_minor >= 5) and v1-v3 notebooks (F-012) are not used as inputs here; those deviations
are covered by the U02 suite.
"""

import copy
import json
import os
import re
import shutil
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import nbformat
import pytest

from gen import rng_for
from gen_nb import dumps_disk, mutate_notebook, random_notebook
from harness import ROOT, UPSTREAM, scaled
from test_u04_notebook_diff import read_without_id_repair

REF_BIN = Path(os.environ.get("NBDIME_REF_BIN", "/home/claude/ref/venv/bin"))
OUR_BIN = Path(os.environ.get("NBDIME_RS_BIN", ROOT / "target" / "release"))
FILES = UPSTREAM / "nbdime" / "tests" / "files"
WORKERS = int(os.environ.get("PARITY_CLI_WORKERS", "4"))

IGNORABLE_FLAGS = ["-s", "-o", "-a", "-m", "-i", "-d"]
IGNORABLE_LONG = ["--sources", "--outputs", "--attachments", "--metadata", "--id", "--details"]


# ---------------------------------------------------------------------------- running


@pytest.fixture(scope="module")
def sandbox(tmp_path_factory):
    exe = ".exe" if os.name == "nt" else ""
    for name in ("nbdime", "nbdiff", "nbmerge"):
        if not (OUR_BIN / (name + exe)).exists():
            raise RuntimeError("binary not found: %s (cargo build --release)" % (OUR_BIN / name))
    root = tmp_path_factory.mktemp("cli")
    home = root / "home"
    (home / "jupyter").mkdir(parents=True)
    env = {
        "PATH": os.environ["PATH"],
        "HOME": str(home),
        "JUPYTER_CONFIG_DIR": str(home / "jupyter"),
        "JUPYTER_PLATFORM_DIRS": "0",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "TZ": "UTC",
        "COLUMNS": "80",
    }
    # Windows processes need these to start at all (Python's site, git, the C runtime).
    for name in ("SYSTEMROOT", "SYSTEMDRIVE", "COMSPEC", "PATHEXT", "TEMP", "TMP", "WINDIR"):
        if os.name == "nt" and name in os.environ:
            env[name] = os.environ[name]
    return root, env


def _run(binary, args, cwd, env, stdin):
    p = subprocess.run([str(binary)] + args, cwd=cwd, env=env, input=stdin, capture_output=True,
                       timeout=300)
    return p.returncode, p.stdout, p.stderr


def _side(case, which, root):
    out = root / ("out-%s-%d" % (which, case["n"]))
    args = [a.replace("{out}", str(out)) for a in case["args"]]
    return args, out


def _setup_dir(case, which, root):
    return root / ("setup-%s-%d" % (which, case["n"]))


def run_pair(case, root, env):
    """Returns (upstream, ours), each (code, stdout, stderr, outfile bytes or None, args).

    Optional case keys: `env`, `unset`, `cwd`, `stdin`; `setup(dir)` builds a fresh directory per
    side (`{dir}` in args and env values, `subdir` for the cwd inside it); `files` lists paths
    inside it whose contents are compared after the run; `path_bins` puts the side's binaries
    first on PATH (for git calling drivers and filters by name); cmd "git" runs git itself;
    `ref_binary` replaces upstream's executable (for entry points upstream installs no script for).
    """
    results = []
    for which, bindir in (("ref", REF_BIN), ("ours", OUR_BIN)):
        args, out = _side(case, which, root)
        if out.exists():
            out.unlink()
        cwd = case.get("cwd", root)
        extra_env = dict(case.get("env", {}))
        if "setup" in case:
            d = _setup_dir(case, which, root)
            if d.exists():
                shutil.rmtree(d)
            d.mkdir()
            case["setup"](d)
            cwd = d / case.get("subdir", "")
            args = [a.replace("{dir}", str(d)) for a in args]
            extra_env = {k: v.replace("{dir}", str(d)) for k, v in extra_env.items()}
        cmd = case["cmd"]
        argv = args
        if cmd.startswith("nbdime "):
            argv = [cmd.split(" ", 1)[1]] + args
            cmd = "nbdime"
        cenv = dict(env, **extra_env)
        for k in case.get("unset", ()):
            cenv.pop(k, None)
        if case.get("path_bins"):
            cenv["PATH"] = str(bindir) + os.pathsep + cenv["PATH"]
        binary = shutil.which("git") if cmd == "git" else bindir / cmd
        if which == "ref" and "ref_binary" in case:
            binary = case["ref_binary"]
        code, so, se = _run(binary, argv, cwd, cenv, case.get("stdin"))
        data = out.read_bytes() if out.exists() else None
        if "files" in case:
            parts = [] if data is None else [b"{out}\n" + data]
            for rel in case["files"]:
                f = cwd / rel
                parts.append(rel.encode() + b"\n" + (f.read_bytes() if f.exists() else b"<missing>"))
            data = b"\0".join(parts)
        results.append((code, so, se, data, args))
    return results


def _stderr_policy(case, ref):
    code, _, se = ref[0], ref[1], ref[2]
    if b"Traceback (most recent call last)" in se or b"Notebook JSON is invalid" in se:
        return "nonempty"
    if code == 2 and se.startswith(b"usage:"):
        return "nonempty"
    if "DEBUG" in case["args"]:
        return "ignore"
    return "exact"


def _unside(case, which, result, root):
    """Each side writes its own output file and setup directory; their paths may appear in
    messages. `mask` is a list of (regex, replacement) bytes pairs for run-specific text."""
    _, out = _side(case, which, root)
    code, so, se, data, args = result
    subs = [(str(out).encode(), b"{out}")]
    if "setup" in case:
        subs.append((str(_setup_dir(case, which, root)).encode(), b"{dir}"))

    def clean(b):
        if b is None:
            return b
        for old, new in subs:
            b = b.replace(old, new)
        for pat, rep in case.get("mask", ()):
            b = re.sub(pat, rep, b)
        return b
    return code, clean(so), clean(se), clean(data), args


def check_cli(cases, root, env, min_cases):
    assert len(cases) >= min_cases, "only %d cases, need %d" % (len(cases), min_cases)
    for n, c in enumerate(cases):
        c["n"] = n
    with ThreadPoolExecutor(WORKERS) as pool:
        results = list(pool.map(lambda c: run_pair(c, root, env), cases))
    failures = []
    for case, (ref, ours) in zip(cases, results):
        ref, ours = _unside(case, "ref", ref, root), _unside(case, "ours", ours, root)
        if case.get("mask_new_ids"):
            ref, ours = _mask_new_ids(case, ref), _mask_new_ids(case, ours)
        problems = []
        if ref[0] != ours[0]:
            problems.append("exit code %r != %r" % (ours[0], ref[0]))
        if ref[1] != ours[1]:
            problems.append("stdout differs:\n--- upstream\n%s\n--- ours\n%s" % (
                ref[1].decode("utf-8", "replace")[:3000], ours[1].decode("utf-8", "replace")[:3000]))
        if ref[3] != ours[3]:
            problems.append("output file differs: upstream %s, ours %s" % (
                None if ref[3] is None else ref[3][:2000], None if ours[3] is None else ours[3][:2000]))
        policy = _stderr_policy(case, ref)
        if policy == "exact" and ref[2] != ours[2]:
            problems.append("stderr differs:\n--- upstream\n%s\n--- ours\n%s" % (
                ref[2].decode("utf-8", "replace")[:2000], ours[2].decode("utf-8", "replace")[:2000]))
        elif policy == "nonempty" and bool(ref[2]) != bool(ours[2]):
            problems.append("stderr emptiness differs: upstream %r, ours %r" % (ref[2][:500], ours[2][:500]))
        if problems:
            failures.append("%s %s (cwd %s)\n%s" % (case["cmd"], " ".join(ours[4]), case.get("cwd", root),
                                                   "\n".join(problems)))
    assert not failures, "%d of %d CLI cases differ; first ones:\n\n%s" % (
        len(failures), len(cases), "\n\n".join(failures[:4]))


_HEX8 = re.compile(rb"(?<![0-9a-f])[0-9a-f]{8}(?![0-9a-f])")


def _mask_new_ids(case, result):
    """F-021: inline-cells marker cells get random ids upstream (deterministic ones here). Mask
    every 8-hex token that does not occur in the input files."""
    known = set()
    for a in case["args"]:
        if a.endswith(".ipynb") and os.path.exists(a):
            known.update(_HEX8.findall(Path(a).read_bytes()))

    def mask(b):
        if b is None:
            return None
        return _HEX8.sub(lambda m: m.group(0) if m.group(0) in known else b"<new-id>", b)

    code, so, se, out, args = result
    return code, mask(so), mask(se), mask(out), args


# ---------------------------------------------------------------------------- inputs


def _usable(text):
    """True if upstream reads this text deterministically and as v4 (no F-003 / F-012 cases)."""
    try:
        raw = json.loads(text)
        if raw.get("nbformat") != 4:
            return False
        repaired = nbformat.reads(text, as_version=4)
        plain = read_without_id_repair(raw)
    except Exception:
        return False
    return json.dumps(repaired, sort_keys=True) == json.dumps(plain, sort_keys=True)


def upstream_files(root):
    """Usable upstream test notebooks, copied into the sandbox, grouped by family."""
    dest = root / "files"
    dest.mkdir(exist_ok=True)
    fams = {}
    for f in sorted(FILES.glob("*.ipynb")):
        text = f.read_text(encoding="utf-8")
        if not _usable(text):
            continue
        target = dest / f.name
        target.write_text(text, encoding="utf-8")
        fam = f.stem.split("--")[0]
        if fam.startswith("multilevel-test"):
            fam = "multilevel-test"
        fams.setdefault(fam, []).append(str(target))
    return fams


def generated_files(root, name, n):
    """n (base, other) pairs of generated notebooks on disk."""
    rng = rng_for(name)
    dest = root / ("gen-" + name)
    dest.mkdir(exist_ok=True)
    pairs = []
    i = 0
    while len(pairs) < n:
        i += 1
        base = random_notebook(rng, n_cells=rng.choice([0, 1, 3, 6, 10]))
        other = mutate_notebook(rng, base, intensity=rng.choice([0, 1, 2, 4]))
        ta, tb = dumps_disk(base, rng), dumps_disk(other, rng)
        if not (_usable(ta) and _usable(tb)):
            continue
        fa, fb = dest / ("%d-a.ipynb" % i), dest / ("%d-b.ipynb" % i)
        fa.write_text(ta, encoding="utf-8")
        fb.write_text(tb, encoding="utf-8")
        pairs.append((str(fa), str(fb)))
    return pairs


def generated_triples(root, name, n):
    rng = rng_for(name)
    dest = root / ("gen-" + name)
    dest.mkdir(exist_ok=True)
    triples = []
    i = 0
    while len(triples) < n:
        i += 1
        base = random_notebook(rng, n_cells=rng.choice([0, 1, 3, 6]))
        local = mutate_notebook(rng, base, intensity=rng.choice([0, 1, 2, 3]))
        remote = mutate_notebook(rng, base, intensity=rng.choice([0, 1, 2, 3]))
        texts = [dumps_disk(x, rng) for x in (base, local, remote)]
        if not all(_usable(t) for t in texts):
            continue
        paths = []
        for tag, t in zip("blr", texts):
            p = dest / ("%d-%s.ipynb" % (i, tag))
            p.write_text(t, encoding="utf-8")
            paths.append(str(p))
        triples.append(tuple(paths))
    return triples


def _flags(rng):
    flags = []
    r = rng.random()
    if r < 0.25:
        flags += rng.sample(IGNORABLE_FLAGS, rng.randint(1, 3))
    elif r < 0.4:
        flags += [f.upper() for f in rng.sample(IGNORABLE_FLAGS, rng.randint(1, 3))]
    elif r < 0.5:
        long = rng.sample(IGNORABLE_LONG, rng.randint(1, 2))
        flags += long if rng.random() < 0.5 else ["--ignore-" + f[2:] for f in long]
    elif r < 0.53:
        flags += ["-s", "-O"]  # mixed positive/negative: usage error
    for opt, p in (("--no-color", 0.3), ("--color-words", 0.15), ("--no-git", 0.1),
                   ("--no-use-diff", 0.1)):
        if rng.random() < p:
            flags.append(opt)
    if rng.random() < 0.1:
        flags += ["--log-level", rng.choice(["WARN", "ERROR", "CRITICAL", "INFO"])]
    rng.shuffle(flags)
    return flags


# ---------------------------------------------------------------------------- nbdiff


def test_cli_nbdiff_upstream_files(sandbox):
    root, env = sandbox
    rng = rng_for("cli-nbdiff-files")
    cases = []
    for paths in upstream_files(root).values():
        for a in paths:
            for b in paths:
                cmd = rng.choice(["nbdiff", "nbdiff", "nbdime diff"])
                cases.append({"cmd": cmd, "args": _flags(rng) + [a, b]})
    check_cli(cases, root, env, min_cases=150)


def test_cli_nbdiff_generated(sandbox):
    root, env = sandbox
    rng = rng_for("cli-nbdiff-gen")
    cases = []
    for a, b in generated_files(root, "nbdiff", scaled(250)):
        args = _flags(rng) + [a, b]
        if rng.random() < 0.25:
            args += ["--out", "{out}"]
        cases.append({"cmd": "nbdiff", "args": args})
    check_cli(cases, root, env, min_cases=scaled(250))


def test_cli_nbdiff_special_inputs(sandbox):
    root, env = sandbox
    fams = upstream_files(root)
    some = fams["src-and-output"][0]
    empty = root / "empty-file.ipynb"
    empty.write_text("")
    garbage = root / "garbage.ipynb"
    garbage.write_text("{not json")
    v3 = root / "v3.ipynb"
    v3.write_text(json.dumps({"nbformat": 3, "nbformat_minor": 0, "metadata": {}, "worksheets": []}))
    cases = [
        {"cmd": "nbdiff", "args": ["/dev/null", some]},
        {"cmd": "nbdiff", "args": [some, "/dev/null"]},
        {"cmd": "nbdiff", "args": [some, "/dev/null", "--out", "{out}"]},
        {"cmd": "nbdiff", "args": [some, str(root / "does-not-exist.ipynb")]},
        {"cmd": "nbdiff", "args": [str(root / "nope.ipynb"), some]},
        {"cmd": "nbdiff", "args": [some, some]},
        {"cmd": "nbdiff", "args": [some, some, "--out", "{out}"]},
        {"cmd": "nbdiff", "args": [str(empty), some]},
        {"cmd": "nbdiff", "args": [str(garbage), some]},
        {"cmd": "nbdiff", "args": ["--log-level", "BOGUS", some, some]},
        {"cmd": "nbdiff", "args": ["--bogus-flag", some, some]},
        {"cmd": "nbdiff", "args": ["-s", "-S", some, some]},
        {"cmd": "nbdime diff", "args": [some, "/dev/null"]},
        {"cmd": "nbdime diff", "args": ["--no-color", some, fams["src-and-output"][-1]]},
    ]
    check_cli(cases, root, env, min_cases=len(cases))


def test_cli_nbdiff_config_file(sandbox):
    """nbdime_config.json in JUPYTER_CONFIG_DIR sets argparse defaults (ConfigBackedParser)."""
    root, env = sandbox
    fams = upstream_files(root)
    a, b = fams["src-and-output"][0], fams["src-and-output"][-1]
    configs = [
        {"Diff": {"outputs": False}},
        {"Diff": {"sources": True}},
        {"NbDiff": {"color_words": True}},
        {"NbDiff": {"metadata": False, "details": False}},
        {"Diff": {"Ignore": {"/cells/*/outputs": True}}},
        {"Diff": {"Ignore": {"/cells/*/metadata": ["collapsed"]}}},
        {"Global": {"log_level": "ERROR"}},
        {"NbDiff": {"bogus": 1}},
    ]
    cases = []
    for i, cfg in enumerate(configs):
        cdir = root / ("cfg-%d" % i)
        cdir.mkdir(exist_ok=True)
        (cdir / "nbdime_config.json").write_text(json.dumps(cfg))
        for args in ([a, b], ["-s", a, b], ["--no-color", a, b]):
            cases.append({"cmd": "nbdiff", "args": args, "env": {"JUPYTER_CONFIG_DIR": str(cdir)}})
    check_cli(cases, root, env, min_cases=len(cases))


def _git(repo, *args, env):
    subprocess.run(["git"] + list(args), cwd=repo, env=env, check=True, capture_output=True)


def test_cli_nbdiff_git_refs(sandbox):
    """nbdiff with git revisions: `nbdiff`, `nbdiff HEAD`, `nbdiff REF path`, `nbdiff A B [paths]`."""
    root, env = sandbox
    repo = root / "repo"
    if repo.exists():
        shutil.rmtree(repo)
    (repo / "sub").mkdir(parents=True)
    genv = dict(env, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@example.com", GIT_COMMITTER_NAME="t",
                GIT_COMMITTER_EMAIL="t@example.com", GIT_AUTHOR_DATE="2020-01-01T00:00:00Z",
                GIT_COMMITTER_DATE="2020-01-01T00:00:00Z")
    _git(repo, "init", "-q", "-b", "main", env=genv)
    rng = rng_for("cli-git")
    names = ["a.ipynb", "sub/b.ipynb", "c.ipynb", "notes.txt"]
    nbs = {}
    for rev in range(4):
        for name in names:
            if name.endswith(".txt"):
                (repo / name).write_text("rev %d\n" % rev)
                continue
            if rev == 0 or rng.random() < 0.7:
                nb = random_notebook(rng, n_cells=4) if name not in nbs else mutate_notebook(rng, nbs[name], 2)
                text = dumps_disk(nb)
                if _usable(text):
                    nbs[name] = nb
                    (repo / name).write_text(text)
        if rev == 2 and (repo / "c.ipynb").exists():
            (repo / "c.ipynb").unlink()
            nbs.pop("c.ipynb", None)
        _git(repo, "add", "-A", env=genv)
        _git(repo, "commit", "-q", "-m", "rev %d" % rev, env=genv)
    # Uncommitted working-tree changes.
    if "a.ipynb" in nbs:
        text = dumps_disk(mutate_notebook(rng, nbs["a.ipynb"], 2))
        if _usable(text):
            (repo / "a.ipynb").write_text(text)
    arg_sets = [
        [], ["HEAD"], ["HEAD~1"], ["HEAD~2", "HEAD"], ["HEAD~3", "HEAD~1"], ["HEAD", "a.ipynb"],
        ["HEAD~2", "HEAD", "sub/b.ipynb"], ["HEAD~3", "HEAD", "a.ipynb", "sub"], ["a.ipynb"],
        ["main~1", "main"], ["HEAD~3", "HEAD", "--out", "{out}"], ["--no-color", "HEAD~2", "HEAD"],
        ["-s", "HEAD~3", "HEAD"], ["HEAD", "missing.ipynb"],
    ]
    cases = [{"cmd": "nbdiff", "args": a, "cwd": str(repo)} for a in arg_sets]
    cases += [{"cmd": "nbdiff", "args": a, "cwd": str(repo / "sub")} for a in ([], ["HEAD~3", "HEAD"])]
    check_cli(cases, root, dict(env, GIT_CEILING_DIRECTORIES=str(root)), min_cases=len(cases))


def _commit_repo(path, env, revs):
    """A repository with one commit per entry of `revs` ({name: text})."""
    path.mkdir(parents=True)
    genv = dict(env, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@example.com", GIT_COMMITTER_NAME="t",
                GIT_COMMITTER_EMAIL="t@example.com", GIT_AUTHOR_DATE="2020-01-01T00:00:00Z",
                GIT_COMMITTER_DATE="2020-01-01T00:00:00Z")
    _git(path, "init", "-q", "-b", "main", env=genv)
    for n, files in enumerate(revs):
        for name, text in files.items():
            (path / name).write_text(text)
        _git(path, "add", "-A", env=genv)
        _git(path, "commit", "-q", "-m", "rev %d" % n, env=genv)
    return genv


def test_cli_review_regressions(sandbox):
    """Linked worktrees, revision forms GitPython rejects or accepts, empty pathspecs, file names
    that are not UTF-8, HOME unset, config dirs with ~ and $VARS, odd top-level config files and
    wrong-typed config values."""
    root, env = sandbox
    base = root / "regr"
    if base.exists():
        shutil.rmtree(base)
    rng = rng_for("cli-regressions")
    nbs = [random_notebook(rng, n_cells=3)]
    for _ in range(3):
        nbs.append(mutate_notebook(rng, nbs[-1], 2))
    texts = [t for t in (dumps_disk(nb) for nb in nbs) if _usable(t)]
    assert len(texts) >= 3
    repo = base / "repo"
    genv = _commit_repo(repo, env, [{"a.ipynb": texts[0]}, {"a.ipynb": texts[1]}])
    wt = base / "wt"
    _git(repo, "worktree", "add", "-q", str(wt), "HEAD", env=genv)
    (wt / "a.ipynb").write_text(texts[2])
    (repo / "a.ipynb").write_text(texts[2])
    cases = [{"cmd": "nbdiff", "args": a, "cwd": str(wt)}
             for a in ([], ["--no-color"], ["HEAD~1"], ["HEAD~1", "HEAD"], ["HEAD", "a.ipynb"])]
    cases += [{"cmd": "nbdiff", "args": a, "cwd": str(repo)}
              for a in ([":/rev 0"], ["HEAD~1..HEAD"], ["HEAD~1...HEAD"], ["HEAD^!"], ["HEAD^{tree}"],
                        ["HEAD:a.ipynb"], ["HEAD", ""], [""], ["HEAD~1", "HEAD", "", "a.ipynb"])]
    cases.append({"cmd": "nbdiff", "args": ["HEAD~1"], "cwd": str(repo / ".git")})

    other = base / "other.ipynb"
    other.write_text(texts[1])
    # macOS (APFS) and Windows refuse file names that are not valid Unicode.
    if sys.platform == "linux":
        odd = base / os.fsdecode(b"\xff.ipynb")
        odd.write_text(texts[0])
        cases += [
            {"cmd": "nbdiff", "args": [str(odd), str(other)]},
            {"cmd": "nbdiff", "args": ["--no-color", str(other), str(odd), "--out", "{out}"]},
            {"cmd": "nbmerge", "args": [str(odd), str(other), str(odd), "--out", "{out}"]},
        ]
    cases.append({"cmd": "nbdiff", "args": ["-1\n", str(other)]})

    plain = base / "plain.ipynb"
    plain.write_text(texts[2])
    cfghome = base / "cfghome"
    (cfghome / "cfg").mkdir(parents=True)
    (cfghome / "cfg" / "nbdime_config.json").write_text('{"NbDiff": {"outputs": false}}')
    cases += [
        {"cmd": "nbdiff", "args": ["--no-color", str(other), str(plain)],
         "env": {"HOME": str(cfghome), "JUPYTER_CONFIG_DIR": "~/cfg"}},
        {"cmd": "nbdiff", "args": ["--no-color", str(other), str(plain)],
         "env": {"HOME": str(cfghome), "JUPYTER_CONFIG_PATH": "$HOME/cfg"}},
    ]
    nohome = base / "nohome"
    (nohome / ".jupyter").mkdir(parents=True)
    (nohome / ".jupyter" / "nbdime_config.json").write_text('{"NbDiff": {"sources": false}}')
    cases.append({"cmd": "nbdiff", "args": ["--no-color", str(other), str(plain)], "cwd": str(nohome),
                  "unset": ["HOME", "JUPYTER_CONFIG_DIR"]})

    for n, content in enumerate(['""', '[1]', '[["NbDiff", {"outputs": false}]]', '"version"',
                                 '{"NbDiff": {"use_color": "no"}}', '{"NbDiff": {"use_color": 2}}',
                                 '{"NbDiff": {"paths": "x"}}', '{"NbDiff": {"color_words": {}}}',
                                 '{"NbMerge": {"base": 5}}']):
        d = base / ("cfg%d" % n)
        d.mkdir()
        (d / "nbdime_config.json").write_text(content)
        cases.append({"cmd": "nbdiff", "args": [str(other), str(plain)], "cwd": str(d)})
        cases.append({"cmd": "nbdiff", "args": ["--config"], "cwd": str(d)})
        cases.append({"cmd": "nbmerge", "args": [str(plain), str(other), str(plain)], "cwd": str(d)})
    cases += [{"cmd": "nbdime extensions", "args": a} for a in (["--x"], [])]
    check_cli(cases, root, dict(env, GIT_CEILING_DIRECTORIES=str(root)), min_cases=len(cases))


# ---------------------------------------------------------------------------- nbmerge


def _merge_flags(rng):
    flags = []
    if rng.random() < 0.5:
        flags += ["--merge-strategy", rng.choice(["inline", "use-base", "use-local", "use-remote",
                                                  "union", "inline", "bogus"])]
    if rng.random() < 0.25:
        flags += ["--input-strategy", rng.choice(["inline", "use-base", "use-local", "use-remote", "union"])]
    if rng.random() < 0.25:
        flags += ["--output-strategy", rng.choice(["inline", "use-base", "use-local", "use-remote",
                                                   "remove", "clear-all"])]
    if rng.random() < 0.15:
        flags.append("--no-ignore-transients")
    if rng.random() < 0.15:
        flags += rng.sample(IGNORABLE_FLAGS, 1)
    if rng.random() < 0.2:
        flags.append("--no-color")
    rng.shuffle(flags)
    return flags


def _merge_case(rng, b, l, r):
    args = _merge_flags(rng)
    mode = rng.random()
    if mode < 0.35:
        args += ["--out", "{out}"]
    elif mode < 0.55:
        args.append("--decisions")
    elif mode < 0.6:
        args += ["--decisions", "--out", "{out}"]
    return {"cmd": rng.choice(["nbmerge", "nbmerge", "nbdime merge"]), "args": args + [b, l, r],
            "mask_new_ids": True}


def test_cli_nbmerge_upstream_files(sandbox):
    root, env = sandbox
    rng = rng_for("cli-nbmerge-files")
    cases = []
    for paths in upstream_files(root).values():
        if len(paths) < 3:
            continue
        for _ in range(12):
            b, l, r = (rng.choice(paths) for _ in range(3))
            cases.append(_merge_case(rng, b, l, r))
    check_cli(cases, root, env, min_cases=60)


SITE_PACKAGES_MASK = (
    rb"(?m)^(\[W warnings:110\] )/[^\n]*/site-packages/(?=nbformat/validator\.py:434: )", rb"\1")


def test_cli_nbmerge_generated(sandbox):
    root, env = sandbox
    rng = rng_for("cli-nbmerge-gen")
    cases = [_merge_case(rng, *t) for t in generated_triples(root, "nbmerge", scaled(250))]
    for c in cases:
        # The warning names nbformat's install location, which differs between environments.
        c["mask"] = list(c.get("mask", ())) + [SITE_PACKAGES_MASK]
    check_cli(cases, root, env, min_cases=scaled(250))


def test_cli_nbmerge_special_inputs(sandbox):
    root, env = sandbox
    fams = upstream_files(root)
    b, l, r = (fams["inline-conflict"] + fams["inline-conflict"])[:3]
    empty = root / "empty-file.ipynb"
    empty.write_text("")
    cases = [
        {"cmd": "nbmerge", "args": ["/dev/null", l, r]},
        {"cmd": "nbmerge", "args": [l, r]},
        {"cmd": "nbmerge", "args": [b, "/dev/null", "/dev/null"]},
        {"cmd": "nbmerge", "args": [b, "/dev/null", "/dev/null", "--decisions"]},
        {"cmd": "nbmerge", "args": [b, "/dev/null", "/dev/null", "--out", "{out}"]},
        {"cmd": "nbmerge", "args": [str(empty), l, r]},
        {"cmd": "nbmerge", "args": [b, str(root / "missing.ipynb"), r]},
        {"cmd": "nbmerge", "args": [b, l]},
        {"cmd": "nbmerge", "args": [b, l, r, "--merge-strategy", "nonsense"]},
        {"cmd": "nbmerge", "args": [b, l, r, "--log-level", "ERROR"]},
        {"cmd": "nbmerge", "args": [b, b, b]},
        {"cmd": "nbmerge", "args": [b, l, l, "--decisions"]},
    ]
    for c in cases:
        c["mask_new_ids"] = True
    check_cli(cases, root, env, min_cases=len(cases))


def test_cli_delta_regressions(sandbox):
    """Notebook text in the private-use range the engine uses for undecodable name bytes, `:path`
    revisions, revision forms GitPython rejects, and GIT_DIR from the environment."""
    root, env = sandbox
    base = root / "delta"
    if base.exists():
        shutil.rmtree(base)
    rng = rng_for("cli-delta")
    # With ids: the appended cell has one, so the notebook must be 4.5 for every seed.
    nb = random_notebook(rng, n_cells=2, ids=True)
    texts = []
    for src in ("x = 1\n", "x = '\U0010F780\U0010F7FF'\ny = '\\udcff'\n"):
        n = copy.deepcopy(nb)
        n["cells"].append({"cell_type": "code", "execution_count": None, "metadata": {},
                           "outputs": [], "source": src, "id": "deadbeef"})
        texts.append(dumps_disk(n))
    repo = base / "repo"
    genv = _commit_repo(repo, env, [{"a.ipynb": texts[0]}, {"a.ipynb": texts[1]}])
    a, b = base / "a.ipynb", base / "b.ipynb"
    a.write_text(texts[0])
    b.write_text(texts[1])
    (repo / "a.ipynb").write_text(texts[0])
    _git(repo, "tag", "light", env=genv)
    cases = [
        {"cmd": "nbmerge", "args": [str(a), str(a), str(b)]},
        {"cmd": "nbmerge", "args": [str(a), str(b), str(a), "--decisions"]},
        {"cmd": "nbmerge", "args": [str(a), str(b), str(a), "--out", "{out}"]},
        {"cmd": "nbdiff", "args": ["--no-color", str(a), str(b)]},
        {"cmd": "nbdiff", "args": [str(b), str(a)]},
    ]
    revs = [[":nope.ipynb"], ["HEAD~1", ":nope.ipynb"], [":a.ipynb"], ["HEAD^{foo}"],
            ["HEAD^{blob}"], ["light^{tag}"], ["HEAD^{tree}~1"], ["HEAD^{commit}"], ["HEAD@{9}"],
            ["@{-5}"], ["HEAD@{-1}"], [":/"], [":/!x"], ["HEAD:nope.ipynb"], ["HEAD~1:sub/nope"],
            ["deadbee@{1}"], ["HEAD~1", "light"]]
    cases += [{"cmd": "nbdiff", "args": ["--no-color"] + r, "cwd": str(repo)} for r in revs]
    other = base / "other"
    _commit_repo(other, env, [{"z.ipynb": texts[1]}])
    cases += [{"cmd": "nbdiff", "args": ["--no-color"] + r, "cwd": str(repo),
               "env": {"GIT_DIR": str(other / ".git")}} for r in ([], ["HEAD~1"])]
    cases += [{"cmd": "nbdiff", "args": ["--no-color", "HEAD~1"], "cwd": str(repo),
               "env": {"GIT_DIR": ".git"}}]
    check_cli(cases, root, dict(env, GIT_CEILING_DIRECTORIES=str(root)), min_cases=len(cases))
