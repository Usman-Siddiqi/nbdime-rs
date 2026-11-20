# Unit U14: git integration and nbpatch

Unit spec for implementer and fixer. The orchestrator owns it. One hostile review (owner
instruction 2026-11-20, `docs/STATE.md`).

## Scope
Port to `crates/nbdime-cli` (reuse the U07 modules: `argparse.rs`, `config.rs`, `args.rs`,
`gitfiles.rs`, `logging.rs`, `dispatch.rs`, `osstr.rs`, and the core `log`/`fsname` modules):
- `nbdime/vcs/git/diffdriver.py` → binary `git-nbdiffdriver` (`diff`, `config`; `webdiff` prints
  the U07 "not provided" message, exit 1, F-029).
- `nbdime/vcs/git/mergedriver.py` → `git-nbmergedriver` (`merge`, `config`).
- `nbdime/vcs/git/difftool.py`, `mergetool.py` → `git-nbdifftool`, `git-nbmergetool`: the
  `config` subcommand exactly; launching the web tool is out of scope ("not provided", exit 1).
- `nbdime/vcs/git/filter_integration.py` (`interrogate_filter`, `get_clean_filter_cmd`,
  `apply_possible_filter`; the U07 clean-filter code in `gitfiles.rs` may be shared).
- `nbdime config-git` in `__main__.py` and every `utils.py` helper they use
  (`locate_gitattributes` for each scope: none = repo `.git/info/attributes`? check upstream,
  `--global` = `core.attributesfile` or `$XDG_CONFIG_HOME/git/attributes`, `--system`;
  `ensure_dir_exists`, the exact text appended to attributes files).
- `nbdime/nbpatchapp.py` → binary `nbpatch` (reads a diff JSON, applies the U05 patch, writes
  JSON or prints).
- The `nbdime` dispatcher (`dispatch.rs`) routes `config-git`, `diff`/`merge`/... as upstream does
  for these commands.

Upstream sources: `/home/claude/ref/nbdime-upstream/nbdime/`. Porting guide sections 2, 10, 11;
findings F-022, F-026, F-029, F-031 (git invocation rules: pin child git to the found repo,
`/bin/sh` for filters, honour the user's git configuration).

## Semantics to get exactly right
- The driver argument lists git passes (`diff` driver: 7 args `path old-file old-hex old-mode
  new-file new-hex new-mode`, plus rename forms; merge driver `%O %A %B %L %P`), how upstream's
  parsers name and consume them (`add_git_diff_driver_args`, `add_filename_args`, `marker`,
  `out`), `opts.out = opts.local` and `decisions=False` for the merge driver, and the exit codes
  git sees (non-zero on conflicts).
- `--use-filter` / `apply_possible_filter` (the remote side is re-filtered from the working tree
  path), `NBDIME_*` environment, the merge driver writing to `%A`.
- Output of `config --enable/--disable` (`--global`, `--system`, none): the git config keys and
  values, the attributes line, idempotence, behaviour outside a repo (upstream prints to stderr
  and returns), and `--disable` when not set.
- Help/usage text: F-029 applies (ours may differ); exit codes and stdout must match.

## Acceptance
`scripts/gate.sh $(grep -v '^#' parity/enabled_suites.txt | sed 's|^|parity/|') parity/test_cli_git.py`
prints `GATE PASSED` (`test_cli_git.py`: differential cases running git with upstream's and our
drivers, config-git, nbpatch). Also run `parity/test_upstream_suite.py` (upstream's own
`test_cli_apps.py`, `test_git_diffdriver.py`, `test_git_filter_integration.py` against our
binaries) and report: every test must pass except `test_nbshow_app` (nbshow is unit U08, running
in parallel). Do not touch the suites.
