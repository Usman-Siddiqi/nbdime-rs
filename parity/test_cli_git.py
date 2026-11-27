"""U14: git integration and nbpatch against upstream's console scripts (`nbpatch`: its module).

git itself runs the drivers here: each side gets a fresh, identical repository (fixed author and
dates, so identical object ids) and its own binaries first on PATH, so `git diff` and `git merge`
call that side's `git-nbdiffdriver` / `git-nbmergedriver` and filters. Temp-file names git
invents for blobs and their timestamps are masked; everything else is compared byte for byte,
including the repository files the commands write (.git/config, attributes, merge results).
"""

import json
import os
import subprocess
import time

from gen import rng_for
from gen_nb import dumps_disk, mutate_notebook, random_notebook
from test_cli import REF_BIN, _usable, check_cli, sandbox  # noqa: F401 (fixture)

GIT_ENV = {"GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.com", "GIT_COMMITTER_NAME": "t",
           "GIT_COMMITTER_EMAIL": "t@example.com", "GIT_AUTHOR_DATE": "2020-01-01T00:00:00Z",
           "GIT_COMMITTER_DATE": "2020-01-01T00:00:00Z"}
MASK = [
    (rb"/tmp/[^\s'\"]*?/(?:[0-9A-Za-z]+_)?(?=[^/\s]*\.ipynb)", b"{tmp}/"),
    (rb"(?m)(?<=Merge result written to )\.merge_file_[0-9A-Za-z]{6}$", b"{merge-result-tmp}"),
    (rb"(?m)^(\[W warnings:110\] )/[^\n]*/site-packages/(?=nbformat/validator\.py:434: DuplicateCellId: )",
     rb"\1"),
    (rb"\d{4}-\d\d-\d\d \d\d:\d\d:\d\d(?:\.\d+)?", b"{time}"),
    (rb"\(no timestamp\)", b"{time}"),
]
DIFF_CMD = "git-nbdiffdriver diff"
MERGE_CMD = "git-nbmergedriver merge %O %A %B %L %P"


def _texts(name, n, intensity=2):
    rng = rng_for(name)
    nbs = [random_notebook(rng, n_cells=4)]
    while len(nbs) < n:
        nbs.append(mutate_notebook(rng, nbs[-1], intensity))
    texts = [dumps_disk(nb) for nb in nbs]
    return [t for t in texts if _usable(t)]


def _git(d, *args, env):
    subprocess.run(["git"] + list(args), cwd=d, env=env, check=True, capture_output=True)


def _env(base_env):
    return dict(base_env, **GIT_ENV)


def _backdate(age, *paths):
    """Sets the mtime of the files just written to `age` seconds ago, before git indexes them.

    A file modified in the same timestamp tick as the index is "racily clean": git re-reads it
    (through any clean filter) instead of trusting its stat data, so whether `git diff` hands it
    to the driver would depend on timing. macOS runners hit that often enough to flake. Each
    revision gets its own whole second, so a same-size rewrite still changes the stat data.
    """
    t = int(time.time()) - age
    for p in paths:
        os.utime(p, (t, t))


def diff_repo(texts, env, attrs=True, driver=DIFF_CMD, extra_config=()):
    """main: a.ipynb, sub/b.ipynb at rev0, rev1 (a renamed copy c.ipynb at rev2); working-tree
    and staged changes on top."""
    def setup(d):
        e = _env(env)
        _git(d, "init", "-q", "-b", "main", env=e)
        (d / "sub").mkdir()
        (d / "a.ipynb").write_text(texts[0])
        (d / "sub" / "b.ipynb").write_text(texts[1])
        (d / "notes.txt").write_text("one\n")
        _backdate(30, d / "a.ipynb", d / "sub" / "b.ipynb", d / "notes.txt")
        _git(d, "add", "-A", env=e)
        _git(d, "commit", "-q", "-m", "rev0", env=e)
        (d / "a.ipynb").write_text(texts[1])
        (d / "sub" / "b.ipynb").write_text(texts[2])
        (d / "notes.txt").write_text("two\n")
        _backdate(20, d / "a.ipynb", d / "sub" / "b.ipynb", d / "notes.txt")
        _git(d, "commit", "-q", "-am", "rev1", env=e)
        _git(d, "mv", "a.ipynb", "c.ipynb", env=e)
        _git(d, "commit", "-q", "-m", "rev2", env=e)
        (d / "c.ipynb").write_text(texts[3])
        _backdate(10, d / "c.ipynb")
        _git(d, "add", "c.ipynb", env=e)
        (d / "sub" / "b.ipynb").write_text(texts[4])
        if driver:
            _git(d, "config", "diff.jupyternotebook.command", driver, env=e)
        for k, v in extra_config:
            _git(d, "config", k, v, env=e)
        if attrs:
            (d / ".git" / "info" / "attributes").write_text("*.ipynb\tdiff=jupyternotebook\n")
    return setup


