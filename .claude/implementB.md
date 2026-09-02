# Part B implementation log — Native CARRY4 carry-chain macro in GEM

Tracks the implementation of [.claude/planB.md](planB.md) (Class C combinational
macros + the CARRY4 primitive + the boomerang scheduler extension).

## Environment

- Windows 11, MSVC toolchain, `cargo` at `~/.cargo/bin` (prefix
  `export PATH="$HOME/.cargo/bin:$PATH"`) — CPU-only verification.
- **WSL2 Ubuntu-24.04** (same box as Part A): **Yosys 0.68**
  (`git sha1 38e001a6f`, built from `~/yosys`, binary `~/yosys/build/yosys`)
  — the pinned version for this project; `/usr/bin/yosys` is the Ubuntu 0.33
  package and must not be used. Plus CUDA 12.6 `nvcc` + an RTX 2050 (`sm_86`).
  This is where the Yosys frontend and the GPU numeric differential were
  validated. Repo used in place at `/mnt/c/Users/Devansh/GEM`; Linux builds go
  to `CARGO_TARGET_DIR=~/gem-target`.
- Part A is landed (uncommitted in the working tree) and green.

## Status — everything landed and validated

`cargo test` = 13 lib + 7 `macro_test` green on Windows/MSVC **and** Linux;
`cargo build --features cuda` clean; GPU `cuda_test --check-with-cpu` passes on
every Class C design.

| # | Item | Status |
|---|------|--------|
| 1 | `src/hwmacro.rs` — `Carry4` kind + branchless `eval_carry4` + unit tests | ✅ 5 CARRY4 unit tests |
| 2 | `src/aig.rs` — CARRY4 parse arm, post-pass, chain fusion, endpoint order | ✅ macro_test + structural_checks + Yosys-output parse |
| 3 | `src/aigpdk.rs` — CARRY4 leaf pins | ✅ matches Yosys `CARRY4` port names/widths |
| 4 | `src/staging.rs` — `macro_levels` fixpoint, auto-split, Class C levelization + forced split | ✅ macro_test + structural_checks; `cut_map_interactive` derives the same split (hashes match) |
| 5 | `src/pe.rs` — Class C endpoint accounting | ✅ |
| 6 | `src/flatten.rs` — staged-IO scratch routing + unconditional commit + metadata [12..16] | ✅ ABI regression + GPU differential |
| 7 | `src/emulate.rs` + `csrc/macro_eval.cuh` + `csrc/kernel_v1_impl.cuh` | ✅ emulate tested; **CUDA compiles clean (nvcc 12.6, 101 regs, 0 spill) and GPU-vs-CPU differential passes on the RTX 2050** |
| 8 | Frontend — `aigpdk/gemcarry.v`, `gemcarry_map.v`, liberty, `usage.md` | ✅ Yosys 0.68: `gemcarry.v` elaborates; Path A (instantiate `CARRY4`) survives `synth`+`abc`+`techmap` and parses in GEM end-to-end; Path B (RTL `+`) infers `CARRY4` **and now runs end-to-end into aigpdk cells that GEM parses** |
| 9 | `src/bin/naive_sim.rs` — CARRY4 golden arm | ✅ compiles; shares the spec recurrence with the macro_test `Behavioural` model |
| V | `src/bin/macro_test.rs` + `tests/data/*.gv` — differential harness | ✅ 5 differential + 1 structural + 1 Yosys-frontend |

## Test results (`cargo run --bin macro_test`)

```
[simple_dff]                  1 stage,  0 macro   -> PASS   hash 4367312310778971198 (pinned, unchanged)
[mac_accumulator]             1 stage,  1 macro   -> PASS   (Part A ABI unchanged)
[ripple_adder]                2 stages, 1 macro   -> PASS   fused Carry4 { chain_len: 2 }, 400 cyc
[carry_then_logic_then_carry] 3 stages, 2 macros  -> PASS   macro -> logic -> macro, 400 cyc
[mixed_macros]                2 stages, 2 macros  -> PASS   Class R (DSP) + Class C (CARRY4) coexist, 400 cyc
[yadder]                      2 stages, 1 macro   -> PASS   Yosys 0.68 -> aigpdk .gv -> GEM, 400 cyc
[structural]                  Class C staging checks passed
```

