#!/usr/bin/env python3
"""Compare GEM's naive_sim output VCD against the Icarus reference.

Samples naive.vcd at each rising edge (t = 5, 15, 25, ...) and diffs the four
primary outputs against iv_out.txt line by line.
"""
import sys

vcd_path = sys.argv[1] if len(sys.argv) > 1 else "naive.vcd"
ref_path = sys.argv[2] if len(sys.argv) > 2 else "iv_out.txt"

ORDER = ["and_out", "or_out", "inv_out", "areg_out"]

ident = {}
cur = {}
samples = {}
t = None
with open(vcd_path) as f:
    for line in f:
        line = line.strip()
        if line.startswith("$var"):
            p = line.split()
            ident[p[3]] = p[4]
        elif line.startswith("#"):
            if t is not None:
                samples[t] = dict(cur)
            t = int(line[1:])
        elif line and line[0] in "01xzXZ" and len(line) >= 2:
            cur[ident.get(line[1:], line[1:])] = line[0]
        elif line.startswith("b"):
            val, sym = line.split()
            cur[ident.get(sym, sym)] = val[1:]
if t is not None:
    samples[t] = dict(cur)

ref = [l.strip() for l in open(ref_path) if l.strip()]

bad = 0
skipped = 0
checked = 0
for i, expect in enumerate(ref):
    # Verilog starts registers at x, GEM starts them at 0; the reference is
    # only meaningful once the first edge has propagated.
    if "x" in expect or "z" in expect:
        skipped += 1
        continue
    ts = 10 * i + 5
    if ts not in samples:
        print("cycle %d: no VCD sample at t=%d" % (i, ts))
        bad += 1
        continue
    checked += 1
    got = "".join(samples[ts].get(n, "?") for n in ORDER)
    if got != expect:
        bad += 1
        if bad <= 5:
            print("cycle %d (t=%d): gem=%s iverilog=%s" % (i, ts, got, expect))

if bad == 0:
    print("PASS: %d cycles compared (%d skipped as x), "
          "GEM naive_sim == Icarus on all 4 outputs" % (checked, skipped))
    sys.exit(0)
print("FAIL: %d mismatching cycles out of %d" % (bad, checked))
sys.exit(1)
