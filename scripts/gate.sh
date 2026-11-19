#!/usr/bin/env bash
# Merge gate for a unit branch. Protected: only the orchestrator edits this file.
#
# usage: scripts/gate.sh [parity test files or pytest args...]
#   GATE_BASE   branch the unit was cut from (default: main)
#   REF_PYTHON  python with nbdime==4.0.4 (default: /home/claude/ref/venv/bin/python)
#
# With no arguments, runs the suites listed in parity/enabled_suites.txt.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

base="${GATE_BASE:-main}"
py="${REF_PYTHON:-/home/claude/ref/venv/bin/python}"

step() { printf '\n== %s\n' "$*"; }

step "protected paths unchanged since $base"
if git rev-parse --verify -q "$base" >/dev/null; then
  changed="$(git diff --name-only "$(git merge-base "$base" HEAD)" HEAD -- parity scripts/gate.sh .github \
             ; git status --porcelain -- parity scripts/gate.sh .github)"
  if [ -n "$changed" ] && [ "${GATE_ALLOW_PROTECTED:-0}" != "1" ]; then
    echo "FAIL: protected paths modified:"; echo "$changed"; exit 1
  fi
else
  echo "warning: base $base not found; skipping protected-path check"
fi

step "rustfmt"
cargo fmt --all -- --check

step "clippy"
cargo clippy --all-targets -- -D warnings

step "unit tests"
cargo test --quiet

step "build engine"
cargo build --release --quiet -p nbdime-cli
export NBDIME_RS_DEV="$PWD/target/release/nbdime-rs-dev"

step "parity"
if [ "$#" -eq 0 ]; then
  mapfile -t suites < <(grep -v '^\s*#' parity/enabled_suites.txt | sed '/^\s*$/d' | sed 's#^#parity/#')
  set -- "${suites[@]}"
fi
log="$(mktemp)"
set +e
"$py" -m pytest -c parity/pytest.ini -rA "$@" | tee "$log"
status=${PIPESTATUS[0]}
set -e
if [ "$status" -ne 0 ]; then echo "FAIL: parity"; exit 1; fi
if grep -Eq '[0-9]+ (skipped|xfailed|xpassed)' "$log"; then echo "FAIL: skipped/xfailed parity tests"; exit 1; fi
if ! grep -Eq '[1-9][0-9]* passed' "$log"; then echo "FAIL: no parity tests ran"; exit 1; fi

step "GATE PASSED"