def merge_repo(base, local, remote, env, driver=MERGE_CMD):
    def setup(d):
        e = _env(env)
        _git(d, "init", "-q", "-b", "main", env=e)
        (d / "a.ipynb").write_text(base)
        _git(d, "add", "-A", env=e)
        _git(d, "commit", "-q", "-m", "base", env=e)
        _git(d, "checkout", "-q", "-b", "remote", env=e)
        (d / "a.ipynb").write_text(remote)
        _git(d, "commit", "-q", "-am", "remote", env=e)
        _git(d, "checkout", "-q", "main", env=e)
        (d / "a.ipynb").write_text(local)
        _git(d, "commit", "-q", "-am", "local", env=e)
        if driver:
            _git(d, "config", "merge.jupyternotebook.driver", driver, env=e)
            _git(d, "config", "merge.jupyternotebook.name", "jupyter notebook merge driver", env=e)
        (d / ".git" / "info" / "attributes").write_text("*.ipynb\tmerge=jupyternotebook\n")
    return setup


def plain_repo(env):
    def setup(d):
        e = _env(env)
        _git(d, "init", "-q", "-b", "main", env=e)
        (d / "a.ipynb").write_text("{}")
    return setup


def git_case(setup, args, **kw):
    case = {"cmd": "git", "args": args, "setup": setup, "path_bins": True, "mask": MASK}
    case.update(kw)
    return case


def test_cli_git_diff_driver(sandbox):
    root, env = sandbox
    texts = _texts("cli-git-diff", 6)
    assert len(texts) >= 5
    setup = diff_repo(texts, env)
    diffs = [["diff"], ["diff", "--cached"], ["diff", "HEAD~1"], ["diff", "HEAD~2", "HEAD"],
             ["diff", "-M", "HEAD~2", "HEAD"], ["diff", "--no-renames", "HEAD~2", "HEAD"],
             ["show", "--format=%s", "HEAD~1"], ["log", "-p", "--format=%s"],
             ["diff", "HEAD~2", "--", "sub"], ["diff", "--no-ext-diff"], ["diff", "--stat"]]
    cases = [git_case(setup, a) for a in diffs]
    for flags in ("-s", "-o", "-a -m", "-i -d", "--no-color", "--color-words", "-S"):
        cases.append(git_case(diff_repo(texts, env, driver=DIFF_CMD + " " + flags), ["diff", "HEAD~1"]))
    cases.append(git_case(diff_repo(texts, env, driver=DIFF_CMD + " --bogus"), ["diff", "HEAD~1"]))
    cases.append(git_case(diff_repo(texts, env), ["diff", "HEAD~1"], subdir="sub"))
    check_cli(cases, root, env, min_cases=len(cases))


def test_cli_git_diff_driver_filters(sandbox):
    """`--use-filter` re-applies the clean filter named in the attributes to the working-tree side."""
    root, env = sandbox
    texts = _texts("cli-git-filter", 6)
    strip = "%s -c \"import json,sys; nb=json.load(sys.stdin); " \
            "[c.update(outputs=[]) for c in nb['cells'] if 'outputs' in c]; " \
            "json.dump(nb, sys.stdout, indent=1)\"" % (REF_BIN / "python")
    filt = [("filter.strip.clean", strip), ("filter.noop.clean", "cat")]
    cases = []
    for flt in ("strip", "noop"):
        for driver in (DIFF_CMD, DIFF_CMD + " --use-filter"):
            base_setup = diff_repo(texts, env, driver=driver, extra_config=filt)

            def setup(d, base_setup=base_setup, flt=flt):
                base_setup(d)
                (d / ".git" / "info" / "attributes").write_text(
                    "*.ipynb\tdiff=jupyternotebook filter=%s\n" % flt)
            cases += [git_case(setup, a) for a in (["diff"], ["diff", "HEAD"])]
    check_cli(cases, root, env, min_cases=len(cases))


