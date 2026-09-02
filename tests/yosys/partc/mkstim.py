#!/usr/bin/env python3
"""Generate a race-free stimulus for the Part C `ysrl` example.

Writes two views of the *same* vectors:
  stim.vcd  - for GEM  (naive_sim / cuda_test read this as the input waveform)
  stim.txt  - for Icarus ($readmemb: one line per cycle, "ce_in d_in a_in")

Inputs change on the falling edge (t = 0, 10, 20, ...) and the clock rises in
between (t = 5, 15, 25, ...), so neither simulator sees a data/clock race.

`ce_in` is high about three cycles in four, so the shift register both advances
and holds within the run, and `a_in` sweeps the whole 5-bit address space.
"""
import random
import sys

CYCLES = int(sys.argv[1]) if len(sys.argv) > 1 else 256
random.seed(20260831)

vec = []
for _ in range(CYCLES):
    vec.append({
        "ce_in": 1 if random.randrange(4) else 0,
        "d_in": random.randrange(2),
        "a_in": random.randrange(32),
    })

with open("stim.vcd", "w") as f:
    f.write("$timescale 1ns $end\n")
    f.write("$scope module ysrl $end\n")
    f.write("$var wire 1 ! clk $end\n")
    f.write("$var wire 1 \" ce_in $end\n")
    f.write("$var wire 1 # d_in $end\n")
    f.write("$var wire 5 $ a_in $end\n")
    f.write("$upscope $end\n$enddefinitions $end\n")
    prev = None
    for i, v in enumerate(vec):
        f.write("#%d\n" % (10 * i))
        f.write("0!\n")
        if prev is None or v["ce_in"] != prev["ce_in"]:
            f.write("%d\"\n" % v["ce_in"])
        if prev is None or v["d_in"] != prev["d_in"]:
            f.write("%d#\n" % v["d_in"])
        if prev is None or v["a_in"] != prev["a_in"]:
            f.write("b%s $\n" % format(v["a_in"], "05b"))
        f.write("#%d\n1!\n" % (10 * i + 5))
        prev = v
    f.write("#%d\n0!\n" % (10 * len(vec)))

with open("stim.txt", "w") as f:
    for v in vec:
        f.write("%d%d%s\n" % (v["ce_in"], v["d_in"],
                              format(v["a_in"], "05b")))

print("wrote stim.vcd and stim.txt (%d cycles)" % CYCLES)
