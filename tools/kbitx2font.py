#!/usr/bin/env python3
# Copyright 2026 Raph Levien
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""Regenerate the font data in font.sy from a Bits'N'Picas .kbitx font.

Usage: kbitx2font.py FONT [--kern KERN]

Replaces FONT_STRIDE, FONT_FIRST, FONT_GLYPHS, FONT_KERN, font_glyphs,
font_kern and font_bits, and leaves the rest of font.sy alone. Each glyph is trimmed to
the columns it has pixels in, and keeps its advance and the position of
those columns.

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


def pack(chars, trimmed):
    # First fit, widest first. The generated code extracts a glyph from a
    # single word, so none can cross a word boundary.
    words = []
    place = {}
    for c in sorted(chars, key=lambda c: -trimmed[c][1]):
        width = trimmed[c][1]
        if width == 0:
            place[c] = 0
            continue
        for i, used in enumerate(words):
            if used + width <= 32:
                break
        else:
            words.append(0)
            i = len(words) - 1
        place[c] = i * 32 + words[i]
        words[i] += width
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
    place, words = pack(chars, trimmed)
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
