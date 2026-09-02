#!/usr/bin/env bash
# Canonical GEM synthesis front-end for the Takneek Zenith flow.
#
#   gem_synth.sh macro|shred TOP out.gv design1.sv [design2.sv ...]
#
#   macro : DSP48E2 / CARRY4 / SRLC32E are intercepted and kept as native GEM
#           macro cells (DSP48E2 is rewritten to GEM_DSP48E2 with OPMODE folded
#           to the 2-bit state). Everything else -> aigpdk AIG cells.
#   shred : the SAME design, but the three primitives are replaced by their
#           behavioural models and flattened into the AIG. This is the honest
#           baseline (what unmodified GEM would have to simulate).
#
# Requires: yosys 0.68 with the slang plugin (oss-cad-suite), $GEM pointing at
# the repo root. SystemVerilog (IEEE 1800-2012) is read through slang.
set -euo pipefail
MODE="$1"; TOP="$2"; OUT="$3"; shift 3
GEM="${GEM:?set GEM=/path/to/repo}"
YOSYS="${YOSYS:-yosys}"
FLOW="$(cd "$(dirname "$0")" && pwd)"
LOG="${OUT%.gv}.$MODE.log"

case "$MODE" in
  macro) PRIMS="$FLOW/dsp48e2_bb.v $GEM/aigpdk/gemcarry.v $GEM/aigpdk/gemsrl.v"; DSPMODEL="$GEM/aigpdk/gemmacro.v" ;;
  shred) PRIMS="$FLOW/dsp48e2_bb.v $FLOW/gemcarry_shred.v $FLOW/gemsrl_shred.v"; DSPMODEL="$GEM/aigpdk/gemmacro_shred.v" ;;
  *) echo "mode must be macro or shred"; exit 2 ;;
esac

"$YOSYS" -q -l "$LOG" -p "
plugin -i slang
read_slang --top $TOP $* $PRIMS
hierarchy -check -top $TOP
proc;;
opt_expr; opt_dff; opt_clean
memory -nomap
memory_libmap -lib $GEM/aigpdk/memlib_yosys.txt -logic-cost-rom 100 -logic-cost-ram 100
techmap -map $GEM/aigpdk/gemmacro_map.v
read_verilog $DSPMODEL
proc
hierarchy -check -top $TOP
opt_expr -fine; opt_clean
synth -flatten
delete t:\$print
dfflibmap -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge
abc -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge
techmap
abc -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge
setparam -unset INIT -unset IS_CLK_INVERTED t:SRLC32E
rename -hide w:*.*
stat
write_verilog -noattr $OUT
" >/dev/null
# GEM's netlist reader (sverilogparse) accepts neither the `signed` qualifier
# nor escaped hierarchical wire names; both are structurally meaningless in a
# gate-level netlist. Hierarchical names are hidden above; `signed` goes here.
sed -i -E 's/^(\s*)(wire|input|output|reg) signed /\1\2 /' "$OUT"
# summary line: macro cells surviving + total cells
awk '/=== '"$TOP"' ===/,0' "$LOG" | grep -E "^\s+[0-9]+\s+(cells|GEM_DSP48E2|CARRY4|SRLC32E|DFF|AND2)" | tr -s ' ' | tr '\n' ';'; echo