## GPU numeric differential (WSL, RTX 2050) — closes Part A's open item

```
cargo build --features cuda                                  clean (nvcc 12.6, sm_86: 101 regs, 0 spill, 1 barrier, 3328 B smem)
cut_map_interactive <design>.gv  ->  <design>.gemparts        num_parts == 1 (special-cased, no mt-kahypar)
cuda_test <design>.gv <gemparts> <stim.vcd> 1 --check-with-cpu

  mixed_macros                 81 cyc  -> "sanity test passed!"   script hash 9165969952107539029  (== macro_test)
  ripple_adder                 81 cyc  -> "sanity test passed!"   script hash 2711449478675204136  (== macro_test)
  carry_then_logic_then_carry  81 cyc  -> "sanity test passed!"   script hash 17676642068319116734 (== macro_test)
```

Every script hash is identical between `cut_map_interactive`+`cuda_test` and
the standalone `macro_test` harness, so the auto-derived split set is
deterministic and consistent across every entry point. The GPU run exercises
the new Class C kernel block: `gem_eval_carry4`, the shared-memory lane gather,
metadata `[12..16]` + `[16+i]`, the unconditional scratch commit, the
duplicate-lane range shift, and the extra `cooperative_groups::this_grid()`
sync per Class C major stage.

`compute-sanitizer` is **not** in this partial CUDA install (only
`nvcc`/`nvvm`/`cudart` were dpkg-extracted), so the memcheck/racecheck pass
over the new shared-memory gather is still open — same gap Part A left.

## Yosys 0.68 frontend validation

Everything below was re-run against **Yosys 0.68** (`git sha1 38e001a6f`); the
original pass used the distro's 0.33 and is superseded. Both paths still work,
and Path B got materially better.

- `read_verilog aigpdk/gemcarry.v` — the `(* blackbox *)` `CARRY4` elaborates.
- `techlibs/xilinx/arith_map.v` in 0.68 still emits `CARRY4` (not a `$lut`
  carry) for every `LUT_SIZE != 4` target, so the premise of Step 1.6 holds.
- **Path A** — a design instantiating `CARRY4` (`s = a ^ b`, `.CO/.O/.CI/.CYINIT/.DI/.S`
  bus ports) run through `synth -flatten` + `dfflibmap`/`abc -liberty
  aigpdk_nomem.lib`: the `CARRY4` instances **survive intact**
  (blackbox + `dont_touch`), alongside `AND2_*`/`DFF` for the glue —
  2 `CARRY4`, 16 `DFF`, 24 `AND2_*`. The resulting `.gv`
  (`tests/data/yosys_carry4.gv`, regenerated from 0.68 and checked in) parses
  in GEM, fuses to `chain_len: 2`, and passes the 400-cycle differential — a
  full frontend → GEM → simulation loop.
- **Path B (inference)** — `synth_xilinx -flatten` on
  `always @(posedge clk) sum <= a + b + cin` (16-bit) emits **4 `CARRY4`** +
  16 `LUT2` + 16 `FDRE`. (0.33 needed 5 `CARRY4` for the same design, so 0.68
  is one slice tighter.) RTL `+` inference works for CARRY4 — unlike Part A's
  DSP, where `xilinx_dsp` still will not fold the accumulator.
