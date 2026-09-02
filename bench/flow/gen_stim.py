#!/usr/bin/env python3
"""Generic random stimulus generator for GEM (naive_sim / cuda_test).

    gen_stim.py TOP CYCLES out.vcd port:width [port:width ...]

Same timing convention as the parta/partb/partc mkstim.py scripts: inputs
change on the falling edge (t = 10i), the clock rises in between (t = 10i+5),
so there is never a data/clock race. A port literally named `rst` is held high
for the first two cycles and pulsed rarely afterwards; a port named `ce` is high
~3/4 of the time. Everything else is uniformly random. `clk` is added
automatically and must NOT be listed.
"""
import random, sys

top, cycles, out = sys.argv[1], int(sys.argv[2]), sys.argv[3]
ports = [(p.split(":")[0], int(p.split(":")[1])) for p in sys.argv[4:]]
random.seed(20260901)

# VCD identifier codes: printable ASCII from '!' upward, skipping nothing
codes = {}
for i, (name, _) in enumerate([("clk", 1)] + ports):
    codes[name] = chr(33 + i)

def draw(name, width, i):
    if name == "rst":
        return 1 if i < 2 or random.randrange(64) == 0 else 0
    if name == "ce":
        return 1 if random.randrange(4) != 0 else 0
    return random.getrandbits(width)

with open(out, "w") as f:
    f.write("$timescale 1ns $end\n$scope module %s $end\n" % top)
    f.write("$var wire 1 %s clk $end\n" % codes["clk"])
    for name, w in ports:
        f.write("$var wire %d %s %s $end\n" % (w, codes[name], name))
    f.write("$upscope $end\n$enddefinitions $end\n")
    prev = {}
    for i in range(cycles):
        f.write("#%d\n0%s\n" % (10 * i, codes["clk"]))
        for name, w in ports:
            v = draw(name, w, i)
            if prev.get(name) != v:
                if w == 1:
                    f.write("%d%s\n" % (v, codes[name]))
                else:
                    f.write("b%s %s\n" % (format(v, "0%db" % w), codes[name]))
                prev[name] = v
        f.write("#%d\n1%s\n" % (10 * i + 5, codes["clk"]))
    f.write("#%d\n0%s\n" % (10 * cycles, codes["clk"]))
print("wrote %s (%d cycles, %d input ports)" % (out, cycles, len(ports)))
