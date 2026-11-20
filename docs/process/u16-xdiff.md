# Unit U16: git's text diff and three-way merge, without git

Unit spec for implementer, reviewers and fixer. The orchestrator owns it.

## Why

With git on `PATH` (the normal case), upstream renders every modified multi-line string in
`nbdiff`/`nbshow` output by writing two temp files and running
`git diff --no-index --color-words before after` (or `--color`, or neither), and resolves every
inline source conflict in `nbmerge` by running `git merge-file -p local base remote`
(`prettyprint.py`: `external_diff_render`, `diff_render_with_git`, `external_merge_render`,
`merge_render_with_git`). nbdime-rs reproduces that output in-process. Spawning git per string
is most of upstream's pretty-print time and would make a drop-in replacement depend on the
installed git version and the user's git configuration.

The target is **git 2.43 with no system or global configuration** (findings F-022). That is the
git the parity suite runs (`parity/conftest.py` sets `GIT_CONFIG_NOSYSTEM=1`,
`GIT_CONFIG_GLOBAL=/dev/null`).

## Scope

New module `crates/nbdime-core/src/xdiff/` (split into files as you see fit):

```rust
pub enum DiffRenderMode { Plain, Color, ColorWords }
/// Upstream's `diff_render_with_git(a, b, config)`, including the stripped header and the
/// removed "\ No newline at end of file" lines.
pub fn diff_render(a: &str, b: &str, mode: DiffRenderMode) -> String;

pub enum MergeFavor { Ours, Theirs, Union }
/// Upstream's `merge_render_with_git(base, local, remote, strategy)`: (merged text, git's exit
/// status, i.e. the number of conflicts capped at 127).
pub fn merge_render(base: &str, local: &str, remote: &str, favor: Option<MergeFavor>)
    -> Result<(String, i32), MergeRenderError>;
```

**Licensing (F-023, binding).** git's xdiff is LGPL-2.1+ and git itself GPL-2.0; nbdime-rs is
BSD-3-Clause. Do **not** translate git's C sources into Rust. Allowed:
- permissively licensed crates (MIT/Apache/BSD), preferably `imara-diff` 0.2 (Myers plus a
  git-style indent-heuristic postprocessor) for the line diff, if its output matches git on the
  suite; if it does not, write the diff yourself from the published algorithms (Myers 1986, the
  slider/indent heuristic as described in git's documentation and commit messages) and fit it to
  git's observed output; `gix-merge`'s text merge (MIT/Apache) may be read as a reference for
  merge-file behavior, but do not pull the gitoxide dependency tree in;
- running the `git` binary as a black-box oracle while developing (the suite already does).
Keep dependencies lean (pure Rust, small trees) and list each new one with its license in your
report. Everything that shapes the output (header, hunks, funcname, colors, word diff, merge
markers) is ours.

What git does by default and therefore must be reproduced:
- Myers diff with the `xdl_change_compact` slider post-processing **and the indent heuristic**
  (`diff.indentHeuristic` defaults to true since git 2.14), context 3, interhunk context 0.
- Hunk headers `@@ -a,b +c,d @@` with git's comma omission rules, followed by the funcname
  context from the **default** funcname matcher (a line starting with a letter, `_` or `$`,
  truncated as git truncates it), looking backwards from the hunk start.
- Binary detection: a NUL byte in the first 8000 bytes of either side makes git print
  `Binary files a/before and b/after differ`; after upstream drops the first 4 lines the result is
  empty (check exactly what upstream returns).
- Identical inputs: no output.
- Header: 4 lines are dropped by upstream (`diff --git`, `index`, `---`, `+++`). With an empty
  side git prints `new file mode`/`deleted file mode`? It does not for `--no-index` with two
  existing files, but verify: an empty `before` is still a regular existing file.
- Colors (`--color`, default `color.diff.*` slots): meta bold, frag cyan, old red, new green,
  context plain, func plain, whitespace errors highlighted (`core.whitespace` default:
  `blank-at-eol`, `space-before-tab`, `blank-at-eof`) on added lines only, `ESC[m` resets
  exactly where git emits them.
- `--color-words` with the default word regex (runs of non-whitespace), git's word diff
  algorithm (it diffs the word lists with xdiff, then emits), colors and resets as git emits
  them, and how it prints hunk headers and context.
- `\ No newline at end of file` handling: upstream removes those lines with a regex
  (`^\\ No newline at end of file\n?`, multiline). With color git may wrap the marker in escape
  codes; whatever survives upstream's regex is part of the expected output.
- Input bytes are the UTF-8 encoding of the Python strings (no newline translation on Linux).
  Output is decoded as UTF-8 by upstream, so it is always valid. CR characters are ordinary
  bytes for diff (`a\r\n` lines), but whitespace-error highlighting treats a trailing CR as git
  does (`cr-at-eol` is off by default).
- Merge: `git merge-file -p` defaults: `XDL_MERGE_ZEALOUS_ALNUM` conflict level, style `merge`
  (no base section), marker size 7, labels `local` / `remote` (git labels with the file names),
  `--ours`/`--theirs`/`--union` favor; exit status = conflict count capped at 127. Upstream then
  replaces `\r\n` with `\n` in the whole output and, if the last line contains `\n` and
  `>>>>>>>`, strips trailing whitespace (`rstrip()`, all of it). When git's output is empty,
  upstream raises `IndexError` on `lines[-1]`: return `MergeRenderError` in that case (both
  sides erroring is agreement in the harness).
- Files without a trailing newline: git's merge adds/omits newlines at conflict markers in
  specific ways (`xdl_fill_merge_buffer`, `is_cr_needed`, `is_eol_crlf`); reproduce byte for byte.

Out of scope: other git options, diff algorithms other than the default, rename detection,
`diff3` style, user configuration. Upstream's non-git fallbacks (`diff`, difflib,
`builtin_merge_render`) belong to the pretty-printer and merge units, if at all.

## Performance

xdiff is linear in practice (Myers with git's heuristics and `XDL_MAX_COST` bailout); keep the
same cost limits so results match on large inputs. Everything iterative. No per-call process
spawn, no temp files.

## Dev tool functions (fixed by the parity suite)

| fn | args | returns |
|---|---|---|
| `git.diff_render` | `{a, b, use_color: bool, color_words: bool}` | str (`use_color=false` ignores `color_words`) |
| `git.merge_render` | `{base, local, remote, strategy: null \| "use-local" \| "use-remote" \| "union"}` | `[merged, status]`; any other strategy string behaves as null (upstream logs a warning) |

## Acceptance

`scripts/gate.sh parity/test_json_io.py parity/test_pilot_generic_diff.py parity/test_u05_patch.py parity/test_u02_nbformat.py parity/test_u16_xdiff.py`
prints `GATE PASSED`; also run the U16 suite with `PARITY_SCALE=4` and `PARITY_SEED=` 1 and 2, and report.
Two hostile reviews (A correctness, B lookalikes: here "lookalikes" means C-vs-Rust: signed vs
unsigned arithmetic, byte vs char, `<` vs `<=` in the heuristics, integer overflow in costs).
