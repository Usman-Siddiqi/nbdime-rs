#!/usr/bin/env bash
# Regenerates parity/data/upstream_merge_calls.jsonl: every top-level call of nbdime's merge entry
# points made by upstream's own merge tests (nbdime 4.0.4), with arguments and results.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
py="${REF_PYTHON:-/home/claude/ref/venv/bin/python}"
tests="$("$py" -c 'import nbdime.tests, os; print(os.path.dirname(nbdime.tests.__file__))')"
out="$here/../data/upstream_merge_calls.jsonl"
cd /tmp
PYTHONDONTWRITEBYTECODE=1 PYTHONPATH="$here" REC_OUT="$out" "$py" -m pytest -q -p no:cacheprovider -p merge_recorder \
  "$tests"/test_apply_merge.py "$tests"/test_autoresolve.py "$tests"/test_merge.py \
  "$tests"/test_merge_format.py "$tests"/test_merge_notebooks.py "$tests"/test_merge_notebooks_inline.py