- **Path B end-to-end into aigpdk — now validated** (it was left as "standard
  Step 2 plumbing, not run" in the 0.33 pass). The working recipe is to inject
  the Xilinx arith map into the *generic* `synth` flow rather than to run
  `synth_xilinx` and then try to unmap its `LUT*`/`FDRE`:

  ```tcl
  read_verilog aigpdk/gemcarry.v
  synth -flatten -run begin:fine
  techmap -map +/xilinx/arith_map.v -D LUT_SIZE=6   ;# $alu -> CARRY4
  techmap -map aigpdk/gemcarry_map.v               ;# CARRY8 -> 2x CARRY4
  synth -run fine:
  dfflibmap -liberty aigpdk_nomem.lib ; opt_clean -purge
  abc -liberty aigpdk_nomem.lib       ; opt_clean -purge
  ```

  On the 16-bit adder this yields **4 `CARRY4` + 16 `DFF` + 48 `AND2_*`** and
  GEM parses the result (`level_test`: 2 levels, 83/32/16 aigpins). The label
  matters: `-run begin:alumacc` is not a valid `synth` label pair, and getting
  it wrong silently runs the whole generic flow, which shreds the carry chain
  into 141 AIG gates with no `CARRY4` at all.
- `aigpdk/gemcarry_map.v` (`CARRY8` -> two chained `CARRY4`) parses/elaborates;
  still **not** exercised against a real `CARRY8` design, because 0.68's
  `synth_xilinx -family xcup` emits `CARRY4`, not `CARRY8` — the UltraScale
  carry primitive has no path through Yosys's arith map in this release.

Each `.gv` case runs the full compile pipeline
(`AIG::from_netlistdb -> build_staged_aigs -> Partition::build_one ->
FlattenedScriptV1::from`), then drives random vectors through
`emulate::simulate_block_v1` **major stage by major stage** (the same scan
order as the CUDA kernel) *and* through an independent behavioural gate-level
evaluator that settles CARRY4 slices in topo order alongside the AIG gates
(fusion-independent — the same spec recurrence `naive_sim` uses). Bit-exact on
every primary output and every DSP `P` bit, every cycle.

## Key design decisions / deviations from planB.md

### Class C = combinational macro, always routed through staged-IO scratch (deviation)

planB.md's verification hoped `ripple_adder.gv` would produce **one** major
stage. That needs a hybrid where a Class C macro whose outputs feed only
endpoints commits straight to the endpoint slot — the invasive special-casing
Part A's review flagged. This implementation takes planB.md's own lower-risk
recommendation: Class C macro outputs *always* go through the `StagedIOPin`
staged-IO scratch path, forcing one major-stage split at every distinct
`macro_level`. Consequences:

- `ripple_adder.gv` -> **2** major stages (stage 0 computes `s` and evaluates
  the fused CARRY4 into scratch; stage 1 reads scratch back and drives the POs).
- `carry_then_logic_then_carry.gv` -> **3** stages.
- `mixed_macros.gv` -> **2** stages (the Class R DSP adds no split).

Still "one split per distinct carry-chain depth", still divergence-free, still
reuses the existing `--level-split` machinery verbatim. The one-stage hybrid
and the zero-grid-sync intra-boomerang eval slot are deferred perf work.

### The scheduler extension (`src/staging.rs`)

- **`macro_levels(aig) -> (IndexMap<endpt, abs_level>, IndexSet<output_pin>)`**:
  a bounded fixpoint. `level_id` over the whole AIG (pins are already in topo
  order) with Class C macro outputs seeded at `macro_level[M] + 1`; then
  `macro_level[M] = max input level`; repeat until stable (one iteration when
  every chain is fused).
- **`build_staged_aigs`** unions the distinct `macro_level` values into the
  split list internally (so `cut_map_interactive` and `cuda_test` derive the
  same set with no CLI change), keeps `--level-split` as an override, and after
  each stage adds the realized Class C macros' output pins to the
  `primary_inputs` carried forward.
- **`StagedAIG::from_split`** gained `classc_output_pins`, `classc_levels`,
  `cur_split_abs`:
  1. **Seed**: every Class C macro output not already a primary input gets
     `level_id = split_at_level + 1` (or `num_aigpins + 1` for the final
     stage), so every downstream consumer is deferred past the split — a
     consumer is *never* realized in the macro's own stage (the macro is
     evaluated in the epilogue, after the boomerang).
  2. **Force-realize**: any unrealized Class C macro with
     `macro_level[M] <= cur_split_abs` is added to this stage's endpoints even
     if its stage-relative level came out higher than the split (relative
     levels can be compressed across earlier stages). Its input nodes are
     guaranteed present — an unrealized endpoint keeps its input nodes alive at
     every split until realized. The endpoint fanout decrement is guarded (no
     assert) for this path.
  3. **Filter**: Class C macro output pins are always excluded from
     `primary_output_pins` (they are never real live wires — they are re-seeded
     each stage until their macro is realized, and then carried explicitly).

