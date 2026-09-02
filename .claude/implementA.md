# Part A implementation log — Native DSP48E2 MAC macro in GEM

This file tracks the implementation of `.claude/planA.md` (native DSP48E2 MAC
macro substrate). It is a running log: what was done, decisions taken, and what
remains unverified.

## Environment notes

- Windows 11, **no Rust or CUDA toolchain** at the start. Installed: `rustup`
  (`stable-x86_64-pc-windows-msvc`), **VS 2022 Build Tools** (VCTools + Win11
  SDK), and **LLVM 22** (used during the mt-kahypar investigation, not needed by
  the final build). No CUDA / `nvcc`, no Yosys on the Windows side.
- `eda-infra-rs/` submodule was initialised (`git submodule update --init
  --recursive`) — it was empty at the start, which blocked every build.
- `cargo build` / `cargo test` (no features) is the CPU verification path and
  is **green** on both Windows (MSVC) and Linux.

### Linux verification env (WSL Ubuntu-24.04, added later)

The machine has **WSL2 Ubuntu-24.04** with a real toolchain, so the parts the
Windows box could not check were done here:

- **Rust** 1.98, native `x86_64-unknown-linux-gnu`.
- **Yosys 0.68** — built from the source checkout at `~/yosys`
  (`git sha1 38e001a6f`, CMake build, binary at `~/yosys/build/yosys`). This
  is the pinned version for this project and the one every frontend claim below
  refers to. Note that `/usr/bin/yosys` on this box is the Ubuntu package
  (**0.33**) and is *not* the target — always invoke `~/yosys/build/yosys`
  explicitly. (An earlier pass of this work validated against 0.33; the whole
  frontend section was re-run against 0.68 and is recorded below.)
- **CUDA 12.6** `nvcc` — no root, so installed by `dpkg-deb -x`-extracting the
  NVIDIA `cuda-nvcc-12-6` / `cuda-nvvm` / `cuda-crt` / `cuda-cudart-dev` /
  `cuda-cccl` `.deb`s into `~/cuda` and pointing `CUDA_HOME` / `PATH` /
  `CUDA_LIBRARY_PATH` at them (`~/cudaenv.sh`). `libcuda.so` comes from the WSL
  driver at `/usr/lib/wsl/lib`. A trivial kernel compiles and runs on the
  on-board **RTX 2050** (`sm_86`).
- `libclang-18` for `ucc::bindgen`.
- The repo is used in place at `/mnt/c/Users/Devansh/GEM`; builds go to a
  Linux-local `CARGO_TARGET_DIR=~/gem-target` so they do not fight the Windows
  `target/`. (The many "modified" tracked files under `git status` on the Linux
  side are pure CRLF-vs-LF working-tree noise from git-for-windows `autocrlf` —
  the real diff, `--ignore-all-space`, is exactly the Part A change set.)

## Status legend

- ✅ done + compiles (CPU-only `cargo build`)
- 🧪 done + covered by a CPU test
- ✍️  written but not compiled/verified in this environment (CUDA / Yosys)
- ⏳ in progress
- ⬜ not started

## Work items

| # | Item | Status |
|---|------|--------|
| 0 | submodule init + toolchain | ✅ |
| 1 | `src/hwmacro.rs` substrate + unit tests | 🧪 9 unit tests pass |
| 2 | `src/aig.rs` — `DriverType::Macro`, `EndpointGroup::Macro`, parse | 🧪 covered by macro_test |
| 3 | `src/aigpdk.rs` — `GEM_DSP48E2` leaf pins | 🧪 covered by macro_test |
| 4 | `src/pe.rs` — partition resource accounting | 🧪 covered by macro_test |
| 5 | `src/flatten.rs` — heterogeneous allocator + script ABI | 🧪 covered by macro_test |
| 6 | `src/emulate.rs` — lift `simulate_block_v1` out of `cuda_test`, add macro path | 🧪 covered by macro_test |
| 7 | `csrc/macro_eval.cuh` + `csrc/kernel_v1_impl.cuh` | ✅ compiles clean under `nvcc` 12.6 (`sm_86` + default arch set), 0 warnings; GPU-vs-CPU numeric diff still pending |
| 8 | Yosys frontend (`aigpdk/gemmacro*.v`, liberty, `usage.md`) | ✅ validated against Yosys 0.68 — **2 bugs found + fixed** (see below) |
| 9 | `src/bin/naive_sim.rs` — `GEM_DSP48E2` golden arm | ✅ compiles + smoke-run on `mac_accumulator.gv` (no panic; `P = A*B` = 14 for 7×2 mult-only observed) |
| V | `src/bin/macro_test.rs` + `tests/data/*.gv` differential harness | 🧪 both cases pass |

