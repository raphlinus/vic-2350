#!/usr/bin/env python3
# Copyright 2026 Raph Levien
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""Hill-climb register placements in one symbolasm function for code size.

Usage: tools/regswap.py FILE FUNC [--write]

symbolasm is run from $SYMBOLASM, default ../symbolasm beside this repo,
which must be built with cargo build --release.

Swaps every pair of registers in the function's @rN placements (not its
push and pop lists, so the saved set stays the same), keeping a swap when
the file still compiles and the function gets smaller, without more cycles in
its straight-line count. The compiler checks placements against calls, so
anything that compiles keeps the program's meaning; run the tests anyway.
"""
import itertools
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SYMBOLASM = os.path.join(
    os.environ.get('SYMBOLASM', os.path.join(HERE, '../../symbolasm')),
    'target/release/symbolasm')
REGS = [f'r{i}' for i in range(13)] + ['lr']


def measure(path, func):
    r = subprocess.run([SYMBOLASM, '--analyze', '--fn', func, '--size', 'n',
                        '--cols', 'size,cyc', path], capture_output=True, text=True)
    if r.returncode != 0:
        return None
    m = re.search(r'(\d+) bytes', r.stdout)
    cyc = 0
    for line in r.stdout.splitlines()[1:]:
        f = line.split()
        if len(f) == 2 and f[0] in '24':
            c = f[1].split('/')[-1]
            if c.isdigit():
                cyc += int(c)
    return int(m.group(1)), cyc


def func_span(lines, func):
    start = next(i for i, l in enumerate(lines) if l.startswith(f'fn {func}('))
    end = next(i for i in range(start, len(lines)) if lines[i] == '}')
    return start, end


def swap_text(text, a, b):
    return re.sub(r'(@)(r\d+|lr)\b', lambda m: m.group(1) + (
        b if m.group(2) == a else a if m.group(2) == b else m.group(2)), text)


def main():
    path, func = sys.argv[1:3]
    write = '--write' in sys.argv
    src = open(path).read()
    lines = src.split('\n')
    s, e = func_span(lines, func)
    body = '\n'.join(lines[s:e + 1])
    tmp = path + '.regswap.sy'

    def build(b):
        new = lines[:s] + b.split('\n') + lines[e + 1:]
        open(tmp, 'w').write('\n'.join(new))
        return measure(tmp, func)

    best = build(body)
    print(f'{func}: start {best}', file=sys.stderr)
    improved = True
    while improved:
        improved = False
        for a, b in itertools.combinations(REGS, 2):
            cand = swap_text(body, a, b)
            if cand == body:
                continue
            m = build(cand)
            if m and (m[0] < best[0]) and m[1] <= best[1]:
                print(f'  swap {a} {b}: {m}', file=sys.stderr)
                best, body, improved = m, cand, True
    os.remove(tmp)
    if write:
        new = lines[:s] + body.split('\n') + lines[e + 1:]
        open(path, 'w').write('\n'.join(new))
    print(f'{func}: end {best}', file=sys.stderr)


if __name__ == '__main__':
    main()
