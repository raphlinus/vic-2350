#!/usr/bin/env python3
# Copyright 2026 Raph Levien
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""Preview a .kbitx font's kerning from a .kern file, as text.sy sets it.

Usage: kernview.py FONT KERN [-c] [--pairs] [--png OUT [--margin N]] [TEXT...]

Draws each TEXT (or a few test lines) in the terminal with half block
characters, a pixel per column and two rows per line, with the kern
amounts marked under the gaps they close. -c draws each line unkerned
above it. --pairs lists every kerned pair with the blank columns left
between it, and the problems kbitx2font.py would refuse. --png writes the
lines to an image as well, each pixel SCALE square, with N blank pixels
around them.
"""

import argparse
import os
import struct
import sys
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import kbitx2font as kf  # noqa: E402

SAMPLES = [
    "To Ta Ty Va Vo Wa Yo Yu AV AT Av LT LV L' T. V, F. P. r. y.",
    "WAVE TAVERN LYNX 'Toast' Yacht Lawyer Avoid",
    "The quick brown fox jumps over the lazy dog.",
    "Typography, very young; Voyage. Pay Fly Tea.",
]


def setting(glyphs, kern, text, kerned):
    """The origin of each character, the kerns between them, and the width."""
    first = text[0]
    pen = max(0, -kf.trim(first, glyphs[first])[0])
    origins, kerns = [], []
    for i, c in enumerate(text):
        k = kf.kern_pair(kern, text[i - 1], c) if i > 0 and kerned else 0
        pen -= k
        kerns.append(k)
        origins.append(pen)
        pen += glyphs[c][2]
    return origins, kerns, pen


def raster(glyphs, kern, text, kerned):
    origins, kerns, width = setting(glyphs, kern, text, kerned)
    width += 16
    rows = [[False] * width for _ in range(kf.HEIGHT)]
    for c, dx in zip(text, origins):
        for y, xs in kf.ink(glyphs[c], dx).items():
            for x in xs:
                rows[y][x] = True
    marks = [" "] * width
    for k, dx in zip(kerns, origins):
        if k:
            marks[dx] = str(k)
    return rows, "".join(marks).rstrip()


def terminal(rows):
    out = []
    for y in range(0, len(rows), 2):
        top, bottom = rows[y], rows[y + 1] if y + 1 < len(rows) else [False] * len(rows[y])
        line = "".join(" ▄▀█"[t * 2 + b] for t, b in zip(top, bottom))
        out.append(line.rstrip())
    return "\n".join(out)


def write_png(path, images, scale, margin):
    width = (max(len(r[0]) for r in images) + 2 * margin) * scale
    blank = bytes([255]) * width
    pixels = [blank] * (margin * scale)
    for i, rows in enumerate(images):
        if i:
            pixels += [blank] * (3 * scale)
        for row in rows:
            line = bytes([255]) * (margin * scale)
            line += bytes(0 if on else 255 for on in row for _ in range(scale))
            line += bytes([255]) * (width - len(line))
            pixels += [line] * scale
    pixels += [blank] * (margin * scale)
    raw = b"".join(b"\0" + line for line in pixels)

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", width, len(pixels), 8, 0, 0, 0, 0)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw)))
        f.write(chunk(b"IEND", b""))


def main():
    parser = argparse.ArgumentParser(description="Preview a font's kerning")
    parser.add_argument("font")
    parser.add_argument("kern")
    parser.add_argument("text", nargs="*")
    parser.add_argument("-c", action="store_true", help="draw each line unkerned too")
    parser.add_argument("--pairs", action="store_true", help="list the kerned pairs")
    parser.add_argument("--png", help="write the lines to this image")
    parser.add_argument("--scale", type=int, default=3)
    parser.add_argument("--margin", type=int, default=0)
    args = parser.parse_args()

    glyphs = kf.load_font(args.font)
    kern = kf.load_kern(args.kern)
    chars = [chr(kf.FIRST + i) for i in range(kf.COUNT)]
    if args.pairs:
        for a in chars:
            pairs = []
            for b in chars:
                k = kf.kern_pair(kern, a, b)
                if k:
                    g = kf.gap(glyphs, a, b, k)
                    pairs.append(f"{a}{b} {k}" + (f"/{g}" if g is not None else ""))
            if pairs:
                print("  ".join(pairs))
        print("(kern/blank columns left)")
        for p in kf.kern_problems(chars, glyphs, kern):
            print(p)
        return
    images = []
    for text in args.text or SAMPLES:
        unknown = sorted(set(text) - set(chars))
        if unknown:
            sys.exit(f"no glyphs for {unknown}")
        for kerned in [False, True] if args.c else [True]:
            rows, marks = raster(glyphs, kern, text, kerned)
            images.append(rows)
            print(terminal(rows))
            if kerned:
                print(marks)
    if args.png:
        write_png(args.png, images, args.scale, args.margin)


if __name__ == "__main__":
    main()