### Test results (`cargo test`, MSVC toolchain, no features)

```
hwmacro::tests            9 passed   (eval_dsp48e2 vs independent reference:
                                      all 3 OPMODE states, USE_D, RSTP, signed
                                      extremes, 27-bit pre-adder wrap, 48-bit
                                      accumulator wrap, 124-bit packing round-trip)
macro_test  simple_dff    256 cycles, 0 macros   -> differential PASS
            mac_accumulator 512 cycles, 1 macro  -> differential PASS
```

`macro_test` runs the full compile pipeline (`AIG::from_netlistdb` ->
`build_staged_aigs` -> `Partition::build_one` -> `FlattenedScriptV1::from`) then
drives random vectors through `emulate::simulate_block_v1` **and** an
independent behavioural gate-level evaluator, asserting bit-exact agreement on
every primary output and every `P` bit every cycle. `mac_accumulator` has one
`GEM_DSP48E2` doing `p <= a*b` / `p <= p + a*b` (state chosen combinationally
from `load`), `RSTP=rst`, plus AND2/INV/DFF glue and comb outputs reading `P`.

### Regression: macro-free script ABI is byte-identical (plan verification #3)

A `git worktree` at pre-Part-A `HEAD` was built (with only the orthogonal
`ucc`/`partitioner` build fixes applied) and its `FlattenedScriptV1` for
`simple_dff.gv` hashed:

```
HEAD (pre-Part-A):  blocks_data hash 4367312310778971198, reg_io_state_size 130
post-Part-A:        blocks_data hash 4367312310778971198, reg_io_state_size 130
```

Byte-identical. Every new code path in `flatten.rs` / `pe.rs` is gated on
`num_macros > 0` and every new metadata word is emitted only when
`num_macros > 0`, so a macro-free design's script is provably unchanged; this
confirms it empirically. `macro_test` pins the hash as a standing guard
(`SIMPLE_DFF_HASH`).

### Toolchain status (item 0)

`mt-kahypar` 0.2.0 (the hypergraph partitioner) **does not build on Windows** on
any toolchain tried — its vendored oneTBB / kahypar-shared-resources have
several Windows portability bugs:
- mingw-w64 ucrt g++: TBB `#include <winbase.h>` without `windows.h`
  (`minwinbase.h` sees `DWORD` undefined).
- MSVC `cl.exe`: the crate's `build.rs` passes GNU-style `-Wno-unused-function`,
  which `cc` mistranslates to `/Wno-unused-function` → `cl : D8021`.
- clang-cl: gets further (with `/FIwindows.h -DNOMINMAX /EHsc`) but then
  `kahypar-shared-resources`'s `CAtomic` picks its non-`std::atomic` fallback
  (clang-cl doesn't define `__GNUC__`) → "no member named 'load'".

Two toolchains were installed along the way: **MSVC Build Tools 2022** (VCTools
+ Win11 SDK) and **LLVM 22 / clang-cl**. The MSVC toolchain is what the CPU-only
build now uses.

**Resolution (small, well-motivated change):** `mt-kahypar` is made an
**optional dependency** behind a new `partitioner` feature
([Cargo.toml](Cargo.toml)). It is only used by `RCHyperGraph::partition`
(`src/repcut.rs`), itself only used by `cut_map_interactive`, which now carries
`required-features = ["partitioner"]`. `cuda = ["ulib/cuda", "partitioner"]`, so
a Linux/CUDA build is byte-for-byte the same as before. The CPU verification
path — `cargo build`, `macro_test`, `naive_sim`, `level_test`, `boomerang_test`,
`flatten_test`, `repcut_test` — needs no C++ partitioner.

`eda-infra-rs/ucc/src/compile.rs::cl_cpp_openmp()` also gained a Windows branch:
MSVC/clang-cl has no `libgomp`, so on Windows the (non-perf-critical) `#pragma
omp` loops in `ulib`'s `memfill.cpp` compile serially. This is a submodule edit
— local to this checkout.

### CUDA (item 7) — compiles, GPU numeric diff pending

`csrc/kernel_v1.cu` (which pulls in `kernel_v1_impl.cuh` -> `macro_eval.cuh`)
**compiles clean under `nvcc` 12.6** with the exact `build.rs` flag set
(`-std=c++14 -Xcompiler -Wall -O3 -lineinfo -maxrregcount=128`), for both the
RTX 2050's native `sm_86` and the `cl_cuda()` default arch set (`sm_80`,
`sm_70`, PTX `compute_50`). `ptxas -v`: 94 registers, **0 spill**, 1 barrier,
3328 B smem — healthy. So the macro datapath (`gem_eval_dsp48e2`), the
uniform-scope 4-lane `__shfl_sync` gather, the 64-bit `P` feedback load and the
predicated macro-commit block are all valid CUDA that nvcc accepts and codegens.

