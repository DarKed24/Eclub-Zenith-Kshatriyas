#!/usr/bin/env python3
"""Generate a race-free stimulus for the Part B `yadder` example.

Writes two views of the *same* vectors:
  stim.vcd  - for GEM  (naive_sim / cuda_test read this as the input waveform)
  stim.txt  - for Icarus ($readmemb: one line per cycle, "cin a_in b_in")

Inputs change on the falling edge (t = 0, 10, 20, ...) and the clock rises in
between (t = 5, 15, 25, ...), so neither simulator sees a data/clock race.
"""
import random
import sys

CYCLES = int(sys.argv[1]) if len(sys.argv) > 1 else 256
random.seed(20260831)

vec = []
for _ in range(CYCLES):
    vec.append({
        "cin": random.randrange(2),
        "a_in": random.randrange(256),
        "b_in": random.randrange(256),
    })

with open("stim.vcd", "w") as f:
    f.write("$timescale 1ns $end\n")
    f.write("$scope module yadder $end\n")
    f.write("$var wire 1 ! clk $end\n")
    f.write("$var wire 1 \" cin $end\n")
    f.write("$var wire 8 # a_in $end\n")
    f.write("$var wire 8 $ b_in $end\n")
    f.write("$upscope $end\n$enddefinitions $end\n")
    prev = None
    for i, v in enumerate(vec):
        f.write("#%d\n" % (10 * i))
        f.write("0!\n")
        if prev is None or v["cin"] != prev["cin"]:
            f.write("%d\"\n" % v["cin"])
        if prev is None or v["a_in"] != prev["a_in"]:
            f.write("b%s #\n" % format(v["a_in"], "08b"))
        if prev is None or v["b_in"] != prev["b_in"]:
            f.write("b%s $\n" % format(v["b_in"], "08b"))
        f.write("#%d\n1!\n" % (10 * i + 5))
        prev = v
    f.write("#%d\n0!\n" % (10 * len(vec)))

with open("stim.txt", "w") as f:
    for v in vec:
        f.write("%d%s%s\n" % (v["cin"],
                              format(v["a_in"], "08b"),
                              format(v["b_in"], "08b")))

print("wrote stim.vcd and stim.txt (%d cycles)" % CYCLES)
