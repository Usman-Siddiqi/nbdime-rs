"""U16: git's text diff and three-way merge output, as nbdime renders them with git installed.

Upstream's pretty-printer runs `git diff --no-index [--color|--color-words] before after` for
every modified multi-line string, and the inline-source merge strategy runs
`git merge-file -p local base remote [--ours|--theirs|--union]`. nbdime-rs reproduces git's
output itself (git 2.43, default configuration; findings F-022) instead of spawning git.

Engine functions (the oracles are upstream's own functions, which spawn git):
- `git.diff_render {a, b, use_color: bool, color_words: bool}` -> str, as
  `prettyprint.diff_render_with_git(a, b, PrettyPrintConfig(use_color=..., color_words=...))`.
- `git.merge_render {base, local, remote, strategy: null|"use-local"|"use-remote"|"union"}` ->
  `[merged, status]`, as `prettyprint.merge_render_with_git`.
"""

import logging

from nbdime import prettyprint as pp

from gen import LINE_VOCAB, mutate_lines, multiline_text, random_string, rng_for
from harness import check_parity, scaled, upstream_test_notebooks

logging.getLogger("nbdime").setLevel(logging.CRITICAL)


def o_diff_render(a, b, use_color=True, color_words=False):
    return pp.diff_render_with_git(a, b, pp.PrettyPrintConfig(use_color=use_color, color_words=color_words))


def o_merge_render(base, local, remote, strategy=None):
    merged, status = pp.merge_render_with_git(base, local, remote, strategy)
    return [merged, status]


CODE = [
    "def f(x):\n", "    return x\n", "class A:\n", "    def g(self):\n", "        pass\n",
    "if x:\n", "    y = 1\n", "else:\n", "    y = 2\n", "\n", "# note\n", "x = 1  \n",
    "\tindented with tab\n", "  \t mixed\n", "trailing space \n", "$dollar\n", "_under\n",
    "for i in range(3):\n", "    print(i)\n", "import os\n", "}\n", "{\n", "@decorator\n",
]


def code_text(rng, n=None):
    n = rng.randint(0, 40) if n is None else n
    lines = [rng.choice(CODE) if rng.random() < 0.8 else rng.choice(LINE_VOCAB) for _ in range(n)]
    s = "".join(lines)
    if s and rng.random() < 0.3:
        s = s.rstrip("\n")
    return s


def mutate_code(rng, s):
    lines = s.splitlines(True)
    for _ in range(rng.randint(1, 6)):
        r = rng.random()
        pos = rng.randint(0, len(lines))
        if r < 0.35:
            lines.insert(pos, rng.choice(CODE))
        elif r < 0.6 and lines:
            del lines[min(pos, len(lines) - 1)]
        elif r < 0.85 and lines:
            i = rng.randrange(len(lines))
            words = lines[i].split(" ")
            words[rng.randrange(len(words))] = random_string(rng, "abcxyz_()=+ ", 6)
            lines[i] = " ".join(words)
        elif lines:
            i = rng.randrange(len(lines))
            lines.insert(pos, lines[i])
    out = "".join(lines)
    if rng.random() < 0.1:
        out = out.rstrip("\n") if out.endswith("\n") else out + "\n"
    return out


def text_pairs(name, n):
    rng = rng_for(name)
    pairs = []
    while len(pairs) < n:
        r = rng.random()
        if r < 0.5:
            a = code_text(rng)
            b = mutate_code(rng, a)
        elif r < 0.8:
            a = multiline_text(rng, rng.randint(0, 20))
            b = mutate_lines(rng, a)
        else:
            a = code_text(rng, rng.randint(50, 200))
            b = mutate_code(rng, mutate_code(rng, a))
        if a == b or ("\n" not in a and "\n" not in b):
            continue
        pairs.append((a, b))
    return pairs