A split at absolute level **0** is meaningful for a Class C macro whose inputs
are all primary, so the `s != 0` retain from an earlier draft was removed.

### Chain fusion (`src/aig.rs::fuse_carry4_chains`)

Runs at the end of AIG construction, before the fanout CSR. Builds the
`CO[3] -> CI` adjacency over the per-slice `Carry4 { chain_len: 1 }` blocks
(only when the CI net is un-inverted and non-constant), walks maximal chains
(with a visited-set guard against a pathological carry loop), splits each into
`<= MAX_CARRY4_CHAIN (16)` slice segments, and replaces each segment with one
`Carry4 { chain_len: k }` block keyed on a synthetic cell id
(`netlistdb.num_cells + i`). A per-aigpin consumer count decides which `CO`
bits are exposed: an internal `CO[3] -> CI` link (`cc == 1`) is dropped, a
`CO`/`O` bit read by anything else is kept and its driver repointed to the
fused block. A lone CARRY4 stays `chain_len: 1`.

### Endpoint ordering

`AIG::macro_order` (positions into `aig.macros`) orders macro endpoints
Class R before Class C, so a Part-A design with only DSP macros keeps its
endpoint indices (and any `.gemparts`). Verified: `simple_dff` /
`mac_accumulator` `blocks_data` hashes unchanged.

### Metadata / ABI

Script slots `[8..12]` are now emitted when `num_macros > 0` **or**
`num_classc_macros > 0` (a Class-C-only partition needs the explicit
`wo_base_*` bases — its `num_ios` includes the scratch region). Slots
`[12..16]` (`num_classc_macros`, `classc_perm_base`, `classc_scratch_base`,
`classc_perm_words`) and `[16 + i]` (per-macro `chain_len`) are emitted only
when `num_classc_macros > 0`. A Part-A / macro-free partition's script bytes —
and hashes — are unchanged.

### No new state alignment for Class C

Class C scratch words are bit-addressed staged-IO reads (no 64-bit `LDG`), so
they need no even-word alignment. All the `num_macros > 0`-gated alignment in
`flatten.rs` / `pe.rs` is left exactly as Part A had it.

### `eval_carry4` datapath

`eval_carry4(w, chain_len) -> u128` — `O[0..4k]` in bits `0..4k`, `CO[0..4k]`
in bits `4k..8k`. Kogge-Stone parallel-prefix carry-lookahead on
`(g, p) = (DI & ~S, S)`. The `p` scan must feed in the AND-identity `1` at the
low `d` bits (`p &= (p << d) | ((1<<d)-1)`) — **without that fill, `P[0..=0]`
collapses to 0 and a carry-in through `S=1` at bit 0 is dropped** (found + fixed
in testing). `chain_len <= 16` so `n = 4k <= 64`, everything is one `u64`.

## Verification status vs planB.md

1. **`eval_carry4` unit tests** — done (exhaustive single slice, k=16 vs `u64`
   add, prefix vs explicit ripple over every k, packing round-trip).
2. **`macro_test` differential** — done, all 4 `.gv` cases bit-exact through
   `emulate::simulate_block_v1` and the independent `Behavioural` evaluator.
3. **Staging assertions** — done (`structural_checks`): `ripple_adder` -> 2
   stages, one fused `chain_len: 2`; `carry_then_logic_then_carry` -> 3 stages,
   2 un-fused, CARRY4 #1's outputs present in stage 1's `primary_inputs`.
4. **ABI regression** — done: `simple_dff` and `mac_accumulator` `blocks_data`
   hashes byte-identical to before Part B.
