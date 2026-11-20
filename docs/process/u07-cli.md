# Unit U07: the `nbdiff`, `nbmerge` and `nbdime` command lines

Unit spec for implementer, reviewers and fixer. The orchestrator owns it. Covers roadmap U07
(nbdiff) and U13 (nbmerge); `nbshow`/`nbpatch` (syntax highlighting) and the git drivers come
later.

## Scope

Binaries in `crates/nbdime-cli` (anyhow allowed here): `nbdime` (dispatcher: `nbdime diff`,
`nbdime merge`; other subcommands print upstream's message for an unknown/unsupported command and
exit like upstream), `nbdiff`, `nbmerge`. Port `nbdiffapp.py`, `nbmergeapp.py`, the relevant
parts of `args.py`, `config.py` (nbdime_config.json loading), `utils.py` (`read_notebook`,
`EXPLICIT_MISSING_FILE`), `gitfiles.py` (`is_gitref`, `changed_notebooks`), `log.py` and
`__main__.py`. Read them yourself.

Behaviour to reproduce exactly (the parity suite `parity/test_cli.py` compares exit code,
stdout, output files and stderr byte for byte, see its docstring for the policy):
- argparse semantics of Python 3.13 for these parsers: option prefixes/abbreviations
  (`--no-col` works), `-s -o` combined as `-so`, `--opt=value`, `--` handling, optional
  positionals with defaults (`base` defaults to `HEAD`, nbmerge's base to `/dev/null`), choices
  validation, the `IgnorableAction` pairs (`-s/-S`, `--sources/--ignore-sources`, ...),
  `process_exclusive_ignorables` (mixed positive/negative is an error), exit code 2 with
  `usage: ...` on stderr for usage errors (the text itself is not compared, F-029).
- `ConfigBackedParser`: defaults from `nbdime_config.json` files found on Jupyter's config path,
  per entry point section (`NbDiff`, `NbMerge`, `Diff`, `Merge`, `Global`, ... see `config.py`
  for the class hierarchy and how sections combine), including `Ignore` dicts passed to
  `set_notebook_diff_ignores`; invalid values are ignored the way upstream ignores them
  (`except ValueError: pass`). Config path (F-029): `$JUPYTER_CONFIG_DIR` (default `~/.jupyter`),
  each entry of `$JUPYTER_CONFIG_PATH`, `$VIRTUAL_ENV/etc/jupyter` or `$CONDA_PREFIX/etc/jupyter`
  if set, `/usr/local/etc/jupyter`, `/etc/jupyter`, in jupyter_core's priority order.
- Logging: `[%(levelname)1.1s %(module)s:%(lineno)d] %(message)s` on stderr; reproduce the exact
  module names and line numbers upstream would print for every message these commands can emit
  at INFO and above (they are part of the stderr comparison), and `--log-level` filtering.
- nbdiff: file mode, `/dev/null` as either side, missing files (`Missing file X` on stdout, exit
  1), `--out` (JSON `indent=2, separators=(",", ": ")`, no trailing newline, exactly like
  `json.dump`), pretty-printed output through U06, gitref mode (`is_gitref`,
  `resolve_diff_args`, `changed_notebooks` via `git` subprocesses exactly as upstream calls
  them, including filters with `--use-filter` if exposed here), exit codes.
- nbmerge: `read_notebook` with `on_null`/`on_empty`, agreed deletion, `--out` (nbformat.write,
  including the validation error log upstream emits for invalid results, e.g. marker cells with
  ids in a minor < 5 notebook: the stderr comparison is relaxed for those, but the file and the
  exit code are compared), `--decisions` (pretty-printed through `logger.warning`), exit code 1
  on conflicts, merged notebook on stdout via `nbformat.write`.
- Errors where upstream prints a traceback: exit code 1 and a non-empty stderr (the text is not
  compared). Never panic.
- stdout/stderr encoding: UTF-8 with backslashreplace for anything unencodable (strings are
  valid UTF-8 here, F-005).

## Performance

Startup well under 10 ms; no work proportional to anything but the inputs.

## Acceptance

`scripts/gate.sh` with all enabled suites, `parity/test_u06_prettyprint.py` and
`parity/test_cli.py` prints `GATE PASSED`. Two hostile reviews (one combined CLI/plumbing review
is acceptable per the owner, 2026-11-19).
