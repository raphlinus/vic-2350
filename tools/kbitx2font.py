#!/usr/bin/env python3
# Copyright 2026 Raph Levien
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""Regenerate the font data in font.sy from a Bits'N'Picas .kbitx font.

Usage: kbitx2font.py FONT [--kern KERN]

Replaces FONT_STRIDE, FONT_FIRST, FONT_GLYPHS, FONT_KERN, font_glyphs,
font_kern and font_bits, and leaves the rest of font.sy alone. Each glyph is trimmed to
the columns it has pixels in, and keeps its advance and the position of
those columns. Glyphs share columns in font_bits where they match: one
whose columns all appear in another's is placed there, and one whose
first columns are another's last overlaps it.

KERN gives each character two bytes of kerning opportunities, one for each
side. A pair kerns by the number of bits set in both the right byte of the
first and the left byte of the second, at most KERNS. Each line of it is
characters, then the left byte and the right byte, written as 8 bits with
the most significant first and . or 0 for a clear bit:

    TVY     ........  ......11

A character on more than one line gets the bits of all of them, so a
line can give one class of shapes its bit. Lines starting with # are
comments. Characters it doesn't list don't kern.
The tool fails if a kerned pair's pixels touch or overlap in a row.

The codes below FIRST are blanks with advances of -1 to -KERNS, which
text.sy puts between kerned pairs: code FONT_KERN - k moves back k
pixels, where FONT_KERN is FIRST.
"""

import argparse
import base64
import os
import random
import re
import sys
import xml.etree.ElementTree as ET

FONT_SY = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "font.sy")

# rows above the baseline, which leaves 3 below
ASCENT = 12
HEIGHT = 15
FIRST = 32
COUNT = 95
KERNS = 4


def decode_b64(d):
    d = d.replace("-", "+").replace("_", "/")
    return base64.b64decode(d + "=" * (-len(d) % 4), validate=True)


def decode_runs(data):
    # The top two bits of each op give its kind and the low six its length:
    # 00 off, 01 on, 10 one pixel byte repeated, 11 literal pixel bytes.
    pixels = []
    i = 0
    while i < len(data):
        kind, count = data[i] >> 6, data[i] & 63
        i += 1
        if kind == 0:
            pixels += [False] * count
        elif kind == 1:
            pixels += [True] * count
        elif kind == 2:
            pixels += [data[i] >= 128] * count
            i += 1
        else:
            pixels += [b >= 128 for b in data[i : i + count]]
            i += count
    return pixels


def load_font(path):
    """Each glyph as (x, y, advance, rows of pixels), by character."""
    glyphs = {}
    for g in ET.parse(path).getroot().iter("g"):
        if g.get("u") is None:
            continue
        d = g.get("d", "")
        data = decode_b64(d) if d else b"\0\0"
        height, width = data[0], data[1]
        pixels = decode_runs(data[2:])
        if len(pixels) != height * width:
            sys.exit(f"glyph {g.get('u')}: {len(pixels)} pixels, expected {width}x{height}")
        rows = [pixels[i * width : (i + 1) * width] for i in range(height)]
        glyphs[chr(int(g.get("u")))] = (int(g.get("x", 0)), int(g.get("y", 0)), int(g.get("w")), rows)
    return glyphs


def trim(c, glyph):
    """A glyph's left edge, relative to its origin, and width in pixels."""
    x, y, advance, rows = glyph
    columns = [i for row in rows for i, on in enumerate(row) if on]
    if not columns:
        return 0, 0
    left, width = x + min(columns), max(columns) - min(columns) + 1
    if not -8 <= left < 8 or width > 32 or left + width - advance >= 16:
        sys.exit(f"glyph {c!r}: left {left} and width {width} don't fit")
    return left, width


def columns(glyph, trim):
    """A trimmed glyph's columns, each as a mask of its rows."""
    x, y, _, rows = glyph
    left, width = trim
    cols = [0] * width
    for row_y, row in enumerate(rows):
        for row_x, on in enumerate(row):
            if on:
                cols[x + row_x - left] |= 1 << (ASCENT - y + row_y)
    return tuple(cols)


def overlap(a, b):
    """How many of a's last columns are b's first."""
    return max((k for k in range(1, min(len(a), len(b))) if a[-k:] == b[:k]), default=0)


def find(a, b):
    """Where a's columns appear in b's, or None."""
    return next((i for i in range(len(b) - len(a) + 1) if b[i : i + len(a)] == a), None)