Still to do (needs the full `cargo build --features cuda` to finish — the
`mt-kahypar` C++ build is very heavy on a 7 GB WSL VM):
- `cuda_test --check-with-cpu` on `tests/data/mac_accumulator.gv` — GPU vs
  `emulate::simulate_block_v1`.
- `compute-sanitizer` over the `__shfl_sync` gather + 64-bit `P` load.

**One integration risk to confirm with the GPU diff:** `emulate.rs` writes the
macro `P` words into `writeouts[]` *after* the `writeout_inv ^= c3`
data-inversion pass, whereas the kernel writes them into `shared_writeouts`
*before* the equivalent line — so they agree only if the flattener emits
`c3 == 0` (no data inversion) for the macro `P` bit positions. It should
(`place_clken_datainv(..., 0)`), but that is exactly what a GPU-vs-CPU diff
would catch.

### Yosys frontend (item 8) — validated against Yosys 0.68, 2 bugs fixed

Checked with **Yosys 0.68** (`git sha1 38e001a6f`, built from `~/yosys`).
Two real bugs were found and fixed:

1. **`gemmacro_map.v` was unusable.** Its register-control parameters defaulted
   to `1` (`AREG = 1`, `ADREG = 1`, ...). Yosys elaborates a techmap map module
   once with default parameters *before* matching any instance, so the
   `generate if (AREG != 0 ...) $error(...)` fired immediately on every
   `techmap -map` — no DSP could ever be mapped. Fixed: the register-param
   defaults are now the *accepted* configuration (`PREG = 1`, everything else
   `0`); techmap re-elaborates with the real instance parameters and the
   `$error`s then fire only for a genuinely unsupported cell. Also brought the
   port/param list in line with `cells_xtra.v` (`XOROUT [7:0]`,
   `CARRYCASCIN`, `MULTSIGNIN`, `PREADDINSEL`, the `IS_*_INVERTED` set) — that
   list is **byte-for-byte the same in 0.68 as it was in 0.33**, so the map
   file needed no port/param change when the version was bumped.

   Validated on 0.68:

   | case | result |
   |---|---|
   | `PREG=1` `DSP48E2`, `OPMODE = 9'h025` | one `GEM_DSP48E2`, `OPMODE_S` folded to constant `2'h2` |
   | `PREG=1` `DSP48E2`, `OPMODE = 9'h005` | one `GEM_DSP48E2`, `OPMODE_S` folded to constant `2'h1` |
   | data-dependent `OPMODE` (`load ? 005 : 025`) | one `GEM_DSP48E2` + ~7 gates driving `OPMODE_S` |
   | `PREG=0` | `ERROR: GEM: DSP48E2 has PREG=0 - GEM only maps a *registered* MAC.` |
   | `AREG=1` | `ERROR: GEM: DSP48E2 has a non-zero AREG/BREG/CREG/DREG/ADREG/MREG.` |

2. **`usage.md` Step 1.5 recipe was wrong.** (a) `+/xilinx/xcup_dsp_map.v` does
   not exist — in 0.68 as in 0.33 the UltraScale+ file is
   `+/xilinx/xcu_dsp_map.v` (`share/xilinx/` holds only `xc3sda`, `xc4v`,
   `xc5v`, `xc6s`, `xc7` and `xcu`).
   (b) The recipe ran `alumacc` before `mul2dsp`, which folds `a*b + acc` into a
   `$macc` cell that `mul2dsp` cannot see. (c) Most important: **0.68's
   `xilinx_dsp` still does not fold a `P <= P + A*B` accumulator into `PREG` +
   the post-adder** — it emits `DSP48E2` with `PREG=0` and leaves the adder and
   accumulator flop in fabric, which `gemmacro_map.v` then (correctly) rejects.

   Re-verified on 0.68 over a wider matrix than the 0.33 pass, and *nothing*
   folds:

   | RTL form | manual `map_dsp` sequence | `synth_xilinx -family xcup / xcu / xc7` |
   |---|---|---|
   | `p <= p + a*b` (accumulator) | `DSP48E2` `PREG=0` + `$add` + flop in fabric | `PREG=0` (`DSP48E2` for xcup/xcu, 2× `DSP48E1` for xc7) |
   | `p <= c + a*b` (C-feedback MACC) | `PREG=0` | `PREG=0` |
   | `p <= a*b` (plain registered multiply) | `PREG=0` | `PREG=0` + 45 `FDRE` in fabric |

   `xilinx_dsp` runs and packs nothing (the pass logs no matches). So the
   "inferred from ordinary RTL arithmetic" story does **not** hold with stock
   Yosys 0.68 either — this is the one Part A conclusion that the version bump
   did not improve.

   `usage.md` Step 1.5 was rewritten into **Path A** (instantiate `GEM_DSP48E2`
   directly, or a `PREG=1` `DSP48E2` + `techmap -map gemmacro_map.v` — both
   validated) and **Path B** (RTL inference — kept, but flagged as needing a
   Yosys whose `xilinx_dsp` actually folds the accumulator).