def test_cli_git_diff_driver_filter_paths_and_diagnostics(sandbox):
    root, env = sandbox
    notebook = '{"cells": [], "metadata": {}, "nbformat": 4, "nbformat_minor": 5}'

    def setup(d):
        plain_repo(env)(d)
        (d / "a.ipynb").write_text(notebook)
        (d / "dir.ipynb").mkdir()
        (d / ".git" / "info" / "attributes").write_text("*.ipynb filter=test\n")
        (d / "bad-config").write_text("[not a config\n")
        _git(d, "config", "filter.test.clean", f"printf '%s' '{notebook}'", env=env)

    def case(path, remote="a.ipynb", **options):
        return {"cmd": "git-nbdiffdriver", "setup": setup, **options,
                "args": ["diff", "--use-filter", path, "a.ipynb", "0", "100644",
                         remote, "0", "100644"]}

    cases = [case("a.ipynb", env={"GIT_CONFIG": "{dir}/bad-config"}),
             case("dir.ipynb", remote="dir.ipynb")]
    # Windows has no byte file names to decode.
    if os.name != "nt":
        cases.insert(0, case(os.fsdecode(b"a\xff.ipynb")))
    check_cli(cases, root, env, min_cases=3)


def test_cli_git_merge_driver(sandbox):
    root, env = sandbox
    rng = rng_for("cli-git-merge")
    cases = []
    tries = 0
    while len(cases) < 30 and tries < 200:
        tries += 1
        base = random_notebook(rng, n_cells=rng.choice([1, 3, 6]))
        local = mutate_notebook(rng, base, rng.choice([1, 2, 3]))
        remote = mutate_notebook(rng, base, rng.choice([1, 2, 3]))
        texts = [dumps_disk(x) for x in (base, local, remote)]
        if not all(_usable(t) for t in texts) or len(set(texts)) < 3:
            continue
        driver = rng.choice([MERGE_CMD, MERGE_CMD + " --merge-strategy use-local",
                             MERGE_CMD + " -s", MERGE_CMD + " --log-level ERROR"])
        cases.append(git_case(merge_repo(*texts, env, driver=driver),
                              ["merge", "-q", "--no-edit", "remote"],
                              files=["a.ipynb"], mask_new_ids=True))
    check_cli(cases, root, env, min_cases=30)


def configured_repo(env):
    """A repository where nbdime is already configured (for idempotence)."""
    def setup(d):
        plain_repo(env)(d)
        e = _env(env)
        _git(d, "config", "diff.jupyternotebook.command", "git-nbdiffdriver diff", env=e)
        (d / ".git" / "info" / "attributes").write_text("*.ipynb\tdiff=jupyternotebook\n")
        (d / "gconfig").write_text("[merge \"jupyternotebook\"]\n\tdriver = x\n")
        (d / "xdg" / "git").mkdir(parents=True)
        (d / "xdg" / "git" / "attributes").write_text("*.ipynb merge=jupyternotebook\n")
    return setup


