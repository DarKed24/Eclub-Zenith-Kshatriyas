#!/usr/bin/env bash
# Zenith benchmark automation: synthesize (macro + shred), partition, time on
# the GPU, profile with Nsight Compute, and append one CSV row per run.
#
#   GEM=~/gem YOSYS=~/oss-cad-suite/bin/yosys NUM_BLOCKS=40 CYCLES=20000 \
#     ./bench_run.sh designs/mac_array_16.sv [more .sv ...]
#
# Env knobs:
#   NUM_BLOCKS  CUDA blocks for cuda_test (2 x SM count; RTX 5050 = 40)
#   CYCLES      simulated cycles for the throughput run (default 20000)
#   CHECK=1     also run --check-with-cpu on a short 300-cycle stimulus first
#   NCU=1       run Nsight Compute on the macro AND shred kernels
#   MODES       "macro shred" (default) or a subset
#   CUTMAP_ARGS extra flags for cut_map_interactive, e.g. "--no-merge --division 128"
#   TAG         label appended to the mode column in the CSV (e.g. TAG=nomerge)
#   UPSTREAM    path to a PRISTINE upstream-GEM cuda_test binary; if set, the
#               shred netlist is ALSO timed with it (the unmodified-GEM baseline)
#
# Output: results/<design>.<mode>.{gv,gemparts,log} and results/bench.csv with
#   design,mode,cells,dff,macros,levels,blocks,cycles,sim_ms,cycles_per_sec,
#   check,ncu_dram_pct,ncu_l1_pct,ncu_threads_per_inst,ncu_branch_uniform_pct,ncu_warps_active_pct
set -uo pipefail
GEM="${GEM:?set GEM=/path/to/repo}"
YOSYS="${YOSYS:-yosys}"
FLOW="$(cd "$(dirname "$0")" && pwd)"
REL="$GEM/target/release"
NUM_BLOCKS="${NUM_BLOCKS:-40}"
CYCLES="${CYCLES:-20000}"
MODES="${MODES:-macro shred}"
RES="${RES:-results}"; mkdir -p "$RES"
CSV="$RES/bench.csv"
[ -f "$CSV" ] || echo "design,mode,cells,dff,macros,levels,blocks,cycles,sim_ms,cycles_per_sec,check,ncu_dram_pct,ncu_l1_pct,ncu_threads_per_inst,ncu_branch_uniform_pct,ncu_warps_active_pct" > "$CSV"
KERNEL=simulate_v1_noninteractive_simple_scan
NCU_METRICS="dram__throughput.avg.pct_of_peak_sustained_elapsed,l1tex__throughput.avg.pct_of_peak_sustained_elapsed,smsp__thread_inst_executed_per_inst_executed.ratio,smsp__sass_average_branch_targets_threads_uniform.pct,sm__warps_active.avg.pct_of_peak_sustained_active"

export GEM YOSYS

stat_of() {  # log top -> "cells dff macros" (from the LAST stat block of `top`)
  awk -v T="=== $2 ===" '
    $0==T {c=0; d=0; m=0; f=1; next}
    f && /^[ \t]*[0-9]+[ \t]+cells$/ {c=$1}
    f && /^[ \t]*[0-9]+[ \t]+DFF$/ {d=$1}
    f && /^[ \t]*[0-9]+[ \t]+(GEM_DSP48E2|CARRY4|SRLC32E)$/ {m+=$1}
    END {printf "%d %d %d", c, d+0, m+0}' "$1"
}

