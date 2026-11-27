<!--
Notes for Usman. Delete before posting.
- Post on discourse.jupyter.org in the "Show and Tell" category, only once `pip install nbdime-rs`
  actually works and gives people the CLI.
- If you've already opened the nbdime issue, link it at the end so people can weigh in there.
- The README still says the git diff/merge drivers "are being ported", but the binaries exist.
  Confirm they are finished (and update that README line) before claiming git driver support.
-->

# nbdime-rs: a much faster nbdiff/nbmerge for big notebooks

Hey everyone! I made [nbdime-rs](https://github.com/Usman-Siddiqi/nbdime-rs), a Rust version of
nbdime's command line tools (`nbdiff`, `nbmerge`, and the git diff/merge drivers). It gives the
same output as nbdime 4.0.4, just a lot faster.

nbdime gets painfully slow on big notebooks, especially older ones without cell ids. Some
numbers from my benchmarks (full `nbdiff` command, ~5% of cells changed):

| Notebook | nbdime 4.0.4 | nbdime-rs |
|---|---:|---:|
| 100 cells | 1.5 s | 0.008 s |
| 1,600 cells | 2.3 s | 0.16 s |
| 1,600 cells, no cell ids | 39 s | 0.43 s |
| 3,000 cells, no cell ids | over 2 min | 1.4 s |

Install it with:

```
pip install nbdime-rs
```

Fair warning: I used AI coding agents to write most of the code. Because of that I tested it
really hard. It runs against nbdime itself on about 10,000 random and weird notebook pairs and
checks the output matches exactly, plus nbdime's own test suite. The few places where it
behaves differently on purpose are listed in the README.

It doesn't have nbdime's web diff viewer, only the terminal and git stuff.

I'd love it if people with big or messy notebooks tried it and told me if anything comes out
different from regular nbdime. Issues welcome on GitHub!