`aigpdk/gemmacro.v` (behavioural model / techmap target) and the `aigpdk.lib`
`GEM_DSP48E2` entry were not exercised further — `gemmacro.v` parses and
elaborates fine as the `_TECHMAP_REPLACE_` target in the validated runs.

## Decisions / deviations from the plan

- `MacroBlock` lives in `src/aig.rs` next to `RAMBlock` (it holds aigpin
  indices — an AIG concept), not in `src/hwmacro.rs`. `hwmacro.rs` stays the
  single source of truth for *shape/packing/semantics* (`MacroKind`,
  `input_bit_slot`, `eval_dsp48e2`); `MacroBlock` just references it.
- `MacroBlock` is **not** `serde`-derived. The plan said "serde, like RAMBlock",
  but `RAMBlock`/`DFF` are not serialized either — only `Partition`/`BoomerangStage`
  are, and the `AIG` (hence `macros`) is rebuilt from the netlist on every run.
- The `+1` alignment-hole reservation in `pe.rs` / the even-rounding of
  `state_start` and `reg_io_state_size` in `flatten.rs` are all made
  **conditional on `num_macros > 0`** so a macro-free design's script bytes (and
  the `blocks_data` hash) are bit-identical to before the change (verified).
- Missing CEP defaults to 0 (never-commit unless RSTP), matching how `DFF`/SRAM
  treat a missing clock enable. The techmap always wires CEP, and the test `.gv`
  ties it to `1'b1`.
- The two *optional* item-4 tweaks — weighting macro endpoints in `repcut.rs`
  and adding macro cost to `executor_fixed_cost` in `flatten.rs` — were **not
  done**. `RCHyperGraph`'s existing weight formula already sums over a macro's
  124 inputs, so macros are naturally heavy; and load-balance tuning has no
  correctness impact and can't be benchmarked here.
- Two hand-written test `.gv` headers avoid empty `//` lines — `sverilogparse`'s
  comment skipper needs ≥1 char after `//` (an empty `//` line aborts the
  parse). Pre-existing `sverilogparse` quirk, not touched.

## Files changed

New:
- `src/hwmacro.rs` — substrate: `MacroClass`, `MacroKind::{class,num_input_bits,
  num_output_bits,num_perm_words,num_state_words,input_bit_slot}`, the canonical
  layout consts, `eval_dsp48e2`, `dsp48e2_pack`, unit tests.
- `src/emulate.rs` — `simulate_block_v1` moved verbatim out of `cuda_test`, then
  the macro path added (reads metadata `[8..12]`, sizes the sram/dup permute for
  `+num_macros*4` lanes, evaluates `eval_dsp48e2` from the 4 gathered lanes +
  the previous cycle's `P`, writes the two `P` words into the macro write-out
  section before the final gated commit).
- `src/bin/macro_test.rs` + `tests/data/{simple_dff,mac_accumulator}.gv` — the
  differential harness.
- `csrc/macro_eval.cuh` — `gem_sext`, `gem_eval_dsp48e2` (`__host__ __device__`).
- `aigpdk/gemmacro.v`, `aigpdk/gemmacro_map.v` — Yosys blackbox model + DSP48E2
  techmap (UNVALIDATED).

Modified:
- `src/lib.rs` — `pub mod hwmacro; pub mod emulate;`
- `src/aig.rs` — `MacroBlock`, `DriverType::Macro`, `EndpointGroup::Macro` (+
  `for_each_input`), `AIG::macros`, `GEM_DSP48E2` arms in
  `dfs_netlistdb_build_aig`, the clock-trace loop, and the post-pass
  (`inputs_iv` collection + `en_iv = clk & (CEP|RSTP)`), endpoint accessors
  (macros appended last).
- `src/aigpdk.rs` — `GEM_DSP48E2` in `direction_of` / `width_of`.
- `src/pe.rs` — `num_macros` counted in `Partition::build_one`; reserved
  write-outs `+ 2*num_macros + (num_macros!=0)`, permute-slot bound
  `+ num_macros*4`.
