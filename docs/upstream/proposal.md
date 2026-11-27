<!--
Notes for Usman, not part of the issue text. Delete this block before posting.

- Post this as an issue on jupyter/nbdime only after nbdime-rs is on PyPI with Linux, macOS and
  Windows wheels, and after a quick search of nbdime's issues for earlier speed or Rust threads
  (link any you find).
- Read it through and change anything that doesn't sound like you. Maintainers will ask questions
  about the code; answer them yourself, in your own words.
- Rerun the timings on a quiet machine and add an end-to-end `nbdiff` time before posting.
-->

# Optional faster engine for notebook diffs and merges

Hi! Diffing large notebooks with nbdime gets slow, because cell matching grows faster than
the number of cells. On a 1600-cell notebook with about 5% of cells edited, `diff_notebooks`
takes around 580 ms. I've built a Rust version of nbdime's diff and merge logic, called
[nbdime-rs](https://github.com/Usman-Siddiqi/nbdime-rs), that gives the same results in about
10 ms. It's published on PyPI as `nbdime-rs`.

Before sending any code your way, I'd like to ask whether you'd be interested in nbdime using
it as an optional extra.

**Up front: most of nbdime-rs was written by AI coding agents, working under my direction.**
Because of that, I put most of the effort into testing it rather than trusting it:

- **nbdime's own test suite.** With a small patch that lets nbdime call the engine, nbdime
  4.0.4's whole non-web test suite (over 6,000 tests) passes with the engine forced on, and
  with it off.
- **Side-by-side parity tests.** 129 tests run nbdime 4.0.4 and nbdime-rs on the same inputs and
  require identical output. They cover nbdime's test notebooks, every predicate and ignore
  option, and about 10,000 random and deliberately awkward notebook pairs.
- **Larger randomized runs.** The parity suites also ran at four times their normal size with
  three different random seeds, and found no differences from nbdime.
- **Known differences are written down.** The few places where nbdime-rs behaves differently on
  purpose (for example, where nbdime raises an error on a valid notebook) are listed in the
  repo, and the engine would use Python for those unless we agree otherwise.

nbdime-rs is BSD-3-Clause like nbdime, and no code was copied from GPL or LGPL projects.

## What it could look like

I see two options and I'm happy with either:

1. **A small hook in nbdime** (for example a plugin entry point) that lets an installed package
   provide the notebook diff and merge-decision functions. All the adapter code would live in
   nbdime-rs, so you would only own a few lines.
2. **A built-in optional engine.** About 300 lines in a new `nbdime/_accel.py`, a few lines in
   the notebook diff and merge modules, tests, and an `accel` extra. I have this working
   against 4.0.4.

Either way:

- **It's off unless you turn it on.** Without `NBDIME_ACCEL=1`, nbdime doesn't even import the
  extension. Turning it on by default could be a later conversation, once people have used it.
- **Plain nbdime doesn't change.** nbdime stays pure Python, and building it doesn't need Rust.
  The extension ships as prebuilt wheels.
- **It falls back safely.** Anything the engine can't represent, such as custom predicates or
  differs, uses the normal Python code. `NBDIME_ACCEL=force` turns the fallback into an error,
  so CI can prove the engine actually ran.
- **I'll maintain it.** The engine only switches on for the exact nbdime version it was tested
  against, so a new nbdime release can't break anyone. It just uses Python until I release a
  matching nbdime-rs, and I'll handle those releases and their CI.

Would either of these be welcome? If not, no worries: nbdime-rs will stay available as a
separate package.
