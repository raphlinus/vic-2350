#!/usr/bin/env python3
# Copyright 2026 Raph Levien
# SPDX-License-Identifier: Apache-2.0 OR MIT

"""List-schedule a straight-line block of symbolasm f32 code.

Usage: tools/sched.py FILE START END REGS [--write]
  REGS like s0-s15 or s0-s15,s16-s23

Prints the block reordered so that, in symbolasm --analyze's model, no
result is used by the very next instruction, with s registers chosen from
REGS; --write puts it in place of lines START..END. Each value keeps the
operands and order of operations it has in the source, so the results are
bit-exact.

Reads lines START..END (1-based, inclusive) of FILE. Each statement is one of
  x [@sN] = *(p + off) | *p          f32 load
  (a [@sN], b, ...) = *p | *p++      vldm (consecutive registers)
  *(p + off) | *p = x                f32 store
  x [@sN] = a op b   (op + - *)      FP op
  x [@sN] = -a | max(a, b) | min(a, b)
  x [@sN] = 0.5                      constant
  x += a * b, x -= a * b             split into a product and an add, which
                                     rounds the same as vmla/vmls
  anything else                      a core statement, kept in order with the
                                     other core statements; its variables are
                                     taken from @rN placements
Comments and blank lines are dropped.

The model, from symbolasm --analyze: a result other than a load's is late
for the very next instruction; f32 loads aren't late; a core load is late
for the next instruction. Each op costs 1 cycle except vldm (N). Memory:
accesses through the same pointer variable (version) and offset, or through
different versions of the same pointer, stay in order; different pointer
variables are assumed not to alias.
"""
import re
import sys

FP = re.compile(r'^\s*([A-Za-z_]\w*)\s*(?:@(s\d+))?\s*(\+=|-=|=)\s*(.*?)\s*$')


def parse_regs(spec):
    regs = []
    for part in spec.split(','):
        a, _, b = part.partition('-')
        lo = int(a[1:])
        hi = int(b[1:]) if b else lo
        regs += [f's{i}' for i in range(lo, hi + 1)]
    return regs


class Op:
    def __init__(self, kind, text=None):
        self.kind = kind  # load vldm store fp core
        self.dst = []  # SSA values defined
        self.src = []  # SSA values used
        self.text = text
        self.deps = set()  # ops that must come before
        self.lat_deps = set()  # deps whose result is late by one
        self.cost = 1
        self.core_w = set()
        self.core_r = set()
        self.mem = None  # (base name, base version, offset, is_store)