def chain(cols, rng):
    """Join glyphs into runs of at most 32 columns, each glyph sharing
    columns with the one before where they match, longest overlap first and
    ties broken by rng. Each run is (columns, [(char, offset)])."""
    runs = {c: (cols[c], [(c, 0)]) for c in cols}
    ov = {(a, b): overlap(cols[a], cols[b]) for a in cols for b in cols if a != b}
    while True:
        fits = [(k, rng.random(), a, b) for (a, b), k in ov.items() if k and len(runs[a][0]) + len(runs[b][0]) - k <= 32]
        if not fits:
            return list(runs.values())
        k, _, a, b = max(fits)
        (ca, pa), (cb, pb) = runs.pop(a), runs.pop(b)
        runs[a] = (ca + cb[k:], pa + [(c, o + len(ca) - k) for c, o in pb])
        ov = {p: v for p, v in ov.items() if b not in p and a not in p}
        for o in runs:
            if o != a:
                ov[a, o] = overlap(runs[a][0], runs[o][0])
                ov[o, a] = overlap(runs[o][0], runs[a][0])


def fill(widths, n, limit=20000):
    """Which of n words each width goes in, without overfilling any, by a
    search that gives up after limit steps; None if it found none."""
    used = [0] * n
    where = []
    steps = 0

    def go(i):
        nonlocal steps
        if i == len(widths):
            return True
        steps += 1
        if steps > limit:
            return False
        tried = set()
        for w in range(n):
            if used[w] + widths[i] <= 32 and used[w] not in tried:
                tried.add(used[w])
                used[w] += widths[i]
                where.append(w)
                if go(i + 1):
                    return True
                where.pop()
                used[w] -= widths[i]
        return False

    return where if go(0) else None


