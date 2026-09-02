#!/usr/bin/env bash
# Runs every Part C (SRLC32E) Yosys verification step in order, mirroring
# parta/partb. Validated against Yosys 0.68 (+ oss-cad-suite 0.68+136).
#
#   GEM=/path/to/GEM  YOSYS=~/yosys/build/yosys  ./run_all.sh [workdir]
#
# Exits non-zero on the first step that does not produce the expected result.
set -uo pipefail

GEM="${GEM:-/mnt/c/Users/Devansh/GEM}"
YOSYS="${YOSYS:-$HOME/yosys/build/yosys}"
OSSCAD="${OSSCAD:-$HOME/oss-cad-suite/bin}"
TARGET="${CARGO_TARGET_DIR:-$HOME/gem-target}"
SRC="$GEM/tests/yosys/partc"
WORK="${1:-$HOME/gemC-run}"

export PATH="$OSSCAD:$PATH"
export CARGO_TARGET_DIR="$TARGET"

fails=0
step() { printf "\n=== %s ===\n" "$1"; }
ok()   { printf "  [ok]   %s\n" "$1"; }
bad()  { printf "  [FAIL] %s\n" "$1"; fails=$((fails + 1)); }
# stat of the LAST design printed in a log (post-mapping counts)
laststat() { awk '/^=== /{buf=""} {buf=buf $0 "\n"} END{printf "%s", buf}' "$1"; }

