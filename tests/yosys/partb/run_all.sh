#!/usr/bin/env bash
# Runs every Part B Yosys verification step from `.claude/yosysB.md` in order.
#
#   GEM=/path/to/GEM  YOSYS=~/yosys/build/yosys  ./run_all.sh [workdir]
#
# Defaults assume the WSL layout described in yosysB.md. Exits non-zero on the
# first step that does not produce the expected result. Step 10 (the GPU
# differential) is skipped unless a CUDA-enabled cuda_test binary is present.
set -uo pipefail

GEM="${GEM:-/mnt/c/Users/Devansh/GEM}"
YOSYS="${YOSYS:-$HOME/yosys/build/yosys}"
OSSCAD="${OSSCAD:-$HOME/oss-cad-suite/bin}"
TARGET="${CARGO_TARGET_DIR:-$HOME/gem-target}"
SRC="$GEM/tests/yosys/partb"
WORK="${1:-$HOME/gemB-run}"

export PATH="$OSSCAD:$PATH"
export CARGO_TARGET_DIR="$TARGET"

fails=0
step() { printf "\n=== %s ===\n" "$1"; }
ok()   { printf "  [ok]   %s\n" "$1"; }
bad()  { printf "  [FAIL] %s\n" "$1"; fails=$((fails + 1)); }

# final `stat` block of a yosys log (there may be several)
laststat() { awk '/Printing statistics/{n++} n==N' N="$(grep -c 'Printing statistics' "$1")" "$1"; }