def notebook_text_pairs():
    texts = []
    for _, raw in upstream_test_notebooks():
        for cell in raw.get("cells", []):
            src = cell.get("source", "")
            texts.append("".join(src) if isinstance(src, list) else src)
    texts = [t for t in dict.fromkeys(texts) if "\n" in t]
    return [(a, b) for a in texts[:25] for b in texts[:25] if a != b]


def diff_cases(name, n):
    rng = rng_for(name + "-modes")
    cases = []
    for a, b in text_pairs(name, n):
        r = rng.random()
        mode = {"use_color": True, "color_words": False} if r < 0.5 else (
            {"use_color": False, "color_words": False} if r < 0.75 else {"use_color": True, "color_words": True})
        cases.append(dict(mode, a=a, b=b))
    return cases


def test_u16_diff_render_color(engine):
    cases = [dict(c, use_color=True, color_words=False) for c in diff_cases("u16-color", scaled(2500))]
    check_parity(engine, "git.diff_render", cases, o_diff_render, min_cases=scaled(2500))


def test_u16_diff_render_plain(engine):
    cases = [dict(c, use_color=False, color_words=False) for c in diff_cases("u16-plain", scaled(1500))]
    check_parity(engine, "git.diff_render", cases, o_diff_render, min_cases=scaled(1500))


def test_u16_diff_render_color_words(engine):
    cases = [dict(c, use_color=True, color_words=True) for c in diff_cases("u16-words", scaled(1500))]
    check_parity(engine, "git.diff_render", cases, o_diff_render, min_cases=scaled(1500))


def test_u16_diff_render_notebook_sources(engine):
    cases = []
    for a, b in notebook_text_pairs():
        for mode in ({"use_color": True, "color_words": False}, {"use_color": False, "color_words": False},
                     {"use_color": True, "color_words": True}):
            cases.append(dict(mode, a=a, b=b))
    check_parity(engine, "git.diff_render", cases, o_diff_render, min_cases=300)


def test_u16_diff_render_edges(engine):
    edges = ["", "\n", "x", "x\n", "\n\n\n", "a\nb\nc\n", "a\r\nb\r\n", "tab\t\n", "trail  \n",
             " \t\n", "\u00e9\U0001F600\n", "no newline", "a\x00b\n", "\x0b\x85\u2028\n", "\ufeffbom\n"]
    cases = []
    for a in edges:
        for b in edges:
            if a != b and ("\n" in a or "\n" in b):
                for mode in ({"use_color": True, "color_words": False}, {"use_color": False, "color_words": False},
                             {"use_color": True, "color_words": True}):
                    cases.append(dict(mode, a=a, b=b))
    check_parity(engine, "git.diff_render", cases, o_diff_render, min_cases=400)


def merge_cases(name, n):
    rng = rng_for(name)
    cases = []
    while len(cases) < n:
        base = code_text(rng) if rng.random() < 0.6 else multiline_text(rng, rng.randint(0, 15))
        local = mutate_code(rng, base) if rng.random() < 0.9 else base
        remote = mutate_code(rng, base) if rng.random() < 0.9 else local
        if rng.random() < 0.1:
            base = ""
        strategy = rng.choice([None, None, None, "use-local", "use-remote", "union"])
        cases.append({"base": base, "local": local, "remote": remote, "strategy": strategy})
    return cases


def test_u16_merge_render(engine):
    cases = merge_cases("u16-merge", scaled(3000))
    check_parity(engine, "git.merge_render", cases, o_merge_render, min_cases=scaled(3000))


def test_u16_merge_render_edges(engine):
    edges = ["", "\n", "x", "x\n", "a\nb\nc\n", "a\nB\nc\n", "a\nb\nC\n", "A\nb\nc", "a\r\nb\r\n", "a\r\nB\r\n"]
    cases = [{"base": b, "local": l, "remote": r, "strategy": s}
             for b in edges for l in edges for r in edges for s in (None, "union")]
    check_parity(engine, "git.merge_render", cases, o_merge_render, min_cases=1500)
