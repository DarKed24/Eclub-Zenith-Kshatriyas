#!/usr/bin/env bash
# Runs every Part A Yosys verification step from `.claude/yosysA.md` in order.
#
#   GEM=/path/to/GEM  YOSYS=~/yosys/build/yosys  ./run_all.sh [workdir]
#
# Defaults assume the WSL layout described in yosysA.md. Exits non-zero on the
# first step that does not produce the expected result.
set -uo pipefail

GEM="${GEM:-/mnt/c/Users/Devansh/GEM}"
YOSYS="${YOSYS:-$HOME/yosys/build/yosys}"
OSSCAD="${OSSCAD:-$HOME/oss-cad-suite/bin}"
TARGET="${CARGO_TARGET_DIR:-$HOME/gem-target}"
SRC="$GEM/tests/yosys/parta"
WORK="${1:-$HOME/gemA-run}"

export PATH="$OSSCAD:$PATH"
export CARGO_TARGET_DIR="$TARGET"

fails=0
step() { printf "\n=== %s ===\n" "$1"; }
ok()   { printf "  [ok]   %s\n" "$1"; }
bad()  { printf "  [FAIL] %s\n" "$1"; fails=$((fails + 1)); }

rm -rf "$WORK"; mkdir -p "$WORK/out" "$WORK/log"; cd "$WORK"
cp "$SRC"/*.v "$SRC"/*.py .

step "0. environment"
"$YOSYS" -V || { echo "no yosys at $YOSYS"; exit 1; }
command -v iverilog >/dev/null && iverilog -V | head -1

step "1. CPU substrate (cargo)"
( cd "$GEM" && cargo test -q >/dev/null 2>&1 ) && ok "cargo test" || bad "cargo test"
( cd "$GEM" && cargo run -q --bin macro_test 2>&1 | grep -q "all differential checks passed" ) \
  && ok "macro_test differential" || bad "macro_test differential"

step "2. Path A1 - hand-instantiated GEM_DSP48E2 -> aigpdk netlist"
cat > a1.ys <<EOF
read_verilog $GEM/aigpdk/gemmacro.v
read_verilog mac_top.v
hierarchy -check -top mac_top
synth -flatten
delete t:\$print
dfflibmap -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge
abc -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge
techmap
abc -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge
stat
write_verilog -noattr out/mac_top.gv
EOF
"$YOSYS" -q -l log/a1.log a1.ys >/dev/null 2>&1
grep -q "1   GEM_DSP48E2" log/a1.log && ok "GEM_DSP48E2 survives synthesis" \
                                     || bad "GEM_DSP48E2 missing from stat"
grep -q "16   DFF" log/a1.log && ok "16 DFF + aigpdk glue mapped" || bad "DFF mapping"

step "3. Path A1 - RTL vs gate netlist (iverilog)"
sed "s/^module mac_top(/module mac_top_gate(/" out/mac_top.gv > out/mac_top_gate.v
iverilog -g2005 -o sim_a1 "$GEM/aigpdk/aigpdk.v" "$GEM/aigpdk/gemmacro_behav.v" \
         mac_top.v out/mac_top_gate.v tb_a1.v 2>/dev/null
./sim_a1 | grep -q "^PASS" && ok "512 cycles bit-exact" || bad "RTL vs gate netlist"

step "4. Path A2 - DSP48E2 -> GEM_DSP48E2 techmap"
for pair in "dsp_static:2'h2:dsp_static.v" \
            "dsp_static_mult:2'h1:dsp_static.v" \
            "dsp_dyn:_:dsp_dyn.v"; do
  top="${pair%%:*}"; rest="${pair#*:}"; want="${rest%%:*}"; src="${rest#*:}"
  cat > "a2_$top.ys" <<EOF
read_verilog -lib +/xilinx/cells_xtra.v
read_verilog $GEM/aigpdk/gemmacro.v
read_verilog $src
hierarchy -check -top $top
proc;;
opt_expr; opt_clean
techmap -map $GEM/aigpdk/gemmacro_map.v
opt_expr -fine; opt_clean
stat
write_verilog -noattr out/${top}_mapped.v
EOF
  "$YOSYS" -q -l "log/a2_$top.log" "a2_$top.ys" >/dev/null 2>&1
  if grep -q "1   GEM_DSP48E2" "log/a2_$top.log"; then
    if [ "$want" = "_" ]; then
      grep -q "OPMODE_S(_" "out/${top}_mapped.v" \
        && ok "$top -> macro + gates driving OPMODE_S" \
        || bad "$top OPMODE_S should be dynamic"
    elif grep -q "OPMODE_S($want)" "out/${top}_mapped.v"; then
      ok "$top -> OPMODE_S folded to $want"
    else
      bad "$top OPMODE_S did not fold to $want"
    fi
  else
    bad "$top produced no GEM_DSP48E2"
  fi
done

step "5. Path A2 - techmapped arithmetic vs golden MAC (iverilog)"
iverilog -g2005 -o sim_opmode "$GEM/aigpdk/gemmacro_behav.v" \
         out/dsp_static_mapped.v tb_opmode.v 2>/dev/null
./sim_opmode | grep -q "^PASS" && ok "1000 random vectors" || bad "OPMODE semantics"

step "6. rejection guards"
for t in dsp_preg0:"PREG=0" dsp_areg1:"non-zero AREG"; do
  top="${t%%:*}"; msg="${t#*:}"
  cat > "rej_$top.ys" <<EOF
read_verilog -lib +/xilinx/cells_xtra.v
read_verilog $GEM/aigpdk/gemmacro.v
read_verilog dsp_dyn.v
hierarchy -check -top $top
proc;;
techmap -map $GEM/aigpdk/gemmacro_map.v
EOF
  out=$("$YOSYS" -q "rej_$top.ys" 2>&1)
  if [ $? -ne 0 ] && printf '%s' "$out" | grep -q "$msg"; then
    ok "$top rejected with the $msg error"
  else
    bad "$top should have raised a \$error"
  fi
done

step "7. Path B - RTL inference (expected NOT to fold on 0.68)"
cat > b_manual.ys <<EOF
read_verilog -lib +/xilinx/cells_xtra.v
read_verilog $GEM/aigpdk/gemmacro.v
read_verilog dsp_rtl.v
hierarchy -check -top dsp_rtl
proc;;
opt_expr; opt_dff; opt_clean
memory_dff
techmap -map +/mul2dsp.v -map +/xilinx/xcu_dsp_map.v -D DSP_A_MAXWIDTH=27 -D DSP_B_MAXWIDTH=18 -D DSP_A_MINWIDTH=2 -D DSP_B_MINWIDTH=2 -D DSP_NAME=\$__MUL27X18
select a:mul2dsp
setattr -unset mul2dsp
select -clear
opt_expr -fine
wreduce
xilinx_dsp -family xcup
chtype -set \$mul t:\$__soft_mul
opt_clean
stat
write_verilog -noattr out/dsp_rtl_manual.v
EOF
"$YOSYS" -q -l log/b_manual.log b_manual.ys >/dev/null 2>&1
if grep -q "PREG(32'sd0)" out/dsp_rtl_manual.v; then
  ok "xilinx_dsp leaves PREG=0 (documented 0.68 limitation)"
else
  bad "PREG changed - re-read yosysA.md Step 7, this Yosys may now fold"
fi

step "8. end-to-end - GEM naive_sim vs iverilog on the same netlist"
python3 mkstim.py 256 >/dev/null
iverilog -g2005 -o sim_e2e "$GEM/aigpdk/aigpdk.v" "$GEM/aigpdk/gemmacro_behav.v" \
         out/mac_top_gate.v tb_e2e.v 2>/dev/null
./sim_e2e >/dev/null
"$TARGET/debug/naive_sim" out/mac_top.gv stim.vcd naive.vcd >/dev/null 2>&1
python3 cmpvcd.py naive.vcd iv_out.txt | tee /dev/stderr | grep -q "^PASS" \
  && ok "GEM == Icarus over 255 cycles" || bad "end-to-end differential"

printf "\n"
if [ "$fails" -eq 0 ]; then
  echo "ALL PART A YOSYS CHECKS PASSED  (workdir: $WORK)"
else
  echo "$fails CHECK(S) FAILED  (workdir: $WORK)"
fi
exit "$fails"
