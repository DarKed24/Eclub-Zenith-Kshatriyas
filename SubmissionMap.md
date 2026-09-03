# Submission Map

This document maps the project deliverables to their corresponding files and directories within the repository.

---

## (1) & (2) Presentation & Technical Report

* **Technical Report**: `docs/report.pdf`  
  *(Note: If building from source or reviewing the PDF, ensure `docs/report.pdf` is placed here prior to grading).*

---

## (3) Testbenches & Golden Models

### CPU Reference Models (Rust)
* `src/emulate.rs` — CPU bit-level/cycle-accurate emulator; direct behavioral twin of the GPU CUDA kernel (`simulate_block_v1`) used for CPU vs. GPU differential validation (`cuda_test --check-with-cpu`).
* `src/hwmacro.rs` — Golden word-level functional models for FPGA macros (`eval_dsp48e2`, `eval_carry4`, `eval_srl32_shift`, `eval_srl32_read`).
* `src/bin/naive_sim.rs` — Independent behavioral gate-level reference simulator.

### Verification Drivers & Test Suites
* `src/bin/macro_test.rs` — Primary differential validation suite comparing native macros against shredded behavioral baselines cycle-by-cycle.
* `src/bin/level_test.rs` — Topological leveling and partition scheduler validation.
* `src/bin/flatten_test.rs` — Netlist flattening and hierarchical boundary elaboration tests.
* `src/bin/repcut_test.rs` — Replication and cut-mapping partitioner unit tests.
* `src/bin/boomerang_test.rs` — End-to-end round-trip equivalence verification.
* `src/bin/cuda_dummy_test.rs` — GPU kernel integration test driver.

### Verilog Fixtures & Differential Testbenches
* `tests/yosys/parta/` — DSP48E2 test suite (`tb_a1.v`, `tb_opmode.v` with golden MAC, `tb_e2e.v`, `dsp_rtl.v`, `dsp_static.v`, `dsp_dyn.v`, `mac_top.v`).
* `tests/yosys/partb/` — CARRY4 / Adder test suite (`tb_b1.v`, `tb_carry8.v` with ripple/carry golden models, `tb_e2e.v`, `add_rtl.v`, `carry_top.v`).
* `tests/yosys/partc/` — SRLC32E / Shift register test suite (`tb_c1.v`, `tb_srl16.v`, `tb_e2e.v`, `srl_rtl.v`, `srl16.v`, `ysrl.v`, `srl_bad.v`).
* `tests/yosys/part*/run_all.sh` — Automated batch verification scripts (~85 verification checks).
* `tests/yosys/part*/mkstim.py` & `cmpvcd.py` — Dynamic stimulus generation and VCD waveform differential comparison scripts.
* `tests/data/*.gv` — 12 hand-crafted gate-level `.gv` structural fixtures (e.g., `ripple_adder.gv`, `mac_accumulator.gv`, `srl_cascade_q31.gv`).
* `aigpdk/gemmacro_behav.v`, `gemcarry.v`, `gemsrl.v` — Behavioral Verilog reference models used for shredded logic baseline comparisons.

---

## (4) Benchmark Automation & Performance Logs

### Automation Toolchain (`bench/flow/`)
* `bench/flow/bench_run.sh` — Master benchmark automation pipeline: drives Yosys macro/shred synthesis, performs cut partitioning, executes GPU simulations, and profiles execution using NVIDIA Nsight Compute (`ncu`).
* `bench/flow/gem_synth.sh` — Automated synthesis wrapper targeting shredded logic vs. macro-preserved netlists.
* `bench/flow/gen_bench.py` — Benchmark generator scaling design sweeps (`mac_array_N`, `carry_adders_KxW`, `srl_lines_N`, `mixed_N`).
* `bench/flow/gen_stim.py` — Random test vector and stimulus generator.
* `bench/flow/cmp_vcd.py` — Post-simulation VCD validation checker (`CHECK=1`).

### Performance Results & Logs (`bench/results/`)
* `bench/results/bench.csv` — Aggregated execution time, speedup, and GPU memory metrics across all benchmark sweeps.
* `bench/results/*.log.ncu` — Raw NVIDIA Nsight Compute kernel profiler logs and hardware performance counter outputs.
* `bench/results/*.log.run` & `*.log.cutmap` — Detailed GPU timing, partitioning statistics, and log outputs.

---

## (5) Simulator Implementation & Source Code

### Core Simulator (`src/`)
* `src/aig.rs` — And-Inverter Graph (AIG) internal representation and manipulation algorithms.
* `src/aigpdk.rs` — Macro detection, pattern extraction, and target cell mapping.
* `src/flatten.rs` — Module hierarchy elaboration and netlist flattener.
* `src/repcut.rs` — Multi-output cut enumeration and replication partitioning algorithm.
* `src/pe.rs` — Processing element mapping and block-level GPU thread scheduler.
* `src/staging.rs` — GPU host-device memory layout staging and buffer streaming.

### GPU Kernels (`csrc/`)
* `csrc/kernel_v1.cu` — Main CUDA execution entry points and driver interfaces.
* `csrc/kernel_v1_impl.cuh` — High-throughput parallel block evaluation engine.
* `csrc/macro_eval.cuh` — CUDA device implementations for DSP48E2, CARRY4, and SRLC32E native macro evaluation.

### Supporting Infrastructure & Dependencies
* `eda-infra-rs/` — Vendored EDA utilities (VCD parser, Verilog parser, NetlistDB, profiling timers).
* `aigpdk/` — Standard cell libraries, synthesis techmaps, and behavioral libraries.
* `build.rs`, `Cargo.toml` — Build system configuration and CUDA compilation hooks.