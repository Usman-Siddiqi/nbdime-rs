# nbdime upstream test suite inventory

Source: `/home/claude/ref/nbdime-upstream/nbdime/tests/` (read-only). Counts are `def test_*` functions (AST count, parametrized cases not expanded). Unit codes follow the port-unit list. Compiled by reading imports, fixtures and external-call sites; no tests were run, no network, no git operations.

## Per test file

| File | Tests | Units (count) | External needs | Notes |
|---|---:|---|---|---|
| `test_apply_merge.py` | 2 | U10 | none | Imports diff/patch (U05) and merging.decisions apply_decisions. |
| `test_args.py` | 5 | U04(3), U07(2) | none (tmpdir, config fixtures) | U04: ignore-config tests (3); U07: SkipAction and ConfigBackedParser tests (2). |
| `test_autoresolve.py` | 5 | U12 | git: `needs_git` fixture on 1 test (test_autoresolve_inline_source_conflict); no git commands in test body | Autoresolve strategies via merge_notebooks; also builds nbmerge parser (U13 CLI parser). |
| `test_cli_apps.py` | 35 | U07(12), U08(1), U09(1), U13(7), U14(12), U15(2) | git (git_repo/git_repo2, `git` CLI via call/check_output, git config); hg (hg_repo, `hg` CLI); subprocess (check_call of `python -m nbdime.nbdiffapp`, Popen of servers); web (tornado + requests, wait_up) in 4 tests | Largest CLI file. U14 web-dependent tests: test_git_difftool, test_git_mergetool, test_hg_diffweb, test_hg_mergetool (launch a web server and call it with requests). U15: test_config_git, test_config_git_fails use main_dispatch. |
| `test_decision_tools.py` | 13 | U10 | none | Contains 1 xfail marker (test_build_diffs_unsorted). |
| `test_diff.py` | 2 | P1 | none | Covers diff() round trip via check_symmetric_diff_and_patch and compute_snakes_multilevel (snakes). |
| `test_diff_format.py` | 4 | U04 | none (jsonschema Draft4Validator) | Validates diff_notebooks output against JSON schema; uses matching_nb_pairs fixture. |
| `test_diff_gitrefs.py` | 20 | U14 | git (git_repo2 fixture creating a repo; GitPython `git` package) | Git ref comparisons (HEAD vs workdir, ref vs ref, subdir/filename/path variants) via tests/gitfiles.py helper. |
| `test_diff_json_conversion.py` | 2 | U05 | none (jsonpatch library) | Test body contains pytest.xfail('Not comparing to jsonpatch') branch. |
| `test_diff_performance.py` | 1 | U04 | none (timeout 20s marker) | Performance smoke test on large random text outputs. |
| `test_diff_sequence.py` | 1 | P1 | none | Single function parametrized over `algorithm` (case count not computed). |
| `test_diff_sequence_bruteforce.py` | 1 | P1 | none | Compares brute-force LCS/compare grids to diff_from_lcs. |
| `test_diff_sequence_difflib.py` | 1 | P1 | none | Uses difflib-based sequence diff. |
| `test_directory_differ.py` | 4 | UNASSIGNED | none (tmpdir, filespath); uses files/src-and-output--*.ipynb | Directory diff (diffing/directorydiff). No port unit in the list; nearest is U04. Filesystem only. |
| `test_git_diffdriver.py` | 8 | U14(7), OUT(1) | git (git_repo fixture, git config, git filter via filters/*.py); web (test_git_web_diff_driver: tornado ioloop + web server) | 1 test is web-dependent (OUT). |
| `test_git_filter_integration.py` | 12 | U14 | git (git_repo fixture; git config via call; subprocess CalledProcessError) | git filter integration. |
| `test_hg_differ.py` | 4 | U14(3), OUT(1) | hg (needs_hg fixture, hg CLI config via mock); web (test_hg_web_diff_driver: tornado + web server) | 3 hg diff-driver tests in U14; 1 web diff driver test OUT. |
| `test_js_artifacts_installed.py` | 1 | OUT | none (filesystem check of webapp static nbdime.js) | Checks webapp static asset presence. |
| `test_merge.py` | 16 | U11 | none | Generic merge of lists, dicts, nested structures; 1 xfail(run=False). |
| `test_merge_format.py` | 4 | U13 | none (jsonschema Draft4Validator) | Validates decide_notebook_merge output against merge JSON schema; uses matching_nb_triplets. |
| `test_merge_notebooks.py` | 34 | U12(16), U13(18) | git: `have_git` conditional inside one test (line ~632); no git CLI calls in body | U12: strategies (input/output/metadata union, mix, autoresolve EC/empty). U13: merge_notebooks, cell merges, only_sources/outputs/metadata. 1 xfail marker. |
| `test_merge_notebooks_inline.py` | 36 | U11(16), U12(20) | git: comments only (no git calls) | U11: decide_merge_strategy/list/dict transient tests (16). U12: test_inline_merge_* (20) conflict-marker output. |
| `test_myers.py` | 3 | P1 | none | No nbdime imports: self-contained greedy SES (Myers) helper code defined in the file; tests named test_greedy_*. |
| `test_notebook_diff.py` | 6 | U04 | none | Uses any_nb_pair (all pairs of all files/*.ipynb), matching_nb_pairs, db; also covers patch_notebook (U05 surface). |
| `test_package.py` | 1 | U15 | none | Assertion checks 'ndime.webapp' (typo; the name 'nbdime.webapp' is never checked), so it cannot fail as written. |
| `test_patch.py` | 3 | U05 | none | patch on str, list and dict. |
| `test_prettyprint.py` | 21 | U06 | none | Pretty-printer for dicts, lists, strings, base64 multiline, cells, and diffs. |
| `test_server_extension.py` | 10 | OUT | web/server: jupyter server or notebook process (popen_with_terminator), HTTP via requests, git_repo/git_repo2 fixtures in some tests, needs_symlink | Entire file is server extension (OUT). |
| `test_utils.py` | 5 | UNASSIGNED(1), U14(4) | git (git_repo fixture, needs_git on 2 tests; reads global/system gitattributes) | test_string_to_lists covers nbdime.utils string helper (no listed unit); the other 4 are is_repo and locate_gitattributes (U14). |
| `test_web.py` | 8 | OUT | web: tornado web server + requests; git_repo2 in 1 test; auth token | Entire file is web (OUT). |

### nbdime imports per file

| File | nbdime-side imports |
|---|---|
| `test_apply_merge.py` | nbdime: diff, patch; diff_format.op_patch; merging.decisions apply_decisions, ensure_common_path, MergeDecision |
| `test_args.py` | args: SkipAction, ConfigBackedParser, LogLevelAction; config: entrypoint_configurables, Global, _Ignorables; diffing.notebooks: notebook_differs, diff |
| `test_autoresolve.py` | nbdime: merge_notebooks; nbmergeapp._build_arg_parser |
| `test_cli_apps.py` | nbdime.__main__ main_dispatch; args process_exclusive_ignorables; diffing.notebooks notebook_differs; nbdiffapp main_diff; nbmergeapp main_merge; nbshowapp main_show; nbpatchapp; vcs.git diffdriver/difftool/mergedriver/mergetool; vcs.hg diff/diffweb/merge/mergeweb |
| `test_decision_tools.py` | diff_format op_remove, op_patch; merging.decisions ensure_common_path, MergeDecisionBuilder, MergeDecision, pop_patch_decision, build_diffs |
| `test_diff.py` | diff; diffing.snakes compute_snakes_multilevel; diff_format op_*; utils check_symmetric_diff_and_patch |
| `test_diff_format.py` | nbdime: diff, diff_notebooks |
| `test_diff_gitrefs.py` | tests.gitfiles changed_notebooks; nbdime.utils EXPLICIT_MISSING_FILE; GitPython |
| `test_diff_json_conversion.py` | nbdime: diff; diff_utils to_clean_dicts, to_json_patch |
| `test_diff_performance.py` | diffing diff_notebooks |
| `test_diff_sequence.py` | diffing.sequences diff_sequence; diff_format is_valid_diff; patch |
| `test_diff_sequence_bruteforce.py` | diffing.lcs diff_from_lcs; diffing.seq_bruteforce (bruteforce_compare_grid, bruteforce_llcs_grid, bruteforce_lcs_indices, diff_sequence_bruteforce); patch |
| `test_diff_sequence_difflib.py` | diffing.seq_difflib diff_sequence_difflib; patch |
| `test_directory_differ.py` | diffing.directorydiff diff_directories; utils EXPLICIT_MISSING_FILE |
| `test_git_diffdriver.py` | vcs.git.diffdriver main; prettyprint file_timestamp; utils locate_gitattributes; tornado; call helper |
| `test_git_filter_integration.py` | vcs.git.filter_integration interrogate_filter, apply_possible_filter, get_clean_filter_cmd; utils locate_gitattributes |
| `test_hg_differ.py` | vcs.hg.diff main; vcs.hg.diffweb main; prettyprint file_timestamp; tornado |
| `test_js_artifacts_installed.py` | nbdime.webapp __file__ |
| `test_merge.py` | nbdime: decide_merge, apply_decisions, diff; diff_format op_add/remove/replace/addrange/removerange |
| `test_merge_format.py` | nbdime: decide_merge; merging.notebooks decide_notebook_merge |
| `test_merge_notebooks.py` | nbdime: merge_notebooks, apply_decisions, diff_notebooks, set_notebook_diff_targets; merging.notebooks decide_merge_with_diff, Strategies; nbmergeapp._build_arg_parser |
| `test_merge_notebooks_inline.py` | nbdime: merge_notebooks, diff; diffing.config DiffConfig; merging.decisions apply_decisions; merging.generic decide_merge, decide_merge_with_diff; merging.strategies _cell_marker_format; utils Strategies |
| `test_myers.py` | none (only operator) |
| `test_notebook_diff.py` | nbdime: patch, patch_notebook, diff_notebooks; diffing.notebooks diff_cells |
| `test_package.py` | nbdime (import only) |
| `test_patch.py` | nbdime: patch; diff_format op_* |
| `test_prettyprint.py` | nbdime: prettyprint; diffing diff; nbformat v4; base64 |
| `test_server_extension.py` | tests.utils call, pushd, WEB_TEST_TIMEOUT, TEST_TOKEN; requests |
| `test_utils.py` | nbdime.utils strings_to_lists, revert_strings_to_lists, is_in_repo, locate_gitattributes |
| `test_web.py` | nbdime.webapp.nbdiffweb, nbdime.webapp.nbmergeweb; tornado; requests |

Units with multiple entries in one row are split per function; see Notes.

## Data files (tests/files and tests/filters)

Direct = a test/helper file names the file (quoted name or stem). Fixture-driven = reached through conftest notebook-DB fixtures (`any_nb_pair` enumerates every `files/*.ipynb`; matching fixtures use `basename--*` groups).

| Data file | Referenced directly by | Fixture-driven use |
|---|---|---|
| `files/apap--1.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'apap' via matching fixtures |
| `files/apap--2.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'apap' via matching fixtures |
| `files/apap--3.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'apap' via matching fixtures |
| `files/apap.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'apap' via matching fixtures; basename group 'apap' (matching fixtures) |
| `files/attachment--change_attachment.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'attachment' via matching fixtures |
| `files/attachment--empty_attachments_list.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'attachment' via matching fixtures |
| `files/attachment--remove_attachment.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'attachment' via matching fixtures |
| `files/attachment.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'attachment' via matching fixtures; basename group 'attachment' (matching fixtures) |
| `files/cellids--base.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'cellids' via matching fixtures |
| `files/cellids--local.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'cellids' via matching fixtures |
| `files/cellids--remote.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'cellids' via matching fixtures |
| `files/empty.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs) |
| `files/error--1.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'error' via matching fixtures |
| `files/error--2.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'error' via matching fixtures |
| `files/foo--1.ipynb` | test_diff_gitrefs.py, test_git_diffdriver.py, test_git_filter_integration.py, test_hg_differ.py, test_server_extension.py, conftest.py | test_notebook_diff.py any_nb_pair (all pairs); group 'foo' via matching fixtures |
| `files/foo--2.ipynb` | test_git_diffdriver.py, test_hg_differ.py, conftest.py | test_notebook_diff.py any_nb_pair (all pairs); group 'foo' via matching fixtures |
| `files/inline-conflict--1.ipynb` | test_autoresolve.py, test_cli_apps.py, conftest.py | test_notebook_diff.py any_nb_pair (all pairs); group 'inline-conflict' via matching fixtures |
| `files/inline-conflict--2.ipynb` | test_autoresolve.py, test_cli_apps.py, conftest.py | test_notebook_diff.py any_nb_pair (all pairs); group 'inline-conflict' via matching fixtures |
| `files/inline-conflict--3.ipynb` | test_autoresolve.py, test_cli_apps.py, conftest.py | test_notebook_diff.py any_nb_pair (all pairs); group 'inline-conflict' via matching fixtures |
| `files/inline-conflict--decisions.json` | test_cli_apps.py | - |
| `files/markdown-only--1.ipynb` | conftest.py | test_notebook_diff.py any_nb_pair (all pairs); group 'markdown-only' via matching fixtures |
| `files/markdown-only--2.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'markdown-only' via matching fixtures |
| `files/mixed-conflicts--1.ipynb` | test_merge_notebooks.py | test_notebook_diff.py any_nb_pair (all pairs); group 'mixed-conflicts' via matching fixtures |
| `files/mixed-conflicts--2.ipynb` | test_merge_notebooks.py | test_notebook_diff.py any_nb_pair (all pairs); group 'mixed-conflicts' via matching fixtures |
| `files/mixed-conflicts--3.ipynb` | test_merge_notebooks.py | test_notebook_diff.py any_nb_pair (all pairs); group 'mixed-conflicts' via matching fixtures |
| `files/multi_cell_nb--cellchange.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'multi_cell_nb' via matching fixtures |
| `files/multi_cell_nb--local.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'multi_cell_nb' via matching fixtures |
| `files/multi_cell_nb--remote.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'multi_cell_nb' via matching fixtures |
| `files/multi_cell_nb.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'multi_cell_nb' via matching fixtures; basename group 'multi_cell_nb' (matching fixtures) |
| `files/multilevel-test-base-local-diff.json` | test_cli_apps.py | - |
| `files/multilevel-test-base.ipynb` | test_cli_apps.py, test_web.py, conftest.py | test_notebook_diff.py any_nb_pair (all pairs) |
| `files/multilevel-test-local.ipynb` | test_cli_apps.py, test_web.py, conftest.py | test_notebook_diff.py any_nb_pair (all pairs) |
| `files/multilevel-test-merged.ipynb` | test_cli_apps.py | test_notebook_diff.py any_nb_pair (all pairs) |
| `files/multilevel-test-remote.ipynb` | test_cli_apps.py, test_web.py, conftest.py | test_notebook_diff.py any_nb_pair (all pairs) |
| `files/output-conflict--1.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'output-conflict' via matching fixtures |
| `files/output-conflict--2.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'output-conflict' via matching fixtures |
| `files/output-conflict--3.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'output-conflict' via matching fixtures |
| `files/single_cell_nb--changed_ec.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'single_cell_nb' via matching fixtures |
| `files/single_cell_nb--changed_metadata.ipynb` | test_cli_apps.py | test_notebook_diff.py any_nb_pair (all pairs); group 'single_cell_nb' via matching fixtures |
| `files/single_cell_nb--changed_output.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'single_cell_nb' via matching fixtures |
| `files/single_cell_nb--changed_source.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'single_cell_nb' via matching fixtures |
| `files/single_cell_nb--changed_source_output.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'single_cell_nb' via matching fixtures |
| `files/single_cell_nb--changed_source_output_ec.ipynb` | test_cli_apps.py | test_notebook_diff.py any_nb_pair (all pairs); group 'single_cell_nb' via matching fixtures |
| `files/single_cell_nb--json_output.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'single_cell_nb' via matching fixtures |
| `files/single_cell_nb--json_output_changed.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'single_cell_nb' via matching fixtures |
| `files/single_cell_nb.ipynb` | test_cli_apps.py | test_notebook_diff.py any_nb_pair (all pairs); group 'single_cell_nb' via matching fixtures; basename group 'single_cell_nb' (matching fixtures) |
| `files/source-conflict--1.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'source-conflict' via matching fixtures |
| `files/source-conflict--2.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'source-conflict' via matching fixtures |
| `files/source-conflict--3.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'source-conflict' via matching fixtures |
| `files/src-and-output--1.ipynb` | test_directory_differ.py, test_git_diffdriver.py, test_server_extension.py, test_web.py, conftest.py | test_notebook_diff.py any_nb_pair (all pairs); group 'src-and-output' via matching fixtures |
| `files/src-and-output--2.ipynb` | test_directory_differ.py, test_server_extension.py, test_web.py, conftest.py | test_notebook_diff.py any_nb_pair (all pairs); group 'src-and-output' via matching fixtures |
| `files/test-data-singlecell--1.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'test-data-singlecell' via matching fixtures |
| `files/test-data-singlecell--2.ipynb` | - | test_notebook_diff.py any_nb_pair (all pairs); group 'test-data-singlecell' via matching fixtures |
| `files/unicode--1.ipynb` | test_cli_apps.py | test_notebook_diff.py any_nb_pair (all pairs); group 'unicode' via matching fixtures |
| `files/unicode--2.ipynb` | test_cli_apps.py | test_notebook_diff.py any_nb_pair (all pairs); group 'unicode' via matching fixtures |
| `filters/add_helper.py` | test_git_diffdriver.py | - |
| `filters/noop.py` | test_git_diffdriver.py | - |
| `filters/strip_outputs.py` | test_git_diffdriver.py | - |

## Totals per unit

| Unit | Test functions |
|---|---:|
| P1 generic diff engine | 8 |
| U04 diff_notebooks + ignore config | 14 |
| U05 patch + diff_utils | 5 |
| U06 diff pretty-printer | 21 |
| U07 nbdiff CLI | 14 |
| U08 nbshow | 1 |
| U09 nbpatch | 1 |
| U10 merge decisions/chunks/apply_decisions | 15 |
| U11 generic merge | 32 |
| U12 merge strategies/autoresolve/inline | 41 |
| U13 notebook merge + nbmerge CLI | 29 |
| U14 git drivers/config | 58 |
| U15 Python package API | 3 |
| OUT web/server (out of scope) | 21 |
| Not in unit list | 5 |
| **Total (all files)** | **268** |

U02 and U03 have no test functions assigned in this inventory. U08 (nbshow) has one test in test_cli_apps.py; U09 (nbpatch) has one.

