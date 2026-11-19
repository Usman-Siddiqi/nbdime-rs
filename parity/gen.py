"""Seeded generators of adversarial inputs for parity suites.

The generators aim at the places where a port typically diverges: near-equal inputs (so the
approximate predicates and LCS tie-breaking are exercised), small alphabets (many equal
elements), non-ASCII and astral characters (code point indexing), every `str.splitlines`
boundary, empty values, and Python's numeric equality across bool/int/float.
"""

import random

from harness import SEED

ALPHABETS = [
    "ab",
    "abc \n",
    "abcdefghij \n\t",
    "xyzéü中\U0001F600\n",
    "ab\r\n\x0b\x0c\x1c\x1d\x1e\x85  ",
    "".join(chr(c) for c in range(32, 127)) + "\n",
]

LINE_VOCAB = [
    "import numpy as np\n", "x = 1\n", "x = 2\n", "y = x + 1\n", "print(x)\n", "print(y)\n",
    "\n", "    return x\n", "def f(x):\n", "# comment\n", "for i in range(10):\n",
    "    pass\n", "z = 'é\U0001F600'\n", "a\r\n", "b\r", "c\x0b", " ", "long line " * 8 + "\n",
]


def rng_for(name, k=0):
    return random.Random("%s:%d:%d" % (name, SEED, k))


def random_string(rng, alphabet=None, max_len=40):
    alphabet = alphabet or rng.choice(ALPHABETS)
    return "".join(rng.choice(alphabet) for _ in range(rng.randint(0, max_len)))


def mutate_string(rng, s, alphabet=None, edits=None):
    alphabet = alphabet or rng.choice(ALPHABETS)
    chars = list(s)
    for _ in range(edits if edits is not None else rng.randint(0, 4)):
        op = rng.random()
        pos = rng.randint(0, len(chars))
        if op < 0.35:
            chars[pos:pos] = list(random_string(rng, alphabet, 4))
        elif op < 0.7 and chars:
            del chars[pos:pos + rng.randint(1, 4)]
        elif op < 0.85 and chars:
            i = rng.randrange(len(chars))
            chars[i] = rng.choice(alphabet)
        elif chars:
            i = rng.randrange(len(chars))
            j = min(len(chars), i + rng.randint(1, 6))
            chars[pos:pos] = chars[i:j]
    return "".join(chars)


def string_pair(rng):
    """A pair of strings, usually similar."""
    alphabet = rng.choice(ALPHABETS)
    a = random_string(rng, alphabet, rng.choice([0, 3, 10, 40, 120]))
    r = rng.random()
    if r < 0.1:
        b = a
    elif r < 0.2:
        b = random_string(rng, alphabet, 40)
    else:
        b = mutate_string(rng, a, alphabet)
    return a, b


def multiline_text(rng, n=None):
    n = rng.randint(0, 12) if n is None else n
    lines = []
    for _ in range(n):
        if rng.random() < 0.7:
            lines.append(rng.choice(LINE_VOCAB))
        else:
            lines.append(random_string(rng, None, 30) + rng.choice(["\n", "\n", "\r\n", "", " "]))
    return "".join(lines)


def mutate_lines(rng, text):
    lines = text.splitlines(True)
    for _ in range(rng.randint(0, 4)):
        op = rng.random()
        pos = rng.randint(0, len(lines))
        if op < 0.3:
            lines.insert(pos, rng.choice(LINE_VOCAB))
        elif op < 0.55 and lines:
            del lines[min(pos, len(lines) - 1)]
        elif op < 0.9 and lines:
            i = rng.randrange(len(lines))
            lines[i] = mutate_string(rng, lines[i], edits=rng.randint(1, 3))
        elif lines:
            i = rng.randrange(len(lines))
            lines.insert(pos, lines[i])
    return "".join(lines)


def text_pair(rng):
    a = multiline_text(rng)
    r = rng.random()
    if r < 0.08:
        return a, a
    if r < 0.15:
        return a, multiline_text(rng)
    return a, mutate_lines(rng, a)


SCALARS = [None, True, False, 0, 1, -1, 1.0, 0.0, -0.0, 1.5, 2, 2.0, 10**20, 1e20, "", "a", "1", "x\ny\n"]


def random_json(rng, depth=0, max_depth=4):
    r = rng.random()
    if depth >= max_depth or r < 0.35:
        if rng.random() < 0.3:
            return multiline_text(rng, rng.randint(0, 3))
        return rng.choice(SCALARS)
    if r < 0.65:
        return [random_json(rng, depth + 1, max_depth) for _ in range(rng.randint(0, 6))]
    keys = ["a", "b", "c", "id", "source", "outputs", "é", "Z", "data", "1"]
    return {rng.choice(keys): random_json(rng, depth + 1, max_depth) for _ in range(rng.randint(0, 5))}


def mutate_json(rng, v, depth=0):
    r = rng.random()
    if r < 0.15:
        return random_json(rng, depth)
    if isinstance(v, bool) or v is None:
        return rng.choice([v, not v if v is not None else None, 1, 0, 1.0])
    if isinstance(v, int):
        return rng.choice([v, float(v), v + 1, bool(v % 2), str(v)])
    if isinstance(v, float):
        return rng.choice([v, int(v) if v == v and abs(v) < 1e300 else v, v * 2, -v])
    if isinstance(v, str):
        return rng.choice([v, mutate_string(rng, v), mutate_lines(rng, v) if "\n" in v else v + "\n"])
    if isinstance(v, list):
        out = [mutate_json(rng, x, depth + 1) if rng.random() < 0.3 else x for x in v]
        for _ in range(rng.randint(0, 2)):
            op = rng.random()
            pos = rng.randint(0, len(out))
            if op < 0.4:
                out.insert(pos, random_json(rng, depth + 1))
            elif op < 0.8 and out:
                del out[min(pos, len(out) - 1)]
            elif out:
                out.insert(pos, out[rng.randrange(len(out))])
        return out
    if isinstance(v, dict):
        out = {}
        for k, x in v.items():
            p = rng.random()
            if p < 0.1:
                continue
            out[k] = mutate_json(rng, x, depth + 1) if p < 0.4 else x
        if rng.random() < 0.3:
            out[rng.choice(["new", "a", "b", "é"])] = random_json(rng, depth + 1)
        return out
    raise TypeError(type(v))


def json_pair(rng):
    a = random_json(rng)
    if rng.random() < 0.5:
        a = {"root": a, "list": [random_json(rng, 2) for _ in range(rng.randint(0, 5))]}
    return a, mutate_json(rng, a)
