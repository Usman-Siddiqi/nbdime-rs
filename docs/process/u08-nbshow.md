# Unit U08: nbshow

Unit spec for implementer and fixer. The orchestrator owns it. One hostile review (owner
instruction 2026-11-20, `docs/STATE.md`).

## Scope
- `nbdime/nbshowapp.py` → binary `nbshow` (argparse via U07's `argparse.rs`, config sections as
  upstream's MRO, `-` for stdin, several files with the `::::::::::::::` banners, `Missing file`,
  exit codes), and `nbdime show` in the dispatcher.
- `prettyprint.pretty_print_notebook`, `pretty_print_source`'s highlighting path and
  `colorize_source` (in `crates/nbdime-core/src/prettyprint/`). nbshow always highlights
  (there is no colour option): markdown cells with the existing Pygments `MarkdownLexer` port,
  code cells with the lexer named by `language_info.pygments_lexer`, else `language_info.name`,
  through Pygments 2.21's `find_lexer_class_by_name` and `Terminal256Formatter`.
- Lexers to port exactly (Pygments 2.21, `/home/claude/ref/venv/lib/python3.13/site-packages/pygments`):
  `PythonLexer` (aliases python, py, sanic, python3, py3, bazel, starlark, pyi) and whatever it
  delegates to; also make fenced code blocks in markdown use it. Port the regex-lexer machinery
  (states, `bygroups`, `using`, `include`, `default`, `words`, `combined`, token types and
  Terminal256 style lookups) generally so more lexers can be added later.
- Decision (F-032, write it): `ipython`/`ipython3` and other IPython lexers are not registered
  unless IPython is installed; a plain `pip install nbdime` has no IPython, and the oracle venv has
  none, so `ipython3` notebooks print unhighlighted source exactly like upstream there. Every other
  lexer name Pygments knows but we do not port: print the source unhighlighted and record the
  deviation in F-032 (list which aliases are affected). Names Pygments does not know:
  unhighlighted, as upstream (`ClassNotFound`).

## Acceptance
`scripts/gate.sh $(grep -v '^#' parity/enabled_suites.txt | sed 's|^|parity/|') parity/test_cli_nbshow.py`
prints `GATE PASSED`, and `parity/test_upstream_suite.py::...test_nbshow_app` passes (run
`parity/test_upstream_suite.py` and report; failures of git-driver tests belong to unit U14,
running in parallel). Do not touch the suites.
