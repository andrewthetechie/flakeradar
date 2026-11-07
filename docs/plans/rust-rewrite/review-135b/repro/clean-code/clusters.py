#!/usr/bin/env python3
"""Group dups.py's normalized 8-line windows into clusters of copies.

dups.py reports pairs, so a block copied n times shows as n(n-1)/2 rows. This
uses the same normalization and picks, greedily, the block whose removal saves
the most lines, then marks its lines as used.
"""
import re
import sys
from collections import defaultdict

MIN = 8
files = sys.argv[1:]
norm = {}
for path in files:
    rows = []
    for number, line in enumerate(open(path), 1):
        text = line.strip()
        if not text or text.startswith("//") or text in {"}", "};", ")", ");", "{", "})", "},", "),"}:
            continue
        text = re.sub(r"\b(test|job|Test|Job|tests|jobs|t|j)\b", "E", text)
        text = re.sub(r"(test_case|test|job)_", "E_", text)
        rows.append((number, text))
    norm[path] = rows

windows = defaultdict(list)
for path, rows in norm.items():
    for i in range(len(rows) - MIN + 1):
        windows[tuple(t for _, t in rows[i : i + MIN])].append((path, i))


def nonoverlap(places, n):
    out, last = [], {}
    for path, i in sorted(places):
        if path in last and i < last[path] + n:
            continue
        out.append((path, i))
        last[path] = i
    return out


def extend(places):
    n = MIN
    while True:
        nexts = set()
        for path, i in places:
            rows = norm[path]
            nexts.add(rows[i + n][1] if i + n < len(rows) else None)
        if len(nexts) != 1 or None in nexts:
            return n
        n += 1


candidates = sorted((k for k, p in windows.items() if len(p) > 1), key=lambda k: -len(windows[k]))[:6000]
scored = []
for key in candidates:
    places = nonoverlap(windows[key], MIN)
    if len(places) < 2:
        continue
    n = extend(places)
    scored.append(((len(places) - 1) * n, n, places))
scored.sort(key=lambda s: -s[0])

used = set()
picked = []
for saving, n, places in scored:
    live = [(p, i) for p, i in places if not any((p, i + k) in used for k in range(n))]
    if len(live) < 2:
        continue
    for p, i in live:
        for k in range(n):
            used.add((p, i + k))
    picked.append(((len(live) - 1) * n, n, live))
    if len(picked) >= 12:
        break

picked.sort(key=lambda s: -s[0])
for saving, n, live in picked:
    byfile = defaultdict(list)
    for p, i in live:
        byfile[p.split("crates/")[-1]].append(norm[p][i][0])
    print(f"\n## saves {saving} lines: {len(live)} copies of {n} normalized lines")
    for f, lines in sorted(byfile.items(), key=lambda kv: -len(kv[1]))[:6]:
        print(f"   {f}: {len(lines)} at {lines[:6]}")
    p, i = live[0]
    for k in range(min(n, 14)):
        print("     |", norm[p][i + k][1][:110])