- `src/flatten.rs` — `FlatteningPart::{num_macros,wo_base_dup,wo_base_sram,
  wo_base_macro}`; explicit write-out bases with an even-rounded macro base; an
  `EndpointGroup::Macro` arm in `make_inputs_outputs` (4 packed permute lanes
  after the SRAM lanes; 48 output bits into the macro state words, en-gated);
  4 new metadata words (emitted only when `num_macros>0`); even `state_start`
  for macro partitions and even `reg_io_state_size` overall.
- `src/bin/cuda_test.rs` — local `simulate_block_v1` deleted, `use
  gem::emulate::simulate_block_v1`.
- `src/bin/naive_sim.rs` — `GEM_DSP48E2` recognised as a clocked endpoint; its
  `P` pins pre-marked; its inputs added as DFS roots; a latch-section arm that
  packs inputs via `input_bit_slot` and calls `hwmacro::eval_dsp48e2`.
- `csrc/kernel_v1_impl.cuh` — `#include "macro_eval.cuh"`; read metadata
  `[8..12]`; a uniform-scope 4-lane `__shfl_sync` gather + early 64-bit `P`
  feedback load; SRAM/dup commits use the explicit bases; a predicated
  macro-commit block writing `P` lo/hi from lanes `sub<2`; dup-lane range
  shifted by `num_macros*4`.
- `src/repcut.rs` — `#[cfg(feature = "partitioner")]` on `RCHyperGraph::partition`.
- `Cargo.toml` — `mt-kahypar` optional; `partitioner` feature; `cuda` implies it;
  `cut_map_interactive` gains `required-features = ["partitioner"]`.
- `aigpdk/aigpdk.lib` — `GEM_DSP48E2` cell (`dont_touch/map_only/is_macro_cell`).
- `aigpdk/gemmacro_map.v` — **substantially rewritten** (see item 8 above): valid
  default parameters, full Yosys-0.68 `DSP48E2` port/param list, `OPMODE` decode
  gated on `X=01 && Y=01`.
- `usage.md` — "Step 1.5" rewritten into Path A (explicit instantiation,
  validated) + Path B (RTL inference, version-dependent).
- `eda-infra-rs/ucc/src/compile.rs` — Windows branch in `cl_cpp_openmp`
  (submodule edit, local to this checkout — a Linux build is unaffected).

## To finish

Only the GPU numeric differential is left; everything else in the original
plan + verification section is done and checked.

1. Let `cargo build --features cuda` finish on the WSL box (it is `mt-kahypar`
   that is slow, not anything Part A; run it `-j2` so the 7 GB VM does not
   thrash). `csrc/` already compiles under `nvcc` in isolation.
2. `cuda_test --check-with-cpu` on `tests/data/mac_accumulator.gv` — generate a
   `.gemparts` with `cut_map_interactive` (`--features cuda`; `num_parts == 1`
   is special-cased so no real `mt-kahypar` call), a small random input VCD,
   then diff GPU vs `emulate::simulate_block_v1`. Focus on the macro `P` bits
   and the `c3 == 0` data-inversion assumption noted above.
3. `compute-sanitizer` over the `__shfl_sync` gather and the 64-bit `P` load.
4. (optional) Path B of `usage.md` Step 1.5 against a Yosys build whose
   `xilinx_dsp` folds `P <= P + A*B` into `PREG`, if word-level MAC inference
   from plain RTL is wanted rather than explicit instantiation. Neither 0.33
   nor the pinned 0.68 does this; closing it properly means either a patch to
   `xilinx_dsp` or a GEM-side pattern match that folds the fabric adder and
   flop into the macro itself.

## Log

- Read the whole pipeline (`aig` -> `staging` -> `repcut` -> `pe` -> `flatten` ->
  `csrc/kernel_v1_impl.cuh`), plus `cuda_test`, `naive_sim`, `cut_map_interactive`,
  the aigpdk liberty/verilog. Confirmed the SRAM path is the template to mirror.
- Implemented items 1-9 + V. `cargo test` green; macro-free `blocks_data` hash
  verified byte-identical against a clean pre-Part-A `HEAD` build.
- **Linux/WSL pass (later session):** `cargo build` + `cargo test` +
  `macro_test` green on native Linux too (`simple_dff` hash matches). Yosys
  validation of the DSP48E2 frontend — found and fixed the `gemmacro_map.v`
  default-parameter `$error` bug and three errors in the `usage.md` Step 1.5
  recipe. `csrc/kernel_v1.cu` compiles clean under `nvcc` 12.6 (`sm_86` +
  default arch set), 0 warnings, 0 register spill. `cargo build --features
  cuda` running; GPU numeric diff is the only remaining item.
