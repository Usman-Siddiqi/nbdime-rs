<!--
Notes for Usman. Paste this near the top of README.md once nbdime-rs is on PyPI, replacing the
current "Why" section. The numbers come from the Performance table already in the README.
-->

## Why use this?

If `nbdiff` or `git diff` on notebooks feels slow, this is for you. nbdime-rs gives the same
output as nbdime 4.0.4, but much faster, and the bigger the notebook, the bigger the gap:

- **100 cells:** 1.5 s with nbdime, 0.008 s with nbdime-rs
- **1,600 cells:** 2.3 s vs 0.16 s
- **1,600 cells without cell ids** (older notebooks): 39 s vs 0.43 s
- **3,000 cells without cell ids:** over 2 minutes vs 1.4 s

That's the full `nbdiff` command, start to finish, with about 5% of cells changed.

```
pip install nbdime-rs
```

It works as a drop-in replacement for nbdime's terminal and git tools: `nbdiff`, `nbmerge`,
and the git diff and merge drivers. The web viewer isn't included.

**About how it was built:** most of the code was written with AI coding agents, under my
direction. That's why it's tested so heavily. Every change runs against real nbdime on about
10,000 random and adversarial notebook pairs and has to match exactly. Known differences are
listed [below](#what-works).
