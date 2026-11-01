#!/usr/bin/env python3
# Copyright 2026 Raph Levien
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""Regenerate the font data in font.sy from a Bits'N'Picas .kbitx font.

Usage: kbitx2font.py FONT

Replaces FONT_STRIDE, font_glyphs and font_bits, and leaves the rest of
font.sy alone. Each glyph is trimmed to the columns it has pixels in, and
keeps its advance and the position of those columns.
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


def lines(values, fmt):
    rows = (values[i : i + 8] for i in range(0, len(values), 8))
    return "".join("    " + " ".join(fmt(v) + "," for v in row) + "\n" for row in rows)


def replace_array(src, name, body):
    pattern = rf"(static {name}: .*? = \[\n).*?(\]\n)"
    return re.sub(pattern, lambda m: m[1] + body + m[2], src, flags=re.S)


def main():
    parser = argparse.ArgumentParser(description="Regenerate font.sy from a .kbitx font")
    parser.add_argument("font")
    args = parser.parse_args()

    glyphs = load_font(args.font)
    chars = [chr(FIRST + i) for i in range(COUNT)]
    trimmed = {c: trim(c, glyphs[c]) for c in chars}
    place, words = pack(chars, trimmed)
    # jit_text's ldr reaches 32 words. The advance limits how many words of
    # code a glyph or blank can need; only a first glyph can be clamped, so
    # any other can't reach left of the text's start.
    if len(words) > 32:
        sys.exit(f"{len(words)} words is too wide")
    advances = [glyphs[c][2] for c in chars]
    if max(advances) > 32 or min(advances) + min(t[0] for t in trimmed.values()) < 0:
        sys.exit("an advance doesn't fit")
    bits = rasterize(chars, glyphs, trimmed, place, len(words))
    table = [glyph_entry(trimmed[c], glyphs[c][2], place[c]) for c in chars]

    with open(FONT_SY) as f:
        src = f.read()
    src = re.sub(r"const FONT_STRIDE = \d+", f"const FONT_STRIDE = {len(words) * 4}", src)
    src = replace_array(src, "font_glyphs", lines(table, lambda v: f"0x{v:08x}"))
    src = replace_array(src, "font_bits", lines(bits, hex))
    with open(FONT_SY, "w") as f:
        f.write(src)
    print(f"stride {len(words) * 4}, {sum(words)} of {len(words) * 32} bits used")


if __name__ == "__main__":
    main()