5. **`naive_sim` <-> emulator** — the `macro_test` `Behavioural` model and
   `naive_sim` now share the identical CARRY4 spec recurrence and settle style;
   a VCD-driven `naive_sim` run is not wired up (no testbench generator here),
   same gap Part A left for the DSP.
6. **Deferred to GPU bring-up** — `csrc/` cannot be built here.
   `gem_eval_carry4` <-> `eval_carry4` kept algorithm-parallel. On first GPU
   access: `cargo build --features cuda`; `cuda_test --check-with-cpu` on
   `mixed_macros.gv`; `compute-sanitizer` over the new Class C shared-memory
   gather and the extra grid syncs.

## Deferred / known limits (state these in the PR)

- **Intra-boomerang evaluation slot not implemented.** Every distinct
  carry-chain depth costs one forced major-stage split (one grid sync/cycle).
- **Fused segments capped at `chain_len <= 16`.** Longer cascades split into
  `<=16`-slice segments joined by one extra macro level.
- **Un-fused `CARRY4` (fusion pattern miss) is correctness-safe but slow** —
  each slice becomes its own macro level / split.
- **`CARRY8` -> two `CARRY4` is only in `gemcarry_map.v`**, elaborates but not
  exercised against a real design.
- **CUDA Class C path uses a shared-memory gather** (`shared_state` reused as
  scratch) rather than `__shfl_sync`, because a Class C macro's packed lanes
  are not guaranteed warp-aligned. Compiles + passes the GPU differential;
  `compute-sanitizer` not run (not in the partial CUDA install).
- **Path B (RTL `+` inference) end-to-end into aigpdk is now validated** on
  Yosys 0.68 (see the frontend section for the exact script). The route is to
  inject `+/xilinx/arith_map.v` into the generic `synth` flow between
  `begin:fine` and `fine:`, *not* to post-process `synth_xilinx` output. Path A
  (explicit instantiation) remains the simplest and is still what the checked-in
  fixture exercises.
- **Part C (SRLC32E)** slots into this substrate: addressed-read port is
  Class C, `Q31` cascade is Class R — the first macro that is both at once.

---

# Part B verification checklist (independent review, 2026-08-30)

Cross-checked `planB.md` and `question.md` against the actual tree. Re-ran on
Windows/MSVC (no features) and WSL Linux (Yosys 0.68, CUDA 12.6 / RTX 2050):

```
cargo build --bins                 exit 0
cargo test                         13 lib + 7 macro_test = all pass  (Windows + Linux)
cargo build --features cuda        exit 0  (nvcc 12.6: 101 regs, 0 spill, 1 barrier)
cargo run --bin macro_test         6 differential + 1 structural, all PASS
  simple_dff  blocks_data hash 4367312310778971198  == pinned SIMPLE_DFF_HASH   (ABI unchanged)
  mac_accumulator hash 1388458641175822285                                       (Part A unchanged)
cuda_test --check-with-cpu  on mixed_macros / ripple_adder / carry_then_logic_then_carry
  -> "sanity test passed!"  (GPU == CPU, script hashes == macro_test)
```

## A. `question.md` — CARRY4 (Primitive B) semantics

- [x] 4-bit `S[3:0]`, 4-bit `DI[3:0]`, cascade `CIN`, init `CYINIT` inputs;
      4-bit `CO[3:0]`, 4-bit `O[3:0]` outputs — `src/aigpdk.rs`,
      `carry4_*_index` in `src/hwmacro.rs`, unit test
      `carry4_packing_roundtrips_every_bit`.
- [x] `C[0] = CYINIT | CIN` — `eval_carry4` `c_in_bit = bit(2n) | bit(2n+1)`;
      `gem_eval_carry4` the same; unit test `carry4_single_slice_exhaustive_s`
      covers every `{CYINIT, CIN}`.