def main():
    path, start, end, regspec = sys.argv[1:5]
    write = '--write' in sys.argv
    start, end = int(start), int(end)
    lines = open(path).read().split('\n')
    block = lines[start - 1:end]
    indent = re.match(r'\s*', next(l for l in block if l.strip())).group(0)
    regs = parse_regs(regspec)

    ops = []
    version = {}  # source name -> current SSA value
    names = {}  # SSA value -> source name
    counter = [0]
    core_version = {}  # core var -> version count

    def new_value(name):
        counter[0] += 1
        v = f'{name}#{counter[0]}'
        names[v] = name
        version[name] = v
        return v

    def use(name):
        if name not in version:
            raise SystemExit(f'use of {name} before its definition in the block')
        return version[name]

    fp_vars = set()

    def addr(expr):
        expr = expr.strip()
        m = re.match(r'^\*\(\s*(\w+)\s*([+-])\s*(\d+)\s*\)$', expr)
        if m:
            off = int(m.group(3)) * (1 if m.group(2) == '+' else -1)
            return m.group(1), off
        m = re.match(r'^\*(\w+)$', expr)
        if m:
            return m.group(1), 0
        return None

    for raw in block:
        line = raw.split('//')[0].strip()
        if not line:
            continue
        # vldm
        m = re.match(r'^\((.*)\)\s*=\s*\*(\w+)(\+\+)?$', line)
        if m and ('@s' in m.group(1) or all(
                x.strip().split()[0] in fp_vars for x in m.group(1).split(','))):
            op = Op('vldm')
            base = m.group(2)
            op.core_r.add(base)
            if m.group(3):
                op.core_w.add(base)
            for item in m.group(1).split(','):
                name = item.strip().split()[0]
                fp_vars.add(name)
                op.dst.append(new_value(name))
            op.cost = len(op.dst)
            op.mem = (base, core_version.get(base, 0), 'vldm', False)
            op.post = bool(m.group(3))
            op.base = base
            ops.append(op)
            continue
        # store
        if line.startswith('*'):
            lhs, rhs = [x.strip() for x in line.split('=', 1)]
            a = addr(lhs)
            if a and rhs in fp_vars:
                op = Op('store')
                op.src = [use(rhs)]
                op.core_r.add(a[0])
                op.mem = (a[0], core_version.get(a[0], 0), a[1], True)
                op.addr = a
                ops.append(op)
                continue
        m = FP.match(line)
        if m:
            name, place, assign, rhs = m.groups()
            a = addr(rhs)
            is_fp = place is not None or name in fp_vars
            if is_fp and assign == '=' and a and not rhs.endswith('++'):
                op = Op('load')
                op.core_r.add(a[0])
                op.mem = (a[0], core_version.get(a[0], 0), a[1], False)
                op.addr = a
                fp_vars.add(name)
                op.dst = [new_value(name)]
                ops.append(op)
                continue
            if is_fp and assign in ('+=', '-='):
                mm = re.match(r'^(\w+)\s*\*\s*(\w+)$', rhs)
                assert mm, line
                prod = Op('fp')
                prod.expr = ('*', mm.group(1), mm.group(2))
                prod.src = [use(mm.group(1)), use(mm.group(2))]
                prod.dst = [new_value('pr')]
                fp_vars.add('pr')
                ops.append(prod)
                acc = Op('fp')
                acc.expr = (assign[0], name, 'pr')
                acc.src = [use(name), prod.dst[0]]
                acc.dst = [new_value(name)]
                ops.append(acc)
                continue
            if is_fp:
                op = Op('fp')
                mm = re.match(r'^(\w+)\s*([-+*])\s*(\w+)$', rhs)
                if mm:
                    op.expr = (mm.group(2), mm.group(1), mm.group(3))
                    op.src = [use(mm.group(1)), use(mm.group(3))]
                elif re.match(r'^-(\w+)$', rhs):
                    x = rhs[1:]
                    op.expr = ('neg', x)
                    op.src = [use(x)]
                elif re.match(r'^(max|min)\((\w+),\s*(\w+)\)$', rhs):
                    mm = re.match(r'^(max|min)\((\w+),\s*(\w+)\)$', rhs)
                    op.expr = (mm.group(1), mm.group(2), mm.group(3))
                    op.src = [use(mm.group(2)), use(mm.group(3))]
                elif re.match(r'^[-\d.]+$', rhs):
                    op.expr = ('const', rhs)
                else:
                    raise SystemExit(f'unhandled f32 statement: {line}')
                fp_vars.add(name)
                op.dst = [new_value(name)]
                ops.append(op)
                continue
        # core statement
        op = Op('core', line)
        words = set(re.findall(r'[A-Za-z_]\w*', line))
        mm = re.match(r'^(\w+)\s*(?:@r\d+|@lr)?\s*(=|\+=|-=|<<=|>>=)', line)
        if mm:
            op.core_w.add(mm.group(1))
            core_version[mm.group(1)] = core_version.get(mm.group(1), 0) + 1
        op.core_r = words - op.core_w | ({mm.group(1)} if mm and mm.group(2) != '=' else set())
        for w in words & fp_vars:
            op.src.append(use(w))
        ops.append(op)

    # dependencies
    producer = {}
    for op in ops:
        for v in op.dst:
            producer[v] = op
    for i, op in enumerate(ops):
        for v in op.src:
            p = producer.get(v)
            if p is not None:
                op.deps.add(p)
                if p.kind in ('fp', 'core'):
                    op.lat_deps.add(p)
        for p in ops[:i]:
            # core register ordering
            if op.core_w & (p.core_r | p.core_w) or op.core_r & p.core_w:
                op.deps.add(p)
                if p.kind == 'core' and op.core_r & p.core_w:
                    op.lat_deps.add(p)
            if p.kind == 'core' and op.kind == 'core':
                op.deps.add(p)
            # memory
            if op.mem and p.mem and (op.mem[3] or p.mem[3]):
                same_base = op.mem[0] == p.mem[0]
                if same_base and (op.mem[1] != p.mem[1] or op.mem[2] == p.mem[2]
                                  or 'vldm' in (op.mem[2], p.mem[2])):
                    op.deps.add(p)

    # critical path priority
    prio = {}
    for op in reversed(ops):
        succ = [s for s in ops if op in s.deps]
        prio[op] = op.cost + max(
            [prio[s] + (1 if op in s.lat_deps else 0) for s in succ], default=0)

    def schedule(window):
        readers = {}
        for op in ops:
            for v in op.src:
                readers[v] = readers.get(v, 0) + 1
        free = list(regs)
        reg_of = {}
        done = []
        scheduled = set()
        last = None
        cycle = 0
        order = {op: i for i, op in enumerate(ops)}
        remaining = list(ops)
        while remaining:
            lo = min(order[op] for op in remaining)
            ready = [op for op in remaining if op.deps <= scheduled
                     and order[op] < lo + window]

            def regs_ok(op):
                if not op.dst:
                    return True
                dying = [reg_of[v] for v in op.src if readers[v] == 1 and v in reg_of]
                pool = free + dying
                if op.kind == 'vldm':
                    n = len(op.dst)
                    return find_run(pool, n, regs) is not None
                return len(set(pool)) >= len(op.dst)

            def stalls(op):
                return last is not None and last in op.lat_deps

            cands = [op for op in ready if regs_ok(op)]
            if not cands:
                return None
            live = len(regs) - len(free)
            # under pressure, prefer ops that free registers
            def key(op):
                frees = sum(1 for v in op.src if readers[v] == 1) - len(op.dst)
                pressure = live > len(regs) - 4
                return (stalls(op), -(frees if pressure else 0), -prio[op], order[op])
            op = min(cands, key=key)
            if stalls(op):
                cycle += 1
            cycle += op.cost
            # assign registers
            dying = [reg_of[v] for v in op.src if readers[v] == 1 and v in reg_of]
            for v in op.src:
                readers[v] -= 1
            pool = free + [r for r in dying if r not in free]
            if op.kind == 'vldm':
                run = find_run(pool, len(op.dst), regs)
                for v, r in zip(op.dst, run):
                    reg_of[v] = r
            else:
                # prefer reusing a dying operand's register, then the lowest free
                for v in op.dst:
                    r = next((r for r in dying if r in pool), None) or min(
                        pool, key=regs.index)
                    pool.remove(r)
                    reg_of[v] = r
            free = [r for r in pool if r not in [reg_of[v] for v in op.dst]]
            for r in dying:
                if r not in free and r not in [reg_of[v] for v in op.dst]:
                    free.append(r)
            for v in op.dst:
                if readers.get(v, 0) == 0 and op.kind != 'core':
                    # never read: free at once
                    free.append(reg_of[v])
            free = [r for r in regs if r in free]
            done.append(op)
            scheduled.add(op)
            remaining.remove(op)
            last = op

        return done, reg_of, cycle

    # Greedy list scheduling can run out of registers by starting too many
    # values at once, so also try it within a window of the source order,
    # and keep the fastest.
    best = None
    for window in range(len(ops), 1, -1):
        r = schedule(window)
        if r and (best is None or r[2] < best[2]):
            best = r
    if best is None:
        raise SystemExit('out of registers; give more or reorder the source')
    done, reg_of, cycle = best

    def vname(v):
        return out_name[v]

    # A value takes its source name when no other value of that name is
    # still live, else the name and its occurrence in the source, so that
    # the result reads like the source.
    out_name = {}
    taken = set(names.values())
    last_use = {}
    for i, op in enumerate(done):
        for v in op.src:
            last_use[v] = i
    occurrence = {}
    count = {}
    for op in ops:
        for v in op.dst:
            n = names[v]
            count[n] = count.get(n, 0) + 1
            occurrence[v] = count[n]
    live = {}  # output name -> value
    for i, op in enumerate(done):
        for v in op.dst:
            n = names[v]
            cur = live.get(n)
            if cur is None or last_use.get(cur, -1) <= i:
                cand = n
            else:
                k = occurrence[v]
                cand = f'{n}{k}'
                if (n[-1].isdigit() or cand in taken
                        or re.match(r'^([rsdq]\d+|lr|sp|pc)$', cand)):
                    cand = f'{n}_{k}'
                while cand in live and last_use.get(live[cand], -1) > i:
                    k += 100
                    cand = f'{n}_{k}'
            live[cand] = v
            out_name[v] = cand

    def a_text(a):
        base, off = a
        if off == 0:
            return f'*{base}'
        return f'*({base} {"+" if off > 0 else "-"} {abs(off)})'

    out = []
    for op in done:
        if op.kind == 'core':
            out.append(op.text)
        elif op.kind == 'load':
            v = op.dst[0]
            out.append(f'{vname(v)} @{reg_of[v]} = {a_text(op.addr)}')
        elif op.kind == 'vldm':
            items = ', '.join(f'{vname(v)} @{reg_of[v]}' for v in op.dst)
            out.append(f'({items}) = *{op.base}{"++" if op.post else ""}')
        elif op.kind == 'store':
            out.append(f'{a_text(op.addr)} = {vname(op.src[0])}')
        else:
            v = op.dst[0]
            e = op.expr
            srcs = [vname(s) for s in op.src]
            if e[0] in '+-*' and len(e) == 3:
                rhs = f'{srcs[0]} {e[0]} {srcs[1]}'
            elif e[0] == 'neg':
                rhs = f'-{srcs[0]}'
            elif e[0] in ('max', 'min'):
                rhs = f'{e[0]}({srcs[0]}, {srcs[1]})'
            elif e[0] == 'const':
                rhs = e[1]
            out.append(f'{vname(v)} @{reg_of[v]} = {rhs}')
    print(f'// {len(ops)} ops, about {cycle} cycles', file=sys.stderr)
    text = [indent + l for l in out]
    if write:
        lines[start - 1:end] = text
        open(path, 'w').write('\n'.join(lines))
    else:
        print('\n'.join(text))


def find_run(pool, n, regs):
    idx = sorted(regs.index(r) for r in set(pool))
    s = set(idx)
    for i in idx:
        if all(i + k in s for k in range(n)):
            return [regs[i + k] for k in range(n)]
    return None


if __name__ == '__main__':
    main()
