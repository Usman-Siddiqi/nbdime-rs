# Unit U06: terminal pretty-printer for diffs and merges

Unit spec for implementer, reviewers and fixer. The orchestrator owns it.

## Scope

Port `nbdime/prettyprint.py` to `crates/nbdime-core/src/prettyprint/` except the notebook
printer's syntax highlighting (`colorize_source`, `pretty_print_notebook`: unit U08, `nbshow`).
That is: `PrettyPrintConfig` (include flags from `diff_ignorables`, `should_ignore_path`, the
colorama constants), `format_value`, `_trim_base64`, `hash_string`, `file_timestamp`, every
`pretty_print_*` used by `pretty_print_notebook_diff` and `pretty_print_merge_decisions`, and the
text renderers `diff_render` / `merge_render` with their fallbacks (`diff_render_with_git`,
`diff_render_with_diff`, `diff_render_with_difflib`, `builtin_diff_render`,
`merge_render_with_git`, `merge_render_with_diff3`, `builtin_merge_render`,
`format_merge_render_lines`, `external_diff_render`, `external_merge_render`).

U10 already implements the merge renderers in `merging/text.rs` (spawning git / diff3 like
upstream, with private temp dirs). Move them into the pretty-printer module so diff and merge
rendering share one implementation of the external-command plumbing, and have the merge engine
call it. Rendering spawns the same commands as upstream (findings F-022, F-023): never
reimplement git's or GNU diff's algorithms. `which` semantics: upstream uses
`shutil.which('git')` / `which('diff')` / `which('diff3')` on `PATH`.

Read the upstream file yourself (about 1,000 lines). Porting guide: sections 2, 4, 10, 11.

## Semantics to get exactly right

- Output is a byte-exact match of what upstream writes to `config.out`, including colorama codes
  (`Fore.RED` = `\x1b[31m`, `Fore.GREEN` = `\x1b[32m`, `Fore.BLUE + Style.BRIGHT` =
  `\x1b[34m\x1b[1m`, `Style.RESET_ALL` = `\x1b[0m`), `IND = "  "`, `MAXWIDTH = 78`.
- `pprint.pformat` (default width 80, indent 1, `sort_dicts=True`) on JSON values: port the
  relevant parts of CPython 3.13's `pprint` exactly: `_safe_repr` (dict keys sorted by
  `_safe_key`), `_format` with the `_dispatch` for dict, list and str (long strings inside
  containers are split into implicitly concatenated chunks by `_pprint_str`), the width and
  `allowance` arithmetic, and `repr` of str (quote choice, escapes, `str.isprintable` per code
  point: needs the Unicode general category table of Python 3.13 / Unicode 15.1), int (arbitrary
  size), float (`py_float_repr`, including `inf`/`nan`), bool and None. Containers nested in
  values come from notebook JSON; recursion by depth is fine.
- `_trim_base64`: snips when `len(s) > 64` (code points) and the base64 regex `re.match`es
  `s.replace('\n', '')` (with `\n` gone, `$` under `re.MULTILINE` matches only at the end, so it
  is a full match; other line breaks such as `\r` are not removed and make it fail). The MD5 is of
  the UTF-8 encoding of the original `s` (`md-5` crate, MIT/Apache, is fine).
- `file_timestamp`: `datetime.fromtimestamp(mtime).isoformat(" ")` in local time (microseconds
  printed only when non-zero), or `(no timestamp)` when the path does not exist.
- `should_ignore_path` uses `star_path(split_path(path))` (exists in `path.rs` since U10).
- `pretty_print_value_at` path handling: `path.split('/', 1)` when the path does not start with
  `/`; a path of `""` raises upstream (keep the error).
- `pretty_print_dict` sorts keys (`sorted(set(d) - set(exclude_keys))`, so Python str ordering by
  code point); `pretty_print_metadata` keeps insertion order of known/unknown groups but each
  group is printed via `pretty_print_dict` (sorted).
- Truthiness checks (`if v:` on output fields, `if id and config.details`, empty lists/dicts/0/
  `""`), `type(aval) is not type(bval)` with Python type names (`int`, `float`, `str`, `bool`,
  `NoneType`, `list`, `dict`) in "(type changed from X to Y)".
- `pretty_print_string_diff` patches `a` with the diff (U05 `patch`) and renders through
  `diff_render(a, b, config)`: git when `use_git` and git is on `PATH`, else `diff` when
  `use_diff` and `diff` is on `PATH`, else difflib's `unified_diff` with `lineterm=''` (port
  `unified_diff`/`get_grouped_opcodes` on top of the existing `difflib.rs` if needed).
- `pretty_print_merge_decision`: `config.INFO.replace("##", "====")`, the `either`/`selected`
  notes, the string-subindex wrap in `op_patch`, and `pretty_print_dict(decision, exclude_keys)`
  which prints the remaining decision keys (e.g. `strategy`) via `format_value`.
- Errors where upstream raises (e.g. `NBDiffFormatError` for unknown ops, invalid types, a
  `""` path) are errors here; never a panic.

## Performance

Linear in the size of the output. No process spawn for strings without a newline. The external
renderers spawn one process per multi-line string, as upstream; that is the accepted cost
(F-022).

## Dev tool functions (fixed by the parity suite)

| fn | args | returns |
|---|---|---|
| `pp.notebook_diff` | `{afn, bfn, a, di, include?, use_color, color_words, use_git, use_diff}` | str |
| `pp.merge_decisions` | `{base, decisions, include?, use_color, use_git, use_diff}` | str |
| `pp.value` | `{value, path: str\|null, prefix, include?, use_color}` | str |
| `git.diff_render` | `{a, b, use_color, color_words}` | str (`diff_render_with_git`) |
| `git.merge_render` | `{base, local, remote, strategy}` | `[merged, status]` (`merge_render_with_git`) |

`include` maps any of `sources, outputs, attachments, metadata, id, details` to booleans; absent
keys are true; absent `include` means all true. `a` and `base` are notebooks as `nbformat.read`
returns them.

## Acceptance

`scripts/gate.sh` with all enabled suites plus `parity/test_u06_prettyprint.py` and
`parity/test_u16_xdiff.py` (the git renderer suite) prints `GATE PASSED`; also the U06 suite at
`PARITY_SCALE=2` with `PARITY_SEED=` 1 and 2. Two hostile reviews.
