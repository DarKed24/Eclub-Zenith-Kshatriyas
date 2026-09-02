#!/usr/bin/env python3
"""Compare GEM's naive_sim output VCD against the Icarus reference.

Samples naive.vcd at each rising edge (t = 5, 15, 25, ...) and diffs the two
primary outputs (`sum_out[7:0]`, `cout`) against iv_out.txt line by line.

naive_sim writes multi-bit ports bit-blasted (`$var wire 1 ! sum_out[7]`), so a
vector is reassembled MSB-first from its individual bit identifiers.
"""
import sys

vcd_path = sys.argv[1] if len(sys.argv) > 1 else "naive.vcd"
ref_path = sys.argv[2] if len(sys.argv) > 2 else "iv_out.txt"

# (name, width) in the same order tb_e2e.v writes them
ORDER = [("sum_out", 8), ("cout", 1)]

ident = {}
cur = {}
samples = {}
t = None
with open(vcd_path) as f:
    for line in f:
        line = line.strip()
        if line.startswith("$var"):
            p = line.split()
            ident[p[3]] = " ".join(p[4:-1])       # names may contain [i]
        elif line.startswith("#"):
            if t is not None:
                samples[t] = dict(cur)
            t = int(line[1:])
        elif line.startswith("b"):
            val, sym = line.split()
            cur[ident.get(sym, sym)] = val[1:]
        elif line and line[0] in "01xzXZ" and len(line) >= 2:
            cur[ident.get(line[1:], line[1:])] = line[0]
if t is not None:
    samples[t] = dict(cur)


def sample(state, name, width):
    """Read `name` from a VCD state, bit-blasted or as a vector, MSB first."""
    if width == 1:
        return state.get(name, "?")
    if name in state:                              # vector form, may be short
        v = state[name]
        pad = "0" if v[0] in "01" else v[0]
        return pad * (width - len(v)) + v
    return "".join(state.get("%s[%d]" % (name, i), "?")
                   for i in range(width - 1, -1, -1))


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
    got = "".join(sample(samples[ts], n, w) for n, w in ORDER)
    if got != expect:
        bad += 1
        if bad <= 5:
            print("cycle %d (t=%d): gem=%s iverilog=%s" % (i, ts, got, expect))

if bad == 0:
    print("PASS: %d cycles compared (%d skipped as x), "
          "GEM naive_sim == Icarus on sum_out + cout" % (checked, skipped))
    sys.exit(0)
print("FAIL: %d mismatching cycles out of %d" % (bad, checked))
sys.exit(1)