def test_cli_git_config(sandbox):
    """config-git and the drivers' `config` subcommands: what they print and write. System scope
    is redirected into the sandbox with GIT_CONFIG_SYSTEM."""
    root, env = sandbox
    files = [".git/config", ".gitattributes", ".git/info/attributes", "gconfig", "sysconfig",
             "gitattributes", "xdg/git/attributes"]
    genv = {"GIT_CONFIG_GLOBAL": "{dir}/gconfig", "XDG_CONFIG_HOME": "{dir}/xdg",
            "GIT_CONFIG_SYSTEM": "{dir}/sysconfig", "GIT_CEILING_DIRECTORIES": str(root)}
    commands = [
        ("nbdime config-git", ["--enable"]), ("nbdime config-git", ["--disable"]),
        ("nbdime config-git", ["--enable", "--global"]), ("nbdime config-git", []),
        ("nbdime config-git", ["--foo"]), ("nbdime config-git", ["--enable", "--system"]),
        ("nbdime config-git", ["--disable", "--global"]),
        ("git-nbdiffdriver", ["config", "--enable"]), ("git-nbdiffdriver", ["config", "--disable"]),
        ("git-nbdiffdriver", ["config", "--enable", "--global"]), ("git-nbdiffdriver", []),
        ("git-nbdiffdriver", ["config", "--enable", "--system"]),
        ("git-nbmergedriver", ["config", "--enable"]), ("git-nbmergedriver", ["config", "--disable"]),
        ("git-nbmergedriver", ["config", "--enable", "--global"]), ("git-nbmergedriver", []),
        ("git-nbdifftool", ["config", "--enable"]), ("git-nbdifftool", ["config", "--disable"]),
        ("git-nbdifftool", ["config", "--enable", "--set-default"]),
        ("git-nbmergetool", ["config", "--enable"]), ("git-nbmergetool", ["config", "--disable"]),
        ("git-nbmergetool", ["config", "--enable", "--global"]),
        ("git-nbmergetool", ["config", "--enable", "--set-default"]),
    ]
    cases = []
    for cmd, args in commands:
        for setup in (plain_repo(env), configured_repo(env), lambda d: None):
            cases.append({"cmd": cmd, "args": args, "setup": setup, "files": files,
                          "env": dict(genv)})
    check_cli(cases, root, env, min_cases=len(cases))


def test_cli_nbpatch(sandbox):
    root, env = sandbox
    texts = _texts("cli-nbpatch", 8, intensity=3)
    d = root / "nbpatch"
    d.mkdir(exist_ok=True)
    # Upstream installs no `nbpatch` script; this is what `python -m nbdime.nbpatchapp` runs.
    refpatch = d / "upstream-nbpatch"
    refpatch.write_text("#!%s\nimport sys\nfrom nbdime.nbpatchapp import main\nsys.exit(main())\n"
                        % (REF_BIN / "python"))
    refpatch.chmod(0o755)
    cases = []
    for i in range(len(texts) - 1):
        a, b = d / ("%d-a.ipynb" % i), d / ("%d-b.ipynb" % i)
        a.write_text(texts[i])
        b.write_text(texts[i + 1])
        df = d / ("%d.json" % i)
        subprocess.run([str(REF_BIN / "nbdiff"), str(a), str(b), "--out", str(df)], env=env,
                       check=True, capture_output=True)
        cases.append({"cmd": "nbpatch", "ref_binary": refpatch, "args": [str(a), str(df)]})
        cases.append({"cmd": "nbpatch", "ref_binary": refpatch, "args": [str(a), str(df), "-o", "{out}"]})
        cases.append({"cmd": "nbpatch", "ref_binary": refpatch, "args": [str(a), str(df), "--output={out}"]})
    empty = d / "empty.json"
    empty.write_text("[]")
    bad = d / "bad.json"
    bad.write_text(json.dumps([{"op": "patch", "key": "nope", "diff": []}]))
    a0 = str(d / "0-a.ipynb")
    cases += [
        {"cmd": "nbpatch", "ref_binary": refpatch, "args": [a0, str(empty)]},
        {"cmd": "nbpatch", "ref_binary": refpatch, "args": [a0, str(bad)]},
        {"cmd": "nbpatch", "ref_binary": refpatch, "args": [a0, str(d / "missing.json")]},
        {"cmd": "nbpatch", "ref_binary": refpatch, "args": [str(d / "missing.ipynb"), str(empty)]},
        {"cmd": "nbpatch", "ref_binary": refpatch, "args": [a0]},
        {"cmd": "nbpatch", "ref_binary": refpatch, "args": []},
    ]
    check_cli(cases, root, env, min_cases=len(cases))