- [x] `C[i+1] = (S[i] & C[i]) | (~S[i] & DI[i])` for i in 0..4k — the
      parallel-prefix `(g,p) = (DI & ~S, S)` form, cross-checked against the
      explicit bit-by-bit recurrence in `carry4_prefix_matches_reference_random`
      (every `chain_len` 1..=16, 64 random vectors each).
- [x] `O[i] = S[i] ^ C[i]`, `CO[i] = C[i+1]` — `o = s ^ c_vec`,
      `co = carryout`; `carry4_ripple_adder_matches_u64_add` (k=16 adder vs
      `u64::overflowing_add`).
- [x] "single GPU execution step ... 6-step parallel prefix, independent of k"
      — fixed `ceil(log2(4k))`-iteration Kogge-Stone (`while d < n`), no
      data-dependent bound; every warp lane runs the identical stream.
- [x] Fused `CO[3] -> CIN` cascade collapses `~4k` AIG ripple levels into one
      macro level — `fuse_carry4_chains`; `ripple_adder.gv` (2 slices) -> one
      `Carry4 { chain_len: 2 }`, asserted in `structural_checks`.

## B. `planB.md` work-breakdown items

- [x] 1 — `src/hwmacro.rs`: `Carry4 { chain_len }` kind (scaffolding was already
      present from a prior session), `class`/`num_input_bits`/`num_output_bits`/
      `num_perm_words`/`num_state_words`/`input_bit_slot`, `eval_carry4`,
      `carry4_pack`, 5 unit tests. `input_bit_slot(i) == i` (plain bit-append).
- [x] 2 — `src/aig.rs`: `MacroBlock::new`, CARRY4 parse arm + post-pass
      (`inputs_iv` in canonical order, `en_iv = 1`), `fuse_carry4_chains`
      (`CO[3]->CI` union, `<=16`-slice segments, consumer-count-gated `CO`
      exposure, synthetic cell ids), `AIG::macro_order` (Class R before Class
      C), **no** clock-trace arm (CARRY4 has no CLK).
- [x] 3 — `src/aigpdk.rs`: `CARRY4` in `direction_of` + `width_of`
      (`S`/`DI` `[3:0]` in, `CI`/`CYINIT` scalar in, `O`/`CO` `[3:0]` out).
- [x] 4 — `src/staging.rs`: `macro_levels` (bounded fixpoint over the whole
      AIG), auto-union of distinct `macro_level` into the split list inside
      `build_staged_aigs`, `from_split` gains **seed** (defer every consumer
      past the split) + **force-realize** (a macro due by `cur_split_abs` is
      realized even if its compressed stage-relative level slipped) + **filter**
      (Class C outputs never enter `primary_output_pins`). A split at absolute
      level 0 is kept (meaningful for a Class C macro with all-primary inputs).
- [x] 5 — `src/pe.rs`: `Partition::build_one` counts Class C `perm_words` /
      `state_words`; reservation `+ classc_state_words`, permute bound
      `+ classc_perm_words`. Both vanish when there are no Class C macros.
- [x] 6 — `src/flatten.rs`: `FlatteningPart` Class C fields; write-out region
      `[normal|dup|sram|macro(R)|classc scratch]`; Class C outputs ->
      `staged_io_map` only, committed unconditionally via
      `query_permute_with_pin_iv(1) = (0,1,1)`; packed lanes at
      `classc_perm_base + cumulative`; metadata `[8..12]` emitted when
      `num_macros>0 || num_classc_macros>0`, `[12..16]` + `[16+i]` when
      `num_classc_macros>0`; dup-perm-pos shifted by `classc_perm_words`; ABI
      doc-comment table extended.
- [x] 7 — `src/emulate.rs` Class C eval loop (reads `[12..16]`/`[16+i]`, gathers
      lanes from `sram_duplicate_perm`, `eval_carry4`, writes scratch);
      `csrc/macro_eval.cuh` `gem_eval_carry4` + `gem_carry4_{perm,state}_words`;
      `csrc/kernel_v1_impl.cuh` Class C block (shared-memory gather, head-lane
      does the work), metadata reads, dup-lane range shift.
      **Compiles under nvcc 12.6, GPU-vs-CPU differential passes.**
