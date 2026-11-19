# Agent prompt templates

Every prompt that can touch git or the network starts with the two hard rules from `CLAUDE.md`,
copied word for word. Unit specs live next to this file (`<unit>.md`).

## Implementer (Sonnet, effort high)

- Role: port one unit, make the gate pass, commit, report. Someone else reviews and fixes.
- Works only in `/home/claude/wt/<unit>` on branch `unit/<unit>`; never pushes.
- Reads: `CLAUDE.md`, the unit spec, the named porting-guide sections, the unit's parity tests,
  the upstream sources (not from memory).
- Protected paths and the no-weakening rule, stated as "any such change is a failed task".
- `CARGO_BUILD_JOBS=2` on every cargo command.
- Done = `scripts/gate.sh <suites>` prints `GATE PASSED`, clean conventional commits, a report
  that includes spec problems found, suspected upstream bugs, iteration count and the traps that
  bit.

## Hostile reviewer A: correctness (Opus, effort high)

- Mandate: find bugs, semantic differences from upstream, and reasons the code is wrong. Do not
  fix anything and do not edit the worktree. Not collaborative.
- Must try to break the code with concrete inputs: write scratch drivers that send cases to
  `target/release/nbdime-rs-dev batch` and to the oracle, outside the parity suite (the suite
  already passes; the point is what it does not cover).
- Also checks: code quality against `CLAUDE.md`, performance pitfalls (quadratic memory,
  recursion on input size, needless clones), error handling, and whether tests are meaningful.
- Output: numbered findings, each with severity (blocker / major / minor / nit), file:line,
  and a concrete failing input with expected vs actual output when it is a behavior bug.
  Finish with "SIGN-OFF: yes/no".

## Hostile reviewer B: looks the same, behaves differently (Opus, effort high)

- Same rules as A, but the hunt is specifically for code that reads like the Python yet behaves
  differently: numeric equality and types, code points vs bytes, Unicode case and line
  boundaries, sort order and stability, float arithmetic order, dict ordering, truthiness,
  slicing semantics, off-by-one in ranges, default arguments, exceptions that become panics or
  silent successes.
- Output format as A.

## Fixer (Sonnet, effort high)

- Applies the reviewers' findings in the unit's worktree. Does not re-litigate findings it
  disagrees with silently: either fixes or explains with evidence.
- Adds a regression unit test for every behavior bug fixed, and a parity-style reproduction in
  its report so the orchestrator can promote it into `parity/`.
- Done = gate passes again, commits, report mapping each finding to fixed / rejected (why).
