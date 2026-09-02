#!/usr/bin/env python3
"""Compare two GEM output VCDs (bit-blasted ports) sample-by-sample.

    cmp_vcd.py a.vcd b.vcd [--after N]   # ignore the first N cycles (reset)

Prints PASS/FAIL, the number of samples compared, and the first mismatch.
Both VCDs must come from GEM (naive_sim or cuda_test) on the same stimulus,
so timestamps line up 1:1.
"""
import sys

def load(path):
    names, vals, times = {}, {}, {}
    t = None
    for line in open(path):
        line = line.strip()
        if line.startswith("$var"):
            p = line.split()
            names[p[3]] = p[4]
        elif line.startswith("#"):
            t = int(line[1:]); times[t] = {}
        elif t is not None and line and line[0] in "01xz":
            times[t][names[line[1:]]] = line[0]
    # forward-fill into per-time full state
    state, seq = {}, []
    for t in sorted(times):
        state.update(times[t]); seq.append((t, dict(state)))
    return seq

a, b = load(sys.argv[1]), load(sys.argv[2])
after = int(sys.argv[sys.argv.index("--after") + 1]) if "--after" in sys.argv else 0
n, first = 0, None
for (ta, sa), (tb, sb) in zip(a, b):
    if ta != tb:
        print("FAIL: timestamps diverge (%d vs %d)" % (ta, tb)); sys.exit(1)
    if ta < 10 * after: continue
    n += 1
    for k in sorted(set(sa) | set(sb)):
        if sa.get(k) != sb.get(k):
            first = (ta, k, sa.get(k), sb.get(k)); break
    if first: break
if first:
    print("FAIL after %d samples: t=%d %s: %s vs %s" % (n, *first)); sys.exit(1)
print("PASS: %d samples identical on %d output bits" % (n, len(a[-1][1]))); sys.exit(0)