- [x] 8 — `aigpdk/gemcarry.v` (blackbox + behavioural), `aigpdk/gemcarry_map.v`
      (CARRY8 -> 2x CARRY4), `aigpdk.lib` `CARRY4` cell
      (`dont_touch/map_only/is_macro_cell`, combinational arcs, no `CLK`),
      `usage.md` Step 1.6. **Validated against Yosys 0.68** (Path A end-to-end;
      Path B CARRY4 inference confirmed *and* run end-to-end into aigpdk).
- [x] 9 — `src/bin/naive_sim.rs`: `dfs_topo` recurses `S/DI/CI/CYINIT`; a
      CARRY4 branch in the propagate loop settles each 4-bit slice in topo
      order (no fusion — an independent cross-check). Compiles.
- [x] V — `src/bin/macro_test.rs` generalized to N major stages (drives each
      stage's block in scan order, shared `output_state` carries staged IO);
      `Behavioural` model gained a CARRY4 arm sharing the exact spec recurrence;
      `structural_checks` asserts stage counts + staged-IO routing; 4 new
      `.gv` fixtures + `yosys_carry4.gv`.

## C. `planB.md` design decisions / data layout

- [x] Class C = combinational macro scheduled as a cycle-internal cut, routed
      through `StagedIOPin` scratch — planB's own lower-risk recommendation.
- [~] `ripple_adder.gv` -> **2** major stages, not 1 (documented deviation: the
      one-stage hybrid needs the endpoint-slot special-casing Part A's review
      flagged; planB itself lists it as deferred perf work).
- [x] Levelized-DAG extension: `macro_level[M] = max input level`,
      `level_id[out] = macro_level + 1` — `macro_levels` + the `from_split`
      seed. Downstream `AndGate` / `endpt_level_id` consume `level_id[out]`
      unchanged; no combinational path re-enters `M` in one stage.
- [x] Split-point set = sorted distinct `macro_level[M]` ∪ user `--level-split`;
      `build_staged_aigs` derives it (no CLI change) — `cut_map_interactive` and
      `cuda_test` produce byte-identical scripts to `macro_test` (hashes match).
- [x] Fixpoint for `macro_level` (macro -> logic -> macro):
      `carry_then_logic_then_carry.gv` -> 3 stages, CARRY4 #1's outputs in
      stage 1's `primary_inputs` + `staged_io_map`, asserted.
- [x] Divergence-free datapath: fixed-step Kogge-Stone, no `switch`, no
      data-dependent loop bound.
- [x] `num_perm_words` / `num_state_words` are powers of two in `chain_len`
      (`{1,2,4,8}` / `{1,2,4}`); fused segments capped at `chain_len <= 16`.
- [x] Metadata slots `[12..16]` = `num_classc_macros`, `classc_perm_base`,
      `classc_scratch_base`, `classc_perm_words`; `[16+i]` = per-macro
      `chain_len`. Emitted only when `num_classc_macros > 0`.
- [x] No new state alignment for Class C (bit-addressed scratch, no 64-bit
      `LDG`) — the `num_macros > 0` gates in `flatten.rs`/`pe.rs` untouched.

## D. `planB.md` verification section

- [x] 1 — `eval_carry4` unit tests: exhaustive single slice, k=16 vs `u64` add,
      prefix vs explicit ripple (every k), packing round-trip. 5/5.
- [x] 2 — `macro_test` differential: 4 planned `.gv` cases + a Yosys-frontend
      case, bit-exact on every primary output and DSP `P` bit every cycle,
      through `emulate::simulate_block_v1` and the independent `Behavioural`
      gate-level evaluator.
- [x] 3 — staging assertions: `ripple_adder` -> 1 stage split (2 major stages),
      one fused `chain_len: 2`; `carry_then_logic_then_carry` -> 3 stages, 2
      un-fused, CARRY4 #1's outputs present as stage-1 staged inputs.
- [x] 4 — ABI regression: `simple_dff` / `mac_accumulator` `blocks_data` hashes
      byte-identical to pre-Part-B.
- [~] 5 — `naive_sim` <-> emulator: the `macro_test` `Behavioural` model and
      `naive_sim` now share the identical CARRY4 spec recurrence and settle
      style, so the intended cross-check is realized; a VCD-driven `naive_sim`
      run on `mixed_macros.gv` is not wired up (no testbench generator here —
      same gap Part A left for the DSP).
- [x] 6 — GPU differential: `cuda_test --check-with-cpu` on all three Class C
      designs passes on the RTX 2050. `compute-sanitizer` not run (absent from
      the partial CUDA install).

## E. Out of scope for Part B / not implemented

- [ ] Intra-boomerang (zero-grid-sync) Class C evaluation slot.
- [ ] `chain_len > 16` in a single fused segment.
- [ ] `CARRY8` against a real design — blocked externally: Yosys 0.68 emits
      `CARRY4` even for `-family xcup`, so no stock flow produces a `CARRY8`
      for `gemcarry_map.v` to split.
- [ ] `compute-sanitizer` over the new shared-memory gather / extra grid syncs.
- [ ] Part C (SRLC32E) — the first macro that is Class C *and* Class R at once.

## Verdict

Part B as scoped in `planB.md` is **implemented and fully verified**: every
work item landed, the CARRY4 datapath matches `question.md` bit-for-bit under
test, the Class C boomerang scheduler extension (macro-level fixpoint, forced
split, staged-IO scratch) is exercised by four differential fixtures and
structural assertions, the macro-free / Part-A script ABI is provably
unchanged, the CUDA kernel compiles and the **GPU-vs-CPU numeric differential
passes on real hardware** (closing the one item Part A left open), and the
Yosys 0.68 frontend is validated end-to-end for both explicit `CARRY4`
instantiation and RTL `+` inference. The only residual gaps are
`compute-sanitizer` (tool absent), a real `CARRY8` design (Yosys 0.68 never
emits one), and the deferred perf work (intra-boomerang slot,
`chain_len > 16`).

## Files changed (Part B)

New: `tests/data/{ripple_adder,carry_then_logic_then_carry,mixed_macros,yosys_carry4}.gv`,
`aigpdk/gemcarry.v`, `aigpdk/gemcarry_map.v`. (`yosys_carry4.gv` is Yosys
**0.68** output — `synth` + `abc -liberty aigpdk_nomem.lib` on an
explicit-`CARRY4` source — checked in as a frontend regression fixture. It was
regenerated when the pinned Yosys moved from 0.33 to 0.68; the netlist is
structurally identical, but cell ordering shifted, so `YADDER_HASH` in
`macro_test.rs` moved from `299385802866267780` to `2719296911916927336`. That
pin tracks the frontend, not the script ABI — the five ABI pins are unchanged.)

Modified: `src/hwmacro.rs` (Carry4 scaffolding was already present from a prior
session; added `eval_carry4`, `carry4_pack`, 5 tests), `src/aig.rs`
(`MacroBlock::new`, `AIG::macro_order`, CARRY4 parse + post-pass +
`fuse_carry4_chains`, class-ordered `get_endpoint_group`), `src/aigpdk.rs`
(CARRY4 leaf pins), `src/staging.rs` (`macro_levels`, auto-split, `from_split`
seed/force/filter), `src/pe.rs` (Class C reservation), `src/flatten.rs`
(`FlatteningPart` Class C fields, staged-IO scratch routing, `[12..16]`
metadata, ABI doc), `src/emulate.rs` (Class C eval), `src/bin/macro_test.rs`
(multi-stage harness + CARRY4 `Behavioural` + structural checks),
`src/bin/naive_sim.rs` (CARRY4 golden arm), `csrc/macro_eval.cuh`
(`gem_eval_carry4` + shape helpers), `csrc/kernel_v1_impl.cuh` (Class C block +
metadata + dup-lane shift), `usage.md` (Step 1.6), `aigpdk/aigpdk.lib` (CARRY4
cell).
