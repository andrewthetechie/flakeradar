#!/usr/bin/env python3
"""Report repeated blocks of source lines. Usage: dups.py MIN_LINES FILE..."""
import re
import sys
from collections import defaultdict

MIN = int(sys.argv[1])
files = sys.argv[2:]
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
        key = tuple(text for _, text in rows[i : i + MIN])
        windows[key].append((path, i))

# Grow each pair of matches to its maximal extent, once.
seen = set()
blocks = []
for key, places in windows.items():
    if len(places) < 2:
        continue
    for a in range(len(places)):
        for b in range(a + 1, len(places)):
            (pa, ia), (pb, ib) = places[a], places[b]
            if (pa, ia, pb, ib) in seen:
                continue
            ra, rb = norm[pa], norm[pb]
            n = 0
            while ia + n < len(ra) and ib + n < len(rb) and ra[ia + n][1] == rb[ib + n][1]:
                seen.add((pa, ia + n, pb, ib + n))
                n += 1
            if pa == pb and ib < ia + n:
                continue
            blocks.append((n, pa, ra[ia][0], ra[ia + n - 1][0], pb, rb[ib][0], rb[ib + n - 1][0]))

blocks.sort(reverse=True)
total = sum(b[0] for b in blocks)
print(f"# {len(blocks)} repeated block pairs of >= {MIN} normalized lines; {total} lines in the second copies")
for n, pa, a1, a2, pb, b1, b2 in blocks[:40]:
    print(f"{n:4d}  {pa.split('crates/')[-1]}:{a1}-{a2}  <->  {pb.split('crates/')[-1]}:{b1}-{b2}")