- **Yosys re-validation pass:** the frontend work above was originally checked
  against the distro's Yosys 0.33. The pinned version for this project is
  **0.68**, so the `v0.68` release (`git sha1 38e001a6f`) was built from source
  in `~/yosys` and the whole
  of item 8 was re-run against it. Every Part A conclusion survived unchanged:
  the `DSP48E2` port/param list is identical, `xcup_dsp_map.v` still does not
  exist, Path A maps and folds `OPMODE_S` exactly as before, both `$error`
  guards fire, and `xilinx_dsp` still refuses to fold the accumulator into
  `PREG` (now checked over three RTL forms × four flows).

---

# Part A verification checklist (independent review, 2026-08-30;
# Yosys claims re-validated against 0.68 on 2026-08-31)

Cross-checked `planA.md` and `question.md` against the actual tree
(`git diff` + the 6 new files). Re-ran on Windows/MSVC, no features:

```
cargo build --bins                 exit 0
cargo test                         9 hwmacro unit tests + 2 macro_test = all pass
cargo run --bin macro_test         simple_dff  256 cyc, 0 macros -> PASS
                                   mac_accumulator 512 cyc, 1 macro -> PASS
  simple_dff  blocks_data hash 4367312310778971198  == pinned SIMPLE_DFF_HASH  (ABI unchanged)
  mac_accumulator blocks_data hash 1388458641175822285, reg/io state 132
```

## A. `question.md` — DSP48E2 (Primitive A) semantics

- [x] AREG/BREG/CREG/DREG/ADREG/MREG combinational, only PREG clocked —
      enforced by `gemmacro_map.v` `$error` guards; hard-wired in `gemmacro.v`
      and in `eval_dsp48e2` / `gem_eval_dsp48e2`.
- [x] 27-bit A, 27-bit D, 18-bit B pre-adder; `AD = A + D` or `A` (via `USE_D`),
      wraps at 27 bits — `hwmacro.rs:194`, `macro_eval.cuh:49`, unit test
      `pre_adder_wraps_at_27_bits`.
- [x] 45-bit combinational product `M = AD * B` — `ad.wrapping_mul(b)`.
- [x] 48-bit ALU with simplified 2-bit `OPMODE_S`: 0 → `P<=C`, 1 → `P<=M`,
      2 → `P<=P+M` — branchless select, unit test `opmode_states`.
- [x] OVERFLOW/UNDERFLOW ignored; P wraps at 48 bits — `& 0x0000_ffff_ffff_ffff`.
- [x] signed two's-complement, single pass — `sext` on A/D/B/C/P, `i64` datapath.
- [x] RSTP synchronous P reset with precedence over CEP — OR'd into `en_iv`
      *and* zeroes data inside the macro; unit test `rstp_zeroes_output`.
- [x] "Yosys parser extracts the intent, passes a simplified 2-bit state" —
      `gemmacro_map.v` decodes `OPMODE[6:0]` (+ `IS_OPMODE_INVERTED`) to
      `OPMODE_S` as combinational logic, so a dynamic `OPMODE` becomes AIG gates
      feeding the macro (the heterogeneous 1-bit-control-into-word-macro case).
