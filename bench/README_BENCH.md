# Zenith benchmark kit (drop into ~/gem/bench)

    ~/gem/bench/flow/        gem_synth.sh   canonical slang-based synthesis (macro | shred)
                             gen_bench.py   SystemVerilog benchmark generator (4 families x sizes)
                             gen_stim.py    generic random-stimulus VCD generator
                             cmp_vcd.py     output-VCD comparator (macro vs shred, GPU vs CPU)
                             bench_run.sh   synth -> partition -> GPU timing -> Nsight -> CSV
                             dsp48e2_bb.v   blackbox DSP48E2 for the slang frontend
                             gem*_shred.v   behavioural CARRY4 / SRLC32E for the baseline
    ~/gem/bench/designs/     *.sv *.ports   generated benchmarks (+ pre-synthesized 4-lane .gv pairs)

## One-time
    cd ~/gem && tar xf /mnt/c/Users/kamal/Downloads/bench_kit.tgz     # creates bench/
    export GEM=~/gem YOSYS=~/oss-cad-suite/bin/yosys NUM_BLOCKS=40

## Step 1 - GPU correctness on the pre-synthesized small designs (fast)
    cd ~/gem/bench && mkdir -p results && cp designs/*.gv designs/*.log results/
    CHECK=1 CYCLES=2000 flow/bench_run.sh designs/mac_array_4.sv designs/carry_adders_4x32.sv \
                                          designs/srl_lines_8.sv designs/mixed_4.sv
Every row must show `GPU==CPU check: PASS` for BOTH macro and shred.

## Step 2 - scale (first multi-block runs) + Nsight
    CHECK=1 NCU=1 CYCLES=20000 flow/bench_run.sh designs/mac_array_16.sv designs/mac_array_64.sv \
        designs/carry_adders_16x64.sv designs/carry_adders_64x64.sv designs/srl_lines_32.sv \
        designs/srl_lines_128.sv designs/mixed_16.sv designs/mixed_64.sv
Shredding the 64-lane designs takes a while (multi-core helps: yosys/abc is single-threaded
per design, so run two bench_run.sh invocations in parallel on different designs).
If Nsight fails with ERR_NVGPUCTRPERM, enable "Manage GPU Performance Counters -> all users"
in the NVIDIA Control Panel (Windows) and reboot.

## Step 3 - the honest baseline (pristine upstream GEM on the shred netlists)
    cd ~ && git clone --recursive https://github.com/NVlabs/GEM.git gem-upstream && cd gem-upstream
    sed -i '84s/c++14/c++17/' eda-infra-rs/ucc/src/compile.rs
    cargo build -r -j 2 --features cuda
    UPSTREAM=~/gem-upstream/target/release/cuda_test ... flow/bench_run.sh <same designs>
adds an `upstream` row per design (unmodified GEM simulating the shredded netlist).

## Verified in the sandbox (Yosys 0.68, CPU reference sim, 300 cycles)
    design             macro cells / depth   shred cells / depth   macro == shred
    srl_lines_8               82 / 9              1921 / 17        PASS (2 bits)
    carry_adders_4x32       1036 / 5              1629 / 68        PASS (33 bits)
    mac_array_4              616 / 4             20622 / 110       PASS (48 bits)
    mixed_4                 1812 / 5             23446 / 110       PASS (81 bits)

## Flow gotchas discovered (put these in the report)
* slang (sv-elab) needs real definitions: a DSP48E2 declared without (* blackbox *) is
  flattened AWAY silently -> dsp48e2_bb.v.
* Unpacked arrays written in always_ff infer memories; memory_libmap maps them to
  $__RAMGEM_ASYNC_ which GEM cannot simulate -> write pipelines as packed vectors.
* GEM's netlist reader rejects `signed` and escaped hierarchical wire names
  (\a.b[0].c) -> gem_synth.sh hides internal names (rename -hide) and strips `signed`.
* SRLC32E instances carry INIT/IS_CLK_INVERTED params after synth_xilinx/cells_sim ->
  setparam -unset before write_verilog.