rm -rf "$WORK"; mkdir -p "$WORK/out" "$WORK/log"; cd "$WORK" || exit 1
cp "$SRC"/*.v "$SRC"/*.py .

step "0. environment"
"$YOSYS" -V || { echo "no yosys at $YOSYS"; exit 1; }
command -v iverilog >/dev/null && iverilog -V 2>/dev/null | head -1

step "1. CPU substrate (cargo)"
( cd "$GEM" && cargo test -q >/dev/null 2>&1 ) && ok "cargo test" || bad "cargo test"
( cd "$GEM" && cargo run -q --bin macro_test 2>&1 | grep -q "all differential checks passed" ) \
  && ok "macro_test differential" || bad "macro_test differential"

step "2. premise - Yosys 0.68 arith_map.v still emits CARRY4"
"$YOSYS" -q -l log/premise.log -p "
read_verilog $GEM/aigpdk/gemcarry.v
read_verilog add_rtl.v
hierarchy -check -top add_rtl8
synth -flatten -run begin:fine
techmap -map +/xilinx/arith_map.v -D LUT_SIZE=6
stat" >/dev/null 2>&1
laststat log/premise.log | grep -q "2   CARRY4" \
  && ok "\$alu -> 2 CARRY4 on an 8-bit add" \
  || bad "arith_map.v no longer produces CARRY4 - Path B premise broken"

step "3. Path A - explicit CARRY4 -> aigpdk netlist"
cat > b1.ys <<EOF
read_verilog $GEM/aigpdk/gemcarry.v
read_verilog carry_top.v
hierarchy -check -top yadder
synth -flatten
delete t:\$print
dfflibmap -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge
abc -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge
stat
write_verilog -noattr out/yadder.gv
EOF
"$YOSYS" -q -l log/b1.log b1.ys >/dev/null 2>&1
laststat log/b1.log | grep -q "2   CARRY4" && ok "2 CARRY4 survive synth + abc" \
                                           || bad "CARRY4 did not survive"
laststat log/b1.log | grep -q "16   DFF"   && ok "16 DFF + AND2 glue mapped" \
                                           || bad "DFF/glue mapping"
if diff -q <(grep -v '^/\*' out/yadder.gv) <(grep -v '^/\*' "$GEM/tests/data/yosys_carry4.gv") >/dev/null; then
  ok "identical to tests/data/yosys_carry4.gv (banner comment ignored)"
else
  bad "netlist differs from the checked-in fixture"
fi

step "4. Path A - RTL vs gate netlist (iverilog)"
sed 's/^module yadder(/module yadder_gate(/' out/yadder.gv > out/yadder_gate.v
iverilog -g2005 -o sim_b1 "$GEM/aigpdk/aigpdk.v" "$GEM/aigpdk/gemcarry.v" \
         carry_top.v out/yadder_gate.v tb_b1.v 2>/dev/null
./sim_b1 | grep -q "^PASS" && ok "512 cycles bit-exact" || bad "RTL vs gate netlist"

step "5. Path B - infer CARRY4 from plain + RTL"
cat > b2.ys <<EOF
read_verilog $GEM/aigpdk/gemcarry.v
read_verilog add_rtl.v
hierarchy -check -top add_rtl
synth -flatten -run begin:fine
techmap -map +/xilinx/arith_map.v -D LUT_SIZE=6
techmap -map $GEM/aigpdk/gemcarry_map.v
synth -run fine:
delete t:\$print
dfflibmap -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge
abc -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge
stat
write_verilog -noattr out/add_rtl.gv
EOF
"$YOSYS" -q -l log/b2.log b2.ys >/dev/null 2>&1
laststat log/b2.log | grep -q "4   CARRY4" && ok "16-bit add -> 4 CARRY4" \
                                           || bad "Path B inferred no CARRY4"
laststat log/b2.log | grep -q "16   DFF"   && ok "16 DFF + AND2 glue mapped" \
                                           || bad "Path B glue mapping"

# the wrong -run label pair: silently shreds the chain, no error
cat > b2bad.ys <<EOF
read_verilog $GEM/aigpdk/gemcarry.v
read_verilog add_rtl.v
hierarchy -check -top add_rtl
synth -flatten
delete t:\$print
dfflibmap -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge
abc -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge
stat
write_verilog -noattr out/add_rtl_shredded.gv
EOF
"$YOSYS" -q -l log/b2bad.log b2bad.ys >/dev/null 2>&1
if grep -q "CARRY4" out/add_rtl_shredded.gv; then
  bad "the no-arith_map control should contain no CARRY4"
else
  ok "control: plain synth shreds the chain (0 CARRY4) - the pitfall is real"
fi

step "6. synth_xilinx contrast - why the generic flow is used"
"$YOSYS" -q -l log/b2x.log -p "
read_verilog add_rtl.v
hierarchy -check -top add_rtl
synth_xilinx -flatten
stat" >/dev/null 2>&1
if laststat log/b2x.log | grep -q "4   CARRY4" && laststat log/b2x.log | grep -q "FDRE"; then
  ok "synth_xilinx also infers 4 CARRY4 but leaves LUT2/FDRE (not aigpdk cells)"
else
  bad "synth_xilinx result changed - re-read yosysB.md Step 6"
fi

step "7. CARRY8 -> 2x CARRY4 techmap"
cat > b8.ys <<EOF
read_verilog $GEM/aigpdk/gemcarry.v
read_verilog -lib $GEM/aigpdk/gemcarry_map.v
read_verilog carry8_top.v
hierarchy -check -top carry8_top
proc;;
techmap -map $GEM/aigpdk/gemcarry_map.v
opt_expr -fine; opt_clean
stat
write_verilog -noattr out/carry8_mapped.v
EOF
"$YOSYS" -q -l log/b8.log b8.ys >/dev/null 2>&1
laststat log/b8.log | grep -q "2   CARRY4" && ok "one CARRY8 -> two chained CARRY4" \
                                           || bad "CARRY8 split failed"
iverilog -g2005 -o sim_c8 "$GEM/aigpdk/gemcarry.v" out/carry8_mapped.v tb_carry8.v 2>/dev/null
./sim_c8 | grep -q "^PASS" && ok "1000 vectors == a + b + cin" || bad "CARRY8 arithmetic"

step "8. GEM reads the Yosys netlists and fuses the chain"
lv() { "$TARGET/debug/level_test" "$1" 2>&1 | grep -oP 'Number of levels: \K[0-9]+'; }
la=$(lv out/yadder.gv); lb=$(lv out/add_rtl.gv); ls_=$(lv out/add_rtl_shredded.gv)
[ "$la" = "2" ] && ok "Path A netlist: $la AIG levels" || bad "Path A levels = $la (want 2)"
[ "$lb" = "2" ] && ok "Path B netlist: $lb AIG levels" || bad "Path B levels = $lb (want 2)"
[ "$ls_" -gt 20 ] 2>/dev/null && ok "control (shredded): $ls_ AIG levels - the chain really is native" \
                              || bad "shredded control levels = $ls_ (want > 20)"
( cd "$GEM" && cargo run -q --bin macro_test 2>&1 | grep -E '^\[yadder\]' ) \
  | grep -q "1 macro(s), 2 major stage(s)" \
  && ok "the two slices fuse into ONE Carry4 macro (1 macro, 2 stages)" \
  || bad "fusion did not happen"

step "9. end-to-end - GEM naive_sim vs iverilog on the same netlist"
python3 mkstim.py 256 >/dev/null
iverilog -g2005 -o sim_e2e "$GEM/aigpdk/aigpdk.v" "$GEM/aigpdk/gemcarry.v" \
         out/yadder_gate.v tb_e2e.v 2>/dev/null
./sim_e2e >/dev/null
"$TARGET/debug/naive_sim" out/yadder.gv stim.vcd naive.vcd >/dev/null 2>&1
cmp_out=$(python3 cmpvcd.py naive.vcd iv_out.txt 2>&1)
echo "  $cmp_out"
printf '%s' "$cmp_out" | grep -q "^PASS" \
  && ok "GEM == Icarus over 255 cycles" || bad "end-to-end differential"

step "10. GPU differential (optional)"
if [ -x "$TARGET/debug/cuda_test" ] && [ -x "$TARGET/debug/cut_map_interactive" ]; then
  # cudaenv.sh appends to possibly-unset vars; don't let `set -u` kill it
  if [ -f "$HOME/cudaenv.sh" ]; then set +u; . "$HOME/cudaenv.sh"; set -u; fi
  "$TARGET/debug/cut_map_interactive" out/yadder.gv out/yadder.gemparts >/dev/null 2>&1
  gpu=$("$TARGET/debug/cuda_test" out/yadder.gv out/yadder.gemparts stim.vcd out/gpu.vcd 1 \
        --check-with-cpu 2>&1)
  printf '%s' "$gpu" | grep -q "sanity test passed" \
    && ok "cuda_test --check-with-cpu: GPU == CPU" || bad "GPU differential"
  printf '%s' "$gpu" | grep -q "Script hash: 2719296911916927336" \
    && ok "script hash matches macro_test's YADDER_HASH" \
    || bad "script hash differs between entry points"
else
  echo "  [skip] no CUDA build (cargo build --features cuda)"
fi

printf "\n"
if [ "$fails" -eq 0 ]; then
  echo "ALL PART B YOSYS CHECKS PASSED  (workdir: $WORK)"
else
  echo "$fails CHECK(S) FAILED  (workdir: $WORK)"
fi
exit "$fails"