for SV in "$@"; do
  top="$(basename "$SV" .sv)"
  ports="$(cat "$(dirname "$SV")/$top.ports")"
  echo "################ $top"
  for mode in $MODES; do
    gv="$RES/$top.$mode.gv"; parts="$RES/$top.$mode.gemparts"; log="$RES/$top.$mode.log"
    echo "=== [$top/$mode] synth"
    if [ ! -f "$gv" ]; then
      "$FLOW/gem_synth.sh" "$mode" "$top" "$gv" "$SV" > "$log.synth" 2>&1 || { echo "  synth FAILED (see $log.synth)"; continue; }
    fi
    read -r cells dff macros <<< "$(stat_of "${gv%.gv}.$mode.log" "$top")"
    levels="$("$REL/level_test" "$gv" 2>&1 | grep -oE 'levels: [0-9]+' | grep -oE '[0-9]+')"
    echo "  cells=$cells dff=$dff macros=$macros levels=$levels"

    echo "=== [$top/$mode] partition (NUM_BLOCKS=$NUM_BLOCKS)"
    "$REL/cut_map_interactive" ${CUTMAP_ARGS:-} "$gv" "$parts" > "$log.cutmap" 2>&1 || { echo "  cut_map FAILED (see $log.cutmap)"; continue; }
    # true part count = sum over major stages of what cut_map produced
    # (works for both the merge pass and --no-merge)
    nparts="$(grep -oE '(after merging: |keeping the )[0-9]+ (initial )?parts' "$log.cutmap" | grep -oE '[0-9]+' | paste -sd+ | bc 2>/dev/null)"

    check="-"
    if [ "${CHECK:-0}" = "1" ]; then
      python3 "$FLOW/gen_stim.py" "$top" 300 "$RES/$top.check.vcd" $ports >/dev/null
      "$REL/cuda_test" "$gv" "$parts" "$RES/$top.check.vcd" "$RES/$top.$mode.check_out.vcd" "$NUM_BLOCKS" --check-with-cpu > "$log.check" 2>&1
      grep -q "sanity test passed" "$log.check" && check=PASS || check=FAIL
      echo "  GPU==CPU check: $check"
    fi

    echo "=== [$top/$mode] throughput ($CYCLES cycles)"
    [ -f "$RES/$top.stim.vcd" ] || python3 "$FLOW/gen_stim.py" "$top" "$CYCLES" "$RES/$top.stim.vcd" $ports >/dev/null
    "$REL/cuda_test" "$gv" "$parts" "$RES/$top.stim.vcd" "$RES/$top.$mode.out.vcd" "$NUM_BLOCKS" > "$log.run" 2>&1
    sim_ms="$(grep -oE 'simulation, Elapsed=[0-9.]+(ms|s|µs)' "$log.run" | tail -1 | sed -E 's/.*Elapsed=//')"
    ncyc="$(grep -oE 'total number of cycles: [0-9]+' "$log.run" | grep -oE '[0-9]+' | tail -1)"
    # normalise to ms
    case "$sim_ms" in
      *µs) ms=$(python3 -c "print(float('${sim_ms%µs}')/1000)");;
      *ms) ms="${sim_ms%ms}";;
      *s)  ms=$(python3 -c "print(float('${sim_ms%s}')*1000)");;
      *)   ms="";;
    esac
    cps="$( [ -n "$ms" ] && python3 -c "print(int($ncyc/($ms/1000)))" )"
    stages="$(grep -oE 'effective partitions in each stage: \[[0-9, ]+\]' "$log.run" | tail -1 | grep -oE '\[.*\]')"
    echo "  parts=$nparts stages=$stages cycles=$ncyc sim=${ms}ms -> $cps cycles/s"

    dram="-"; l1="-"; tpi="-"; bru="-"; wa="-"
    if [ "${NCU:-0}" = "1" ]; then
      echo "=== [$top/$mode] nsight compute"
      ncu --csv --metrics "$NCU_METRICS" -k "$KERNEL" --launch-count 1 \
          "$REL/cuda_test" "$gv" "$parts" "$RES/$top.stim.vcd" "$RES/$top.$mode.ncu_out.vcd" "$NUM_BLOCKS" > "$log.ncu" 2>&1
      getm() { grep -F "$1" "$log.ncu" | tail -1 | awk -F'","' '{print $NF}' | tr -d '"\r'; }
      dram="$(getm dram__throughput)"; l1="$(getm l1tex__throughput)"
      tpi="$(getm thread_inst_executed_per_inst)"; bru="$(getm branch_targets_threads_uniform)"
      wa="$(getm sm__warps_active)"
      echo "  dram=$dram% l1=$l1% threads/inst=$tpi branch-uniform=$bru% warps-active=$wa%"
    fi
    echo "$top,$mode${TAG:+-$TAG},$cells,$dff,$macros,$levels,$nparts,$ncyc,$ms,$cps,$check,$dram,$l1,$tpi,$bru,$wa" >> "$CSV"

    if [ "$mode" = "shred" ] && [ -n "${UPSTREAM:-}" ] && [ -x "$UPSTREAM" ]; then
      echo "=== [$top/upstream] pristine GEM on the shred netlist"
      "$(dirname "$UPSTREAM")/cut_map_interactive" "$gv" "$RES/$top.upstream.gemparts" > "$log.up.cutmap" 2>&1
      "$UPSTREAM" "$gv" "$RES/$top.upstream.gemparts" "$RES/$top.stim.vcd" "$RES/$top.upstream.out.vcd" "$NUM_BLOCKS" > "$log.up.run" 2>&1
      ums="$(grep -oE 'simulation, Elapsed=[0-9.]+ms' "$log.up.run" | tail -1 | grep -oE '[0-9.]+')"
      ucps="$( [ -n "$ums" ] && python3 -c "print(int($ncyc/($ums/1000)))" )"
      echo "  upstream sim=${ums}ms -> $ucps cycles/s"
      echo "$top,upstream,$cells,$dff,0,$levels,,$ncyc,$ums,$ucps,-,-,-,-,-,-" >> "$CSV"
    fi
  done
done
echo; echo "CSV: $CSV"; column -s, -t "$CSV" 2>/dev/null || cat "$CSV"
