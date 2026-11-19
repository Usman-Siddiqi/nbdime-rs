# Progress log

## Session 1 (2026-11-19)

### Done
- Phase 0: read upstream nbdime 4.0.4 (diffing, merging, patching, prettyprint, CLI, git
  integration, tests). Upstream test suite runs green on the reference venv: 6,351 passed,
  7 skipped, 4 xfailed in 38 s (web/server tests excluded).
- Reproduced the scaling problem and profiled it: `docs/upstream-performance.md`.
- Wrote `CLAUDE.md`, `docs/porting-guide.md`, `docs/design.md`, `docs/findings.md`, `ROADMAP.md`.

### In progress
- Phase 1 foundation (see ROADMAP).

### Environment
- Reference venv: `/home/claude/ref/venv` (nbdime 4.0.4, nbformat 5.11.1, Python 3.13.18).
- Upstream clone (read-only, tag v4.0.4): `/home/claude/ref/nbdime-upstream`.
- VM: 4 vCPU, 15 GB RAM in this session (the brief assumed 2 cores / 8 GB; keep the 3-build cap).
- Development branch: `claude/project-thread-0e989x` (pushed to origin).