def pack(chars, glyphs, trimmed):
    # The generated code extracts a glyph from a single word, so none can
    # cross a word boundary, but glyphs can share columns: a glyph whose
    # columns are all in another's goes there, and the rest are chained
    # where one's last columns are the next one's first. The chains are
    # packed into as few words as a bounded search finds, and as the
    # result depends on how ties were broken, that's tried a few times.
    cols = {c: columns(glyphs[c], trimmed[c]) for c in chars if trimmed[c][1]}
    inside = {}
    for c in cols:
        for h in cols:
            if h != c and h not in inside and len(cols[c]) < len(cols[h]) and find(cols[c], cols[h]) is not None:
                inside[c] = h
                break
    rng = random.Random(0)
    best = None
    for _ in range(100):
        runs = sorted(chain({c: cols[c] for c in cols if c not in inside}, rng), key=lambda r: -len(r[0]))
        widths = [len(r[0]) for r in runs]
        n = -(-sum(widths) // 32)
        if best and n >= best[1]:
            continue
        where = fill(widths, n)
        if where is not None:
            best = (runs, n, where)
    if best is None:
        sys.exit("no packing found")
    runs, n, where = best
    words = [0] * n
    place = {c: 0 for c in chars}
    for (run, at), w in zip(runs, where):
        for c, o in at:
            place[c] = w * 32 + words[w] + o
        words[w] += len(run)
    for c, h in inside.items():
        place[c] = place[h] + find(cols[c], cols[h])
    return place, words


def rasterize(chars, glyphs, trimmed, place, stride_words):
    bits = [0] * (stride_words * HEIGHT)
    for c in chars:
        x, y, _, rows = glyphs[c]
        left = trimmed[c][0]
        for row_y, row in enumerate(rows):
            for row_x, on in enumerate(row):
                if not on:
                    continue
                px, py = x + row_x - left, ASCENT - y + row_y
                if not 0 <= py < HEIGHT:
                    sys.exit(f"glyph {c!r}: row {py} is outside the font's {HEIGHT}")
                bit = place[c] + px
                bits[py * stride_words + bit // 32] |= 1 << (bit % 32)
    return bits


def glyph_entry(trim, advance, offset):
    left, width = trim
    return offset | (left & 15) << 16 | width << 20 | advance << 26


def load_kern(path):
    """Each character's kerning opportunities, as (left, right) bytes."""
    kern = {}
    with open(path) as f:
        for n, line in enumerate(f, 1):
            fields = line.split()
            if not fields or line.startswith("#"):
                continue
            if len(fields) != 3 or not all(re.fullmatch(r"[.01]{8}", b) for b in fields[1:]):
                sys.exit(f"{path}:{n}: expected characters, then two bytes of 8 bits")
            left, right = (int(b.replace(".", "0"), 2) for b in fields[1:])
            for c in fields[0]:
                old_left, old_right = kern.get(c, (0, 0))
                kern[c] = (old_left | left, old_right | right)
    return kern


def kern_pair(kern, a, b):
    """How many pixels the pair a, b moves together."""
    return bin(kern.get(a, (0, 0))[1] & kern.get(b, (0, 0))[0]).count("1")


def ink(glyph, dx):
    """The columns of a glyph's pixels in each row of the font, its origin at dx."""
    x, y, _, rows = glyph
    columns = {}
    for row_y, row in enumerate(rows):
        for row_x, on in enumerate(row):
            if on:
                columns.setdefault(ASCENT - y + row_y, []).append(dx + x + row_x)
    return columns


def gap(glyphs, a, b, k):
    """The fewest blank columns between a's pixels and b's in any row, with b
    k pixels closer than its advance; None if no row has both."""
    left = ink(glyphs[a], 0)
    right = ink(glyphs[b], glyphs[a][2] - k)
    gaps = [min(right[y]) - max(left[y]) - 1 for y in left if y in right]
    return min(gaps, default=None)


def kern_problems(chars, glyphs, kern):
    """What's wrong with each kerned pair that can't be used."""
    problems = []
    for a in chars:
        for b in chars:
            k = kern_pair(kern, a, b)
            if k == 0:
                continue
            g = gap(glyphs, a, b, k)
            if k > KERNS:
                problems.append(f"{a + b!r} kerns {k}, more than {KERNS}")
            elif glyphs[a][2] < k:
                problems.append(f"{a + b!r} kerns {k}, more than {a!r}'s advance")
            elif g is not None and g < 1:
                problems.append(f"{a + b!r} kerns {k}, and its pixels touch")
    return problems


def lines(values, fmt):
    rows = (values[i : i + 8] for i in range(0, len(values), 8))
    return "".join("    " + " ".join(fmt(v) + "," for v in row) + "\n" for row in rows)


def replace_array(src, name, body):
    pattern = rf"(static {name}: .*? = \[\n).*?(\]\n)"
    return re.sub(pattern, lambda m: m[1] + body + m[2], src, flags=re.S)


def main():
    parser = argparse.ArgumentParser(description="Regenerate font.sy from a .kbitx font")
    parser.add_argument("font")
    parser.add_argument("--kern", help="the kerning opportunities")
    args = parser.parse_args()

    glyphs = load_font(args.font)
    chars = [chr(FIRST + i) for i in range(COUNT)]
    kern = load_kern(args.kern) if args.kern else {}
    problems = kern_problems(chars, glyphs, kern)
    if problems:
        sys.exit("\n".join(problems))
    trimmed = {c: trim(c, glyphs[c]) for c in chars}
    place, words = pack(chars, glyphs, trimmed)
    # jit_text's ldr reaches 32 words. The advance limits how many words of
    # code a glyph or blank can need; only a first glyph can be clamped, so
    # any other can't reach left of the text's start, kerned or not.
    if len(words) > 32:
        sys.exit(f"{len(words)} words is too wide")
    if max(glyphs[c][2] for c in chars) > 32:
        sys.exit("an advance doesn't fit")
    for a in chars:
        for b in chars:
            if glyphs[a][2] - kern_pair(kern, a, b) + trimmed[b][0] < 0:
                sys.exit(f"{a + b!r} reaches left of {a!r}")
    bits = rasterize(chars, glyphs, trimmed, place, len(words))
    # the kern codes, then the characters
    table = [glyph_entry((0, 0), -k & 63, 0) for k in range(KERNS, 0, -1)]
    table += [glyph_entry(trimmed[c], glyphs[c][2], place[c]) for c in chars]
    kern_table = [0] * KERNS + [kern.get(c, (0, 0))[0] | kern.get(c, (0, 0))[1] << 8 for c in chars]

    with open(FONT_SY) as f:
        src = f.read()
    src = re.sub(r"const FONT_STRIDE = \d+", f"const FONT_STRIDE = {len(words) * 4}", src)
    src = re.sub(r"const FONT_FIRST = \d+", f"const FONT_FIRST = {FIRST - KERNS}", src)
    src = re.sub(r"const FONT_GLYPHS = \d+", f"const FONT_GLYPHS = {KERNS + COUNT}", src)
    src = re.sub(r"const FONT_KERN = \d+", f"const FONT_KERN = {FIRST}", src)
    src = replace_array(src, "font_glyphs", lines(table, lambda v: f"0x{v:08x}"))
    src = replace_array(src, "font_kern", lines(kern_table, lambda v: f"0x{v:04x}"))
    src = replace_array(src, "font_bits", lines(bits, hex))
    with open(FONT_SY, "w") as f:
        f.write(src)
    print(f"stride {len(words) * 4}, {sum(words)} of {len(words) * 32} bits used")


if __name__ == "__main__":
    main()
