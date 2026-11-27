<!--
Notes for Usman, not part of the issue. Delete this block before posting.
- Post this on github.com/jupyter/nbdime/issues only after nbdime-rs is on PyPI.
- Search their issues for "slow" / "performance" first and link anything relevant.
- Change any wording that doesn't sound like you. If they ask about the code, answer yourself.
-->

# Would you want an optional faster engine for notebook diffs?

Hey! I've been working on a Rust rewrite of nbdime's diff and merge logic, called
[nbdime-rs](https://github.com/Usman-Siddiqi/nbdime-rs), and wanted to ask if you'd be open to
nbdime using it as an optional extra before I send any code.

The reason I started: nbdime gets really slow on big notebooks, especially older ones without
cell ids. Here's what I measured, nbdime 4.0.4 on its own vs. with the Rust engine turned on
(same output both ways):

| Notebook | Diff: Python | Diff: engine | Merge: Python | Merge: engine |
|---|---:|---:|---:|---:|
| 100 cells | 6.5 ms | 1.4 ms | 16 ms | 5.2 ms |
| 400 cells | 44 ms | 5.8 ms | 104 ms | 20 ms |
| 1,600 cells | 661 ms | 29 ms | 1.21 s | 107 ms |
| 400 cells, no ids | 1.82 s | 9.6 ms | 3.49 s | 28 ms |
| 1,600 cells, no ids | 30.4 s | 114 ms | 60.8 s | 241 ms |

And the full `nbdiff` command on the 1,600-cell notebook without ids goes from **32 s to
1.2 s** (most of what's left is Python startup and reading the files).

These are medians of 3 runs on a 4-core Intel Xeon cloud VM with CPython 3.13, using
generated notebooks with about 5% of cells changed. To rerun it, apply the patch to a 4.0.4
checkout, `pip install nbdime-rs`, and run
[`bench/upstream_engine.py`](https://github.com/Usman-Siddiqi/nbdime-rs/blob/main/bench/upstream_engine.py)
from the checkout. Raw numbers are in
[`bench/results/2026-11-27-upstream-engine.json`](https://github.com/Usman-Siddiqi/nbdime-rs/blob/main/bench/results/2026-11-27-upstream-engine.json).

To be upfront: I used AI coding agents to write most of the code. I planned it and directed it,
but I didn't hand-write the Rust. Because of that I went pretty overboard on testing, since
"trust me" obviously isn't good enough:

- nbdime's own test suite passes with the engine forced on (and with it off).
- 129 tests run nbdime 4.0.4 and nbdime-rs on the same inputs and check the output is identical.
  That includes your test notebooks, every ignore/predicate option, and about 10,000 random and
  intentionally weird notebook pairs.
- I also ran all of that at 4x the size with three different random seeds. No differences.
- The few places where it behaves differently on purpose are written down in the repo (mostly
  cases where nbdime throws an error on a valid notebook). Those would just use the normal
  Python code.

It's BSD-3 like nbdime, and nothing was copied from GPL/LGPL code.

There are two ways I could see this working, and I'm fine with either:

1. **A tiny hook in nbdime** (like a plugin entry point) so an installed package can provide the
   notebook diff/merge functions. All the glue code lives in nbdime-rs, and you'd only have a
   few lines to look after.
2. **A built-in optional engine.** About 300 lines in a new `nbdime/_accel.py`, plus tests and an
   `accel` extra. I have this working against 4.0.4 already.

Either way it'd be off unless someone sets `NBDIME_ACCEL=1`, so nothing changes for anyone who
doesn't opt in. nbdime stays pure Python and doesn't need Rust to build. Anything the engine
can't handle (custom predicates, custom differs, etc.) falls back to the Python code. It also
only turns on for the exact nbdime version it was tested with, so a new nbdime release can't
break anyone. I'd keep nbdime-rs updated for each release.

Would either of these be something you'd want? If not, totally fine. nbdime-rs will keep
existing as its own package either way.