rm -rf "$WORK"; mkdir -p "$WORK/out" "$WORK/log"; cd "$WORK"
cp "$SRC"/*.v "$SRC"/*.py .

step "0. environment"
"$YOSYS" -V || { echo "no yosys at $YOSYS"; exit 1; }
command -v iverilog >/dev/null && iverilog -V 2>/dev/null | head -1

step "1. CPU substrate (cargo)"
( cd "$GEM" && cargo test -q >/dev/null 2>&1 ) && ok "cargo test" || bad "cargo test"
( cd "$GEM" && cargo run -q --bin macro_test 2>&1 | grep -q "all differential checks passed" ) \
  && ok "macro_test differential" || bad "macro_test differential"

step "2. Path A - explicit SRLC32E (ysrl.v) -> aigpdk netlist"
cat > c1.ys <<EOF
read_verilog $GEM/aigpdk/gemsrl.v
read_verilog ysrl.v
hierarchy -check -top ysrl
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
stat
write_verilog -noattr out/ysrl.gv
EOF
"$YOSYS" -q -l log/c1.log c1.ys >/dev/null 2>&1
laststat log/c1.log | grep -q "1   SRLC32E" && ok "SRLC32E survives synth + abc" \
                                            || bad "SRLC32E missing from stat"
laststat log/c1.log | grep -q "6   DFF" && ok "6 DFF + aigpdk glue mapped" || bad "DFF mapping"
if diff -q <(grep -v '^/\*' out/ysrl.gv) <(grep -v '^/\*' "$GEM/tests/data/yosys_srl.gv") >/dev/null 2>&1; then
  ok "identical to tests/data/yosys_srl.gv (banner comment ignored)"
else
  bad "netlist differs from the checked-in fixture"
fi

step "3. Path A - RTL vs gate netlist (iverilog, 512 random cycles)"
sed 's/^module ysrl(/module ysrl_gate(/' out/ysrl.gv > out/ysrl_gate.v
iverilog -g2005 -o sim_c1 "$GEM/aigpdk/aigpdk.v" "$GEM/aigpdk/gemsrl.v" \
         ysrl.v out/ysrl_gate.v tb_c1.v 2>/dev/null
./sim_c1 | grep -q "^PASS" && ok "512 cycles bit-exact" || bad "RTL vs gate netlist"

step "4. Path B - RTL shift registers -> SRLC32E inference (Yosys 0.68)"
# top : expected SRLC32E count : extra grep on the mapped netlist (or _)
for pair in "srl8:1:_" "srl20:1:_" "srl32:1:_" "srl48:2:Q31" "srl32_ce:1:.CE(ce" "srl32_addr:1:_"; do
  top="${pair%%:*}"; rest="${pair#*:}"; want="${rest%%:*}"; extra="${rest#*:}"
  cat > "c2_$top.ys" <<EOF
read_verilog $GEM/aigpdk/gemsrl.v
read_verilog srl_rtl.v
hierarchy -check -top $top
synth_xilinx -flatten -noiopad -noclkbuf -run :map_cells
xilinx_srl -fixed -variable
techmap -map +/xilinx/cells_map.v
techmap -map $GEM/aigpdk/gemsrl_map.v
opt_clean -purge
setparam -unset INIT -unset IS_CLK_INVERTED t:SRLC32E
stat
write_verilog -noattr out/${top}_mapped.v
EOF
  "$YOSYS" -q -l "log/c2_$top.log" "c2_$top.ys" >/dev/null 2>&1
  got=$(laststat "log/c2_$top.log" | grep -oP '^\s+\K[0-9]+(?=\s+SRLC32E)' | tail -1)
  if [ "${got:-0}" = "$want" ]; then
    if [ "$extra" = "_" ] || grep -qF "$extra" "out/${top}_mapped.v"; then
      ok "$top -> $want SRLC32E$([ "$extra" != "_" ] && echo " (with $extra..)")"
    else
      bad "$top mapped but missing $extra in netlist"
    fi
  else
    bad "$top -> ${got:-0} SRLC32E (want $want)"
  fi
done
grep -q " SRL16E " out/srl8_mapped.v \
  && bad "srl8 left an unmapped SRL16E" \
  || ok "srl8: no SRL16E survives (remapped to SRLC32E, zero-extended A)"

step "5. SRL16E exactness - techmapped srl16_top vs behavioural gold (iverilog)"
cat > c3.ys <<EOF
read_verilog -lib +/xilinx/cells_sim.v
read_verilog $GEM/aigpdk/gemsrl.v
read_verilog srl16.v
hierarchy -check -top srl16_top
proc;;
techmap -map $GEM/aigpdk/gemsrl_map.v
opt_clean -purge
setparam -unset INIT -unset IS_CLK_INVERTED t:SRLC32E
stat
write_verilog -noattr out/srl16_mapped.v
EOF
"$YOSYS" -q -l log/c3.log c3.ys >/dev/null 2>&1
laststat log/c3.log | grep -q "1   SRLC32E" && ok "SRL16E -> SRLC32E" || bad "SRL16E techmap"
awk '/^module srl16_gold/,/^endmodule/' srl16.v > out/srl16_gold_only.v
iverilog -g2005 -o sim_srl16 "$GEM/aigpdk/gemsrl.v" out/srl16_mapped.v \
         out/srl16_gold_only.v tb_srl16.v 2>/dev/null
./sim_srl16 | grep -q "^PASS" && ok "srl16 mapped == gold" || bad "srl16 differential"

step "6. rejection guards"
cat > rej.ys <<EOF
read_verilog -lib +/xilinx/cells_sim.v
read_verilog $GEM/aigpdk/gemsrl.v
read_verilog srl_bad.v
hierarchy -check -top srl_inv_clk
proc;;
techmap -map $GEM/aigpdk/gemsrl_map.v
EOF
out=$("$YOSYS" -q rej.ys 2>&1)
if [ $? -ne 0 ] && printf '%s' "$out" | grep -q "inverted clock"; then
  ok "srl_inv_clk rejected with the inverted-clock error"
else
  bad "srl_inv_clk should have raised a \$error"
fi

step "7. end-to-end - GEM naive_sim vs iverilog on the same netlist"
python3 mkstim.py 256 >/dev/null
iverilog -g2005 -o sim_e2e "$GEM/aigpdk/aigpdk.v" "$GEM/aigpdk/gemsrl.v" \
         out/ysrl_gate.v tb_e2e.v 2>/dev/null
./sim_e2e >/dev/null
"$TARGET/debug/naive_sim" out/ysrl.gv stim.vcd naive.vcd >/dev/null 2>&1
python3 cmpvcd.py naive.vcd iv_out.txt | tee /dev/stderr | grep -q "^PASS" \
  && ok "GEM == Icarus over 255 cycles" || bad "end-to-end differential"

step "8. CUDA cross-check (optional)"
if [ -x "$TARGET/release/cuda_test" ]; then
  echo "  [info] cuda_test present - run the GPU differential manually per usage.md"
else
  echo "  [skip] no CUDA build (cargo build --features cuda)"
fi

printf "\n"
if [ "$fails" -eq 0 ]; then
  echo "ALL PART C YOSYS CHECKS PASSED  (workdir: $WORK)"
else
  echo "$fails CHECK(S) FAILED  (workdir: $WORK)"
fi
exit "$fails"