- [~] MAC **inferred from ordinary RTL `a*b+p`** (plan's stated approach) — does
      **not** hold with stock Yosys 0.68 (nor 0.33): `xilinx_dsp` leaves
      `PREG=0` and the accumulator in fabric, which the techmap then rejects.
      Only *explicit* instantiation (`GEM_DSP48E2`, or a `PREG=1` `DSP48E2` +
      techmap) is validated. `usage.md` Step 1.5 now documents this as Path A
      (works) vs Path B (version-dependent). **Functional gap vs. the
      challenge's "no hand-instantiation" framing** — and, unlike Parts B and
      C, the 0.33 → 0.68 bump did not close it.

## B. `planA.md` work-breakdown items

- [x] 1 — `src/hwmacro.rs`: `MacroClass`, `MacroKind::Dsp48e2`, `class`,
      `num_input_bits`(124), `num_output_bits`(48), `num_perm_words`(4),
      `num_state_words`(2), `input_bit_slot`, `eval_dsp48e2`, `dsp48e2_pack`,
      9 unit tests. Canonical order matches the plan
      (`A,D,B,C,OPMODE_S,USE_D,RSTP`).
- [x] 2 — `src/aig.rs`: `MacroBlock`, `DriverType::Macro`, `EndpointGroup::Macro`
      (+ `for_each_input` = `en_iv` + 124 inputs), `AIG::macros`, `GEM_DSP48E2`
      arms in `dfs_netlistdb_build_aig` / clock-trace loop / post-pass,
      `en_iv = clk & (CEP | RSTP)` via the De Morgan `add_and_gate` idiom,
      endpoint groups **appended after SRAMs**.
- [x] 3 — `src/aigpdk.rs`: `GEM_DSP48E2` arms in `direction_of` + `width_of`
      (CLK/USE_D/CEP/RSTP scalar in, A/D `[26:0]`, B `[17:0]`, C `[47:0]`,
      OPMODE_S `[1:0]` in, P `[47:0]` out).
- [x] 4 — `src/pe.rs`: `num_macros` counted; permute bound
      `+ num_macros*4`; reserved write-outs `+ 2*num_macros + (num_macros!=0)`.
      Both extra terms vanish at `num_macros == 0`.
- [~] 4 (optional) — macro endpoint weighting in `repcut.rs` and
      `executor_fixed_cost` in `flatten.rs`: **not done** (deliberate; no
      correctness impact, load-balance only).
- [x] 5 — `src/flatten.rs`: `FlatteningPart::{num_macros,wo_base_dup,
      wo_base_sram,wo_base_macro}`, explicit bases replace the
      `num_ios - num_srams - …` subtractions, even-rounded `wo_base_macro`,
      `EndpointGroup::Macro` arm in `make_inputs_outputs` (48 en-gated output
      bits + 4 packed input lanes after the SRAM lanes), 4 metadata words
      emitted only when `num_macros > 0`, even `state_start` for macro
      partitions + even `reg_io_state_size`, ABI table in the module doc.
- [x] 6 — `src/emulate.rs`: `simulate_block_v1` moved verbatim out of
      `cuda_test` (that binary now `use`s it), macro path added mirroring the
      kernel epilogue. `simple_dff` hash proves the move is behaviour-preserving.
- [x] 7 — `csrc/macro_eval.cuh` (`gem_sext`, `gem_eval_dsp48e2`, `__host__
      __device__`) + `kernel_v1_impl.cuh` (metadata `[8..12]`, uniform-scope
      4-lane `__shfl_sync` gather, early 64-bit P load, predicated macro
      commit, dup-range shift). **Compiles under nvcc 12.6 per the log; not
      recompilable or runnable in this review env (no CUDA).**
- [x] 8 — `aigpdk/gemmacro.v` (blackbox + behavioural), `aigpdk/gemmacro_map.v`
      (`DSP48E2 → GEM_DSP48E2`, full Yosys-0.68 port/param list, `$error`
      guards), `aigpdk.lib` `GEM_DSP48E2` cell (`dont_touch/map_only/
      is_macro_cell`), `usage.md` Step 1.5. Validated against Yosys 0.68 per the
      log (2 bugs found + fixed). See the [~] caveat in section A.
- [x] 9 — `src/bin/naive_sim.rs`: `GEM_DSP48E2` recognised as a clocked
      endpoint, P pins pre-marked, inputs as DFS roots, latch-section arm
      packing via `input_bit_slot` + `eval_dsp48e2`. Compiles; smoke-run only.
- [x] V — `src/bin/macro_test.rs` + `tests/data/{simple_dff,mac_accumulator}.gv`.
      Full pipeline `AIG::from_netlistdb → build_staged_aigs → Partition::
      build_one → FlattenedScriptV1` then a per-cycle bit-exact differential.

## C. `planA.md` design decisions / data layout

- [x] Class R only; `MacroClass::Combinational` reserved but unused.
- [x] Class-R schedule level = max level of its input bits — inherited free from
      the generic `for_each_input` path in `staging.rs` (no edit needed there);
      macro output pins are non-`AndGate` so `topo_traverse_generic` stops at
      them and they get level 0, exactly like SRAM/DFF-Q.
- [x] Branchless datapath (sign-masks, no `switch` on opmode).
- [x] 4 lanes/macro, 4 | 32 so lanes never straddle a warp; `__shfl_sync` full
      mask at uniform scope.
- [x] 64-bit-aligned P: even `wo_base_macro`, even `state_start` for macro
      partitions, even `reg_io_state_size`.
- [x] Input packing table (lane 0 `A`+`OPMODE_S`@27+`USE_D`@29+`RSTP`@30,
      lane 1 `D`+`C[36:32]`@27, lane 2 `B`+`C[47:37]`@18, lane 3 `C[31:0]`) —
      matches `input_bit_slot`; unit tests `input_bit_slots_are_a_permutation`
      + `packing_roundtrips_every_bit`. (Minor: the doc-comment table in
      `hwmacro.rs` has stale "27..29" / "0..27" exclusive-range typos; the code
      is correct.)
- [x] CEP not packed — rides `clken_permute` like the SRAM `port_r_en_iv`.
- [x] Metadata slots 8–11 = `num_macros, wo_base_dup, wo_base_sram,
      wo_base_macro`; kernel + emulator both read `[8..12]` and fall back to the
      legacy subtraction arithmetic when `num_macros == 0`.
- [x] `sram_duplicate_permute` = `[srams×4][macros×4][dups×1]`; `dup_perm_pos`
      and the kernel dup-branch bound both shifted by `num_macros*4`.

## D. `planA.md` verification section

- [x] 1 — `eval_dsp48e2` unit tests: all 3 states, USE_D on/off, RSTP on/off,
      signed extremes (`-2^26`, `-2^17`, `-2^47`), 27-bit pre-adder wrap, 48-bit
      accumulator wrap, 124-bit packing round-trip, negative P feedback. **9/9.**
- [x] 3 — macro-free script ABI byte-identical: `simple_dff` `blocks_data` hash
      `4367312310778971198` reproduced this run and pinned as `SIMPLE_DFF_HASH`.
- [~] 2 — end-to-end differential exists and passes (512 cycles, bit-exact on
      every primary output and every P bit), **but** it runs `emulate::
      simulate_block_v1` against a bespoke `Behavioural` evaluator defined inside
      `macro_test.rs`, not against `naive_sim` as the plan wording specified.
      Still an independent second implementation, so the cross-check is real;
      the `naive_sim` ↔ emulator agreement the plan called out is not wired up.
- [~] 4 — frontend smoke test: done for Yosys 0.68 Path A; Path B (plain-RTL
      inference) blocked by that Yosys release too (see section A).
- [ ] 5 — GPU numeric differential (`cuda_test --check-with-cpu`,
      `compute-sanitizer`): **not done.** No CUDA toolchain in this environment;
      nvcc compile-clean is claimed in the log but the GPU-vs-CPU numeric run
      and the sanitizer pass over the new shuffles / 64-bit loads remain open.
      Residual risk: the kernel applies `writeout_inv ^= c3` to the macro P
      words *after* it stores them while the emulator does not — they agree only
      because `place_clken_datainv(…, 0)` forces `c3 == 0` for those bit
      positions (structurally true in the code, but only a GPU diff proves it
      end-to-end).

## E. Out of scope for Part A / not implemented

- [ ] Primitive B (CARRY4) — Class C, needs an intra-cycle macro eval slot in
      the boomerang. Enum reserved only.
- [ ] Primitive C (SRLC32E) — mixed Class C (addressed read) / Class R (Q31).
- [ ] Boomerang scheduler extension for combinational (Class C) macros — the
      genuinely new scheduler work; untouched.
- [ ] `PREG=0` (purely combinational `a*b`) — falls back to AIG gates; in spec
      for Part A but a real limitation for the wider challenge.
- [ ] Wide-multiplier `mul2dsp` tiling / DSP cascade rewrite — untested.
- [ ] Unpacking non-zero `AREG/BREG/…` into explicit DFFs — `gemmacro_map.v`
      errors out instead (plan allowed this as an acceptable first cut).

## F. Incidental

- [x] `Cargo.toml`: `mt-kahypar` made optional behind `partitioner`;
      `cuda = [..., "partitioner"]`; `cut_map_interactive` gains
      `required-features = ["partitioner"]`; `repcut::RCHyperGraph::partition`
      is `#[cfg(feature = "partitioner")]`. Motivated (Windows C++ build), and
      a CUDA/Linux build is unchanged.
- [x] `eda-infra-rs/ucc/src/compile.rs` — Windows OpenMP branch (submodule edit,
      local to this checkout).
- [ ] `src/bin/cuda_test.rs` line 1 gained a UTF-8 BOM (`U+FEFF`) — harmless
      (rustc strips it, and the file is `cuda`-gated) but an unintended edit
      worth reverting.

## Verdict

Part A as scoped in `planA.md` is **implemented and CPU-verified**: every
work item landed, the substrate mirrors the SRAM path faithfully, the datapath
matches `question.md` bit-for-bit under test, and the macro-free script ABI is
provably unchanged. Two things keep it from "fully done":

1. **GPU numeric differential not run** (verification item 5) — the only
   correctness gate not closeable without a GPU.
2. **RTL inference of the MAC does not work with stock Yosys 0.68** (the pinned
   version) — Part A only maps *explicitly instantiated* DSP macros, not
   `p <= a*b + p` written as plain arithmetic, which is a real divergence from
   the challenge's intent. Note this is Part A's problem alone: with 0.68,
   Parts B and C *do* infer their primitives from plain RTL end-to-end.
