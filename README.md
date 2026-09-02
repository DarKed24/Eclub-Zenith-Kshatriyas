# The Big-GEM Theory — Pool Kshatriyas (Takneek 2026, PS Zenith)

Fork of [NVlabs/GEM](https://github.com/NVlabs/GEM) in which three word-level
Xilinx primitives — **DSP48E2** (the PS's constrained MAC subset), **CARRY4**
(with fused CO→CIN chains), and **SRLC32E** — are simulated as **native
functional units on the GPU ALU** (`int64` datapaths) instead of being
shredded into And-Inverter gates by the frontend.

Headline (RTX 5050, 20k cycles, all runs bit-exact vs 4 independent
references): **2.28×** end-to-end throughput on a 64-MAC DSP array and
**1.30×** on 16×64-bit CARRY4 adder banks over the shredded baseline.
Full analysis: `docs/report.pdf`.

## Requirements

* Linux or WSL2. Rust stable ≥ 1.85 (`rustup`), g++, python3.
* CUDA toolkit ≥ 12 (13.3 tested). Any sm_75+ NVIDIA GPU; the build embeds
  portable PTX by default — for a native build:
  `UCC_CUDA_GENCODE=120 UCC_CUDA_PTX=120` (RTX 50xx; 90=H100, 89=RTX 40xx).
* Yosys **0.68** with the slang plugin + Icarus — easiest via
  [oss-cad-suite](https://github.com/YosysHQ/oss-cad-suite-build) (we used
  build 2026-08-31).

## Build

```bash
git clone <this repo> gem && cd gem        # eda-infra-rs is vendored, no submodules
cargo build -r --features cuda             # cuda implies the partitioner
cargo build -q --bin naive_sim --bin level_test --bin macro_test   # debug tools for the test suites
```

CPU-only development (no GPU/CUDA needed): plain `cargo build -r` builds
everything except `cut_map_interactive`/`cuda_test`; `macro_test` runs the
whole differential suite against the CPU emulator.

## Verify (≈85 scripted checks)

```bash
cargo run -r --bin macro_test              # differential + structural suite
cd tests/yosys/parta && GEM=~/gem YOSYS=<yosys> OSSCAD=<osscad/bin> \
    CARGO_TARGET_DIR=~/gem/target ./run_all.sh   # likewise partb, partc
```

Each suite ends with `ALL ... CHECKS PASSED`: synthesis interception on real
Yosys 0.68, Icarus differentials, rejection guards, GEM-vs-Icarus e2e, and
(when a CUDA build exists) the GPU-vs-CPU differential.

## Simulate a SystemVerilog design

```bash
export GEM=~/gem YOSYS=<yosys-0.68-with-slang>
bench/flow/gem_synth.sh macro <top> out.gv design.sv     # native macros
bench/flow/gem_synth.sh shred <top> base.gv design.sv    # shredded baseline
target/release/cut_map_interactive out.gv out.gemparts
target/release/cuda_test out.gv out.gemparts stim.vcd result.vcd <NUM_BLOCKS> [--check-with-cpu]
```

`NUM_BLOCKS` = 2 × SM count. Stimulus VCDs: `bench/flow/gen_stim.py`.

## Benchmarks (produced every number in the report)

```bash
cd bench && NUM_BLOCKS=40 CHECK=1 NCU=1 CYCLES=20000 \
  flow/bench_run.sh designs/mac_array_64.sv designs/mixed_16.sv ...
```

Synthesizes macro+shred, partitions, times on GPU, profiles with Nsight
Compute, appends `results/bench.csv`. `UPSTREAM=<pristine cuda_test>` adds
unmodified-GEM baseline rows. Design generator: `flow/gen_bench.py`.

## Layout

```
src/hwmacro.rs        macro substrate: classes, packing, CPU golden evals
src/staging.rs        Class C forced-stage-split scheduling (fixpoint)
src/aig.rs,flatten.rs,pe.rs   graph, script builder, partition budgets
src/bin/              cut_map_interactive, cuda_test, naive_sim, macro_test, level_test
csrc/                 CUDA kernel + gem_eval_* device twins
aigpdk/               liberty libs, techmaps (gemmacro/gemcarry/gemsrl), behavioural models
tests/                fixtures + parta/partb/partc verification suites
bench/                benchmark kit: flow/, designs/, results/ (CSV + Nsight logs)
docs/                 technical report (LaTeX + PDF)
eda-infra-rs/         vendored infra (carries the CUDA-13 C++17 fix)
```

## Key upstream deltas

Macro substrate + Class R/C scheduling; slang-based SV2012 ingestion with
OPMODE folding and synthesis-time rejection guards; heterogeneous memory
layout; parallel Class C kernel epilogue (Nsight-driven, +42–72%);
partitioner `--no-merge`/`--division`; portable CUDA arch defaults in
`build.rs`; vendored `eda-infra-rs` with `-std=c++17` (CUDA 13).
