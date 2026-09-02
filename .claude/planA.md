# Part A — Native DSP48E2 MAC macro in GEM

## Context

GEM today knows exactly one datatype: the 1-bit AIG node. `AIG::from_netlistdb`
shreds every cell in the gate-level netlist into `AND2_*`/`INV`/`BUF`/`DFF`
primitives ([src/aig.rs:395-437](src/aig.rs#L395-L437)), the boomerang scheduler
places those single bits into a 13-level binary fold ([src/pe.rs:60-501](src/pe.rs#L60-L501)),
and the CUDA kernel evaluates them 32-at-a-time as bitwise
`(a^xora) & ((b^xorb) | orb)` ([csrc/kernel_v1_impl.cuh:145-194](csrc/kernel_v1_impl.cuh#L145-L194)).
A 27×18 signed multiply becomes thousands of AND gates and ~40 levels of boomerang depth.

The task is to let word-level hardware macros survive the frontend and be
evaluated natively on the GPU ALU. Part A is the DSP48E2 MAC: a 27-bit pre-adder,
a 45-bit multiplier, and a 48-bit ALU writing a clocked `P` register.

**The key insight that makes this tractable:** GEM already has a word-level macro —
the SRAM. `$__RAMGEM_SYNC_` is an endpoint group whose 128 input bits are gathered
out of the boomerang into 4 threads, evaluated natively as `ram[addr]`, and whose
32 output bits are re-injected as AIG source pins on the next cycle
([csrc/kernel_v1_impl.cuh:231-295](csrc/kernel_v1_impl.cuh#L231-L295),
[src/flatten.rs:392-427](src/flatten.rs#L392-L427)). The DSP is the same shape:
124 input bits in, a native `int64_t` datapath, 48 registered output bits out.
This plan generalises that one-off SRAM path into a reusable macro substrate and
lands DSP48E2 as its first kind.

**Intended outcome:** a design containing `p <= a*b + p` maps to one macro
endpoint instead of a multiplier's worth of AIG cone, evaluated in a handful of
GPU instructions with no warp divergence.

---

## Blocking prerequisite

`eda-infra-rs/` is **empty** — the submodule was never initialised, and every path
dependency in [Cargo.toml](Cargo.toml) points into it. Nothing builds right now.

```sh
git submodule update --init --recursive
cargo build              # no features: lib + macro_test + naive_sim + level_test
```

`cargo build` without `--features cuda` must succeed before any of the work below
is verifiable — that is the whole CPU-only verification story.

---

## Design decisions

**Macro classes.** Two kinds of macro exist in the required-primitives list, and
they need different scheduling. Part A only needs the first, but the substrate
must name the distinction so B and C don't force a redesign:

- **Class R (registered output)** — every output bit comes from clocked state, so
  the macro is a *cut at the cycle boundary*: its inputs are boomerang endpoints,
  its outputs are AIG sources next cycle. It can never combinationally feed
  another macro in the same cycle, so no new DAG cycles are possible and the
  levelization equations stay acyclic. **DSP48E2 with PREG=1 is Class R.**
- **Class C (combinational output)** — outputs depend combinationally on inputs,
  so the macro sits *inside* a combinational cone and needs an intra-cycle
  evaluation slot. CARRY4 (part B) is entirely Class C; SRLC32E's addressed read
  port (part C) is Class C while its `Q31` cascade port is Class R.

This plan builds Class R only, and reserves `MacroClass` in the enum so Class C is
an additive change later. This is why the problem statement's PREG=1 /
everything-else-combinational constraint matters: it is exactly what makes the DSP
Class R.

**Levelized-DAG extension.** A Class R macro's schedule level is
`max(level_id[b] for b in its 124 input bits)` — the level at which its *last*
input bit is ready. That is already what
[src/staging.rs:130-136](src/staging.rs#L130-L136) computes for any endpoint group
via `for_each_input`; the "extension" is making `for_each_input` enumerate macro
inputs and making the partitioner/boomerang account for macro resources. All 124
bits belong to one endpoint group, so they are guaranteed co-partitioned — that is
the "grouping mixed-width operations" requirement, satisfied structurally rather
than by a new heuristic.

**Divergence-free evaluation.** Different macros in the same warp can carry
different `OPMODE_S` values, so a `switch` on opmode would diverge. The datapath is
therefore written entirely branchlessly with sign-masks (`-(int64_t)(state==2)`),
computing all three ALU results and selecting with AND/OR. Every lane executes an
identical instruction stream.

**4 lanes per macro.** The DSP's 124 input bits pack into exactly 4×32 bits, which
matches the SRAM's existing 4-lane granularity and — because 4 divides 32 — guarantees
a macro's lanes never straddle a warp boundary, so `__shfl_sync` gathering is safe.

**64-bit alignment.** `P` is allocated as a 2-word slot at an even word offset so
`(const unsigned long long *)(input_state + io_offset + wo_base_macro)` is 8-byte
aligned and the accumulator feedback is a single `LDG.64`.

---

## Data layout specification

### Macro input packing — 4 permute words per DSP (124 bits)

| lane | bits 0..                          | remaining bits                                        |
|------|-----------------------------------|-------------------------------------------------------|
| 0    | `A[26:0]` @ 0..26                 | `OPMODE_S[1:0]` @ 27..28, `USE_D` @ 29, `RSTP` @ 30   |
| 1    | `D[26:0]` @ 0..26                 | `C[36:32]` @ 27..31                                   |
| 2    | `B[17:0]` @ 0..17                 | `C[47:37]` @ 18..28                                   |
| 3    | `C[31:0]` @ 0..31                 | —                                                     |

`CEP` is deliberately **not** in these words: like the SRAM's `port_r_en_iv`, the
clock enable rides the existing `clken_permute` commit-gating path.

### Per-partition write-out (I/O) region

Current layout is `[normal][duplicates][sram]` with bases derived by subtracting
from `num_ios` ([csrc/kernel_v1_impl.cuh:289-294](csrc/kernel_v1_impl.cuh#L289-L294)).
That arithmetic breaks once a fourth region is appended, so replace it with
**explicit bases in the metadata**:

```
[0,            N)              N = num_normal_writeouts   (boomerang results)
[wo_base_dup,  +Dup)           duplicated outputs
[wo_base_sram, +S)             SRAM read data, 1 word each
[wo_base_macro,+2*M)           macro state, 64-bit aligned pairs   <-- new
```

`wo_base_macro = round_up_even(wo_base_sram + S)`, and `num_ios = wo_base_macro + 2*M`.
Any alignment hole is harmless: `clken_set0` defaults to `u32::MAX` and `clken_inv`
to `0`, so an untouched slot commits `clken == 0` and preserves the old word
([src/flatten.rs:382-386](src/flatten.rs#L382-L386)).

For absolute alignment, `state_start` must itself be even for any partition holding
macros, and `reg_io_state_size` (the per-cycle stride) must be rounded up to even so
alignment survives across cycles.

### Metadata words (currently `[0..8)` used, `[8..128)` padding)

| slot | meaning                                    |
|------|--------------------------------------------|
| 8    | `num_macros`                               |
| 9    | `wo_base_dup`                              |
| 10   | `wo_base_sram`                             |
| 11   | `wo_base_macro`                            |

### Permute-slot region

`sram_duplicate_permute` slots become `[srams ×4][macros ×4][duplicates ×1]`, so
`dup_perm_pos` in [src/flatten.rs:353](src/flatten.rs#L353) shifts by `num_macros*4`,
and the kernel's duplicate branch bound at
[csrc/kernel_v1_impl.cuh:293](csrc/kernel_v1_impl.cuh#L293) shifts likewise.

---

## Work breakdown

### 1. `src/hwmacro.rs` (new) — the substrate

The single source of truth for macro shape, packing, and semantics. Everything
else references it, so `MacroKind::Carry4` later touches only this file plus one
kernel function.

```rust
pub enum MacroClass { Registered, Combinational }   // Combinational unused in part A
pub enum MacroKind  { Dsp48e2 }

impl MacroKind {
    fn class(self) -> MacroClass;
    fn num_input_bits(self) -> usize;    // 124
    fn num_output_bits(self) -> usize;   // 48
    fn num_perm_words(self) -> usize;    // 4
    fn num_state_words(self) -> usize;   // 2
    /// canonical input index -> bit slot in the 4x32 packed words (table above)
    fn input_bit_slot(self, i: usize) -> usize;
}

pub struct MacroBlock {                  // serde, like RAMBlock
    pub kind: MacroKind,
    pub inputs_iv: Vec<usize>,   // canonical order, invert bit in LSB
    pub outputs:   Vec<usize>,   // canonical order, no invert
    pub en_iv:     usize,        // clock-edge & (CEP | RSTP)
}

/// CPU twin of `gem_eval_dsp48e2` in csrc/macro_eval.cuh — keep line-for-line in sync.
pub fn eval_dsp48e2(w: [u32; 4], p_cur: u64) -> u64;
```

Canonical input order: `A[0..27], D[0..27], B[0..18], C[0..48], OPMODE_S[0..2], USE_D, RSTP`.

`eval_dsp48e2` semantics (mirrors the CUDA version exactly):

- sign-extend `A`/`D` from 27, `B` from 18, `C` from 48, `p_cur` from 48
- pre-adder `AD = sext27((A + (D & -USE_D)) & 0x7ffffff)` — wraps at 27 bits, as
  the real DSP48E2 pre-adder does, which keeps `M = AD * B` exactly 45 bits
- `M = AD * B`
- branchless ALU select: `s1=-(state==1)`, `s2=-(state==2)`, `s0=~(s1|s2)`;
  `P_next = (s0 & C) | (s1 & M) | (s2 & (P + M))`
- synchronous reset: `P_next &= ~(-(RSTP))`
- return `P_next & 0xffff_ffff_ffff` (48-bit wrap; OVERFLOW/UNDERFLOW ignored per spec)

### 2. `src/aig.rs` — macros in the AIG

Mirror the `RAMBlock` pattern throughout:

- `DriverType::Macro(usize)` — cell id; output pins are AIG sources, exactly like
  `DriverType::SRAM` ([src/aig.rs:88-106](src/aig.rs#L88-L106))
- `EndpointGroup::Macro(&'i MacroBlock)`, with `for_each_input` enumerating
  `en_iv >> 1` plus every `inputs_iv[i] >> 1`
  ([src/aig.rs:60-83](src/aig.rs#L60-L83))
- `AIG::macros: IndexMap<usize, MacroBlock>`
- `num_endpoint_groups` / `get_endpoint_group`: **append macros after SRAMs**
  ([src/aig.rs:640-654](src/aig.rs#L640-L654)). Appending keeps existing
  `.gemparts` files valid for designs with zero macros.
- `dfs_netlistdb_build_aig`: a `GEM_DSP48E2` arm alongside the `$__RAMGEM_SYNC_`
  arm at [src/aig.rs:367-374](src/aig.rs#L367-L374) — each `P[k]` output pin gets
  `add_aigpin(DriverType::Macro(cellid))` recorded into `outputs[k]`
- clock-trace loop at [src/aig.rs:449-472](src/aig.rs#L449-L472): add `GEM_DSP48E2`
  to the cell-type match (`"CLK"` is already in the pin list)
- post-pass beside the SRAM one at [src/aig.rs:525-567](src/aig.rs#L525-L567):
  collect `A/D/B/C/OPMODE_S/USE_D/RSTP` into `inputs_iv`, and build
  `en_iv = and(trace_clock_pin(CLK), or(CEP, RSTP))` using the existing
  `add_and_gate` De Morgan idiom (`add_and_gate(x^1, y^1) ^ 1`), matching how
  `port_w_wr_en_iv` is built at [src/aig.rs:559-565](src/aig.rs#L559-L565).
  `RSTP` has precedence over `CEP` on the real part, which is why it is OR'd into
  the commit enable and *also* zeroes the data inside the macro.

### 3. `src/aigpdk.rs` — leaf pin declarations

Add `GEM_DSP48E2` arms to `direction_of` and `width_of`
([src/aigpdk.rs:26-94](src/aigpdk.rs#L26-L94)): `CLK`/`USE_D`/`CEP`/`RSTP` scalar
inputs; `A`/`D` `[26:0]`, `B` `[17:0]`, `C` `[47:0]`, `OPMODE_S` `[1:0]` input
buses; `P` `[47:0]` output bus.

### 4. `src/pe.rs` — partition resource accounting

In `Partition::build_one` ([src/pe.rs:508-562](src/pe.rs#L508-L562)), count
`num_macros` alongside `num_srams` and update both constraints:

- permute slots: `num_srams*4 + num_macros*4 + num_output_dups <= 256`
- reserved write-outs: `num_srams + 2*num_macros + 1 + ceil(num_dups/32)`
  (the `+1` covers the alignment hole)

`build_one_boomerang_stage` itself needs no change — macro output pins are
non-`AndGate` drivers, so `topo_traverse_generic` already stops at them and
levelization already assigns them level 0.

Optionally weight macro endpoints in
[src/repcut.rs:160-166](src/repcut.rs#L160-L166) so the partitioner does not pile
macros into one block; and add macro cost to `executor_fixed_cost` in the masonry
layout at [src/flatten.rs:739-752](src/flatten.rs#L739-L752).

### 5. `src/flatten.rs` — the heterogeneous allocator

- `FlatteningPart`: add `num_macros`, `wo_base_dup`, `wo_base_sram`,
  `wo_base_macro`; compute them in `init_afters_writeouts`
  ([src/flatten.rs:229-292](src/flatten.rs#L229-L292)) and use the named bases
  everywhere instead of the current `num_writeouts - num_srams - ...` subtractions.
- State allocation loop at [src/flatten.rs:769-776](src/flatten.rs#L769-L776):
  round `sum_state_start` up to even before assigning `state_start` to a partition
  that holds macros; round `reg_io_state_size` up to even at the end.
- `make_inputs_outputs` ([src/flatten.rs:371-465](src/flatten.rs#L371-L465)): a
  `EndpointGroup::Macro` arm modelled on the `RAMBlock` arm —
  - place the 124 input bits with
    `place_sram_duplicate(macro_perm_st + kind.input_bit_slot(i), query_permute_with_pin_iv(inputs_iv[i]))`
  - for each output bit `k`: `input_map.insert(outputs[k], base*32 + k)`,
    `output_map.insert(outputs[k] << 1, ...)`, and
    `place_clken_datainv(local*32 + k, <perm of en_iv>, ..., 0)` — the same
    construction the SRAM uses for `port_r_en_iv` at
    [src/flatten.rs:395-405](src/flatten.rs#L395-L405)
- `build_script` ([src/flatten.rs:474-482](src/flatten.rs#L474-L482)): emit the
  four new metadata words.
- Extend the module doc-comment at [src/flatten.rs:21-108](src/flatten.rs#L21-L108)
  with the I/O-region table above — that comment is the de-facto script ABI spec.

### 6. `src/emulate.rs` (new) — CPU script emulator, lifted out of the CUDA binary

`simulate_block_v1` currently lives inside
[src/bin/cuda_test.rs:170-425](src/bin/cuda_test.rs#L170-L425), which is gated
behind `required-features = ["cuda"]`. Move it verbatim into a new `src/emulate.rs`
library module and have `cuda_test` call it for `--check-with-cpu`.

**This refactor is what makes CPU-only verification possible** — without it there
is no way to exercise the script format on a machine with no CUDA. Do it as a
pure move first (no behaviour change), then add the macro path.

Then add the macro evaluation, mirroring the SRAM block at
[src/bin/cuda_test.rs:347-368](src/bin/cuda_test.rs#L347-L368):

```rust
for m_i in 0..num_macros {
    let base = (num_srams * 4 + m_i * 4) as usize;
    let w = [perm[base], perm[base+1], perm[base+2], perm[base+3]];
    let p_off = (io_offset + wo_base_macro) as usize + m_i * 2;
    let p_cur = (input_state[p_off] as u64) | ((input_state[p_off+1] as u64) << 32);
    let p_next = eval_dsp48e2(w, p_cur);
    writeouts[wo_base_macro as usize + m_i*2]     = p_next as u32;
    writeouts[wo_base_macro as usize + m_i*2 + 1] = (p_next >> 32) as u32;
}
```

### 7. `csrc/macro_eval.cuh` (new) + `csrc/kernel_v1_impl.cuh`

New header holding the branchless datapath, marked `__host__ __device__` so the
same code is host-unit-testable once nvcc is available:

```cuda
__device__ __forceinline__ long long gem_sext(unsigned long long v, int w) {
  return ((long long)(v << (64 - w))) >> (64 - w);
}

__device__ __forceinline__ unsigned long long gem_eval_dsp48e2(
    u32 w0, u32 w1, u32 w2, u32 w3, unsigned long long p_cur)
{
  long long a = gem_sext(w0 & 0x07ffffffu, 27);
  long long d = gem_sext(w1 & 0x07ffffffu, 27);
  long long b = gem_sext(w2 & 0x0003ffffu, 18);
  unsigned long long c_raw = (unsigned long long)w3
      | ((unsigned long long)(w1 >> 27) << 32)
      | ((unsigned long long)((w2 >> 18) & 0x7ffu) << 37);
  long long c = gem_sext(c_raw, 48);
  long long p = gem_sext(p_cur, 48);

  u32 state = (w0 >> 27) & 3u, use_d = (w0 >> 29) & 1u, rstp = (w0 >> 30) & 1u;

  long long ad = gem_sext(((unsigned long long)(a + (d & -(long long)use_d)))
                          & 0x07ffffffull, 27);
  long long m  = ad * b;                       // 45-bit product

  long long s1 = -(long long)(state == 1u);
  long long s2 = -(long long)(state == 2u);
  long long s0 = ~(s1 | s2);
  long long pn = (s0 & c) | (s1 & m) | (s2 & (p + m));
  pn &= ~(-(long long)rstp);
  return (unsigned long long)pn & 0x0000ffffffffffffull;
}
```

Kernel integration in `simulate_block_v1`, threading macro lanes into the existing
epilogue at [csrc/kernel_v1_impl.cuh:231-296](csrc/kernel_v1_impl.cuh#L231-L296):

1. Read the four new metadata words alongside the existing ones at
   [csrc/kernel_v1_impl.cuh:43-53](csrc/kernel_v1_impl.cuh#L43-L53).
2. **Issue the `P` feedback load early**, next to the SRAM read at
   [csrc/kernel_v1_impl.cuh:250-252](csrc/kernel_v1_impl.cuh#L250-L252), so its
   latency overlaps the clock-enable permutation the same way the SRAM read
   already does. All 4 lanes of a macro load the same `u64` — redundant but
   divergence-free, and a warp's 8 macros touch 64 contiguous bytes, so it is a
   perfectly coalesced pair of sectors.
3. Gather the four packed words with **full-mask** shuffles at uniform block
   scope (not inside the lane-range `if`), so every lane of every warp
   participates:
   ```cuda
   int lane0 = (threadIdx.x & 31) & ~3;
   u32 w0 = __shfl_sync(0xffffffff, sram_duplicate_t, lane0 + 0);
   /* w1, w2, w3 likewise */
   ```
4. Evaluate and store into `shared_writeouts` in the same block as the SRAM commit
   at [csrc/kernel_v1_impl.cuh:286-296](csrc/kernel_v1_impl.cuh#L286-L296) — this
   ordering matters, because both permutation gathers read `shared_writeouts`
   *before* this point. Lanes `sub < 2` of each macro store `P` lo/hi as a
   predicated store, not a branch.
5. Shift the duplicate-lane range bound by `num_macros * 4`.

The commit at [csrc/kernel_v1_impl.cuh:303-307](csrc/kernel_v1_impl.cuh#L303-L307)
needs no change: `num_ios` now includes the macro words and their clock-enable
gating is already in `clken_permute`.

### 8. Frontend — Yosys inference of `a*b+c`

Per the chosen approach, MAC instances are **inferred from ordinary RTL
arithmetic**, not hand-instantiated. Rather than write a pattern matcher, reuse
Yosys's own well-tested Xilinx DSP inference to produce real `DSP48E2` cells, then
techmap those into our cell. New `aigpdk/` files:

- **`gemmacro.v`** — `GEM_DSP48E2` behavioural model (for reference simulation)
  plus a `(* blackbox *)` declaration for Yosys.
- **`gemmacro_map.v`** — the techmap: `DSP48E2 -> GEM_DSP48E2`. It does the
  "extract the intent, pass a simplified 2-bit state" job:
  ```verilog
  wire [2:0] Z = OPMODE[6:4];
  wire [1:0] X = OPMODE[1:0];
  wire mul = (X == 2'b01);
  wire [1:0] S = (Z == 3'b010 && mul) ? 2'd2      // multiply-accumulate
               : (Z == 3'b000 && mul) ? 2'd1      // multiply-only
               :                        2'd0;     // bypass, P = C
  ```
  Written as combinational logic rather than a parameter lookup, so a *dynamic*
  `OPMODE` synthesises into AIG gates feeding `OPMODE_S` — which is precisely the
  heterogeneous case: 1-bit boolean control computed by the boomerang feeding a
  word-level macro in the same schedule.

  The wrapper must also reconcile Yosys's register packing with the mandated
  configuration:
  - `PREG == 1` is **required**. If Yosys packed no output register the multiply is
    combinational, which would make the macro Class C — out of scope for part A.
    Error with a message telling the user to register the accumulator.
  - `AREG/BREG/CREG/DREG/ADREG/MREG != 0`: **unpack** them into explicit aigpdk
    `DFF` instances in front of the macro (generate loops over the bus widths), so
    the mandated all-combinational-inputs configuration always holds inside the
    macro while extra pipelining is still simulated correctly as ordinary flops.
    If this proves fiddly, erroring out is an acceptable first cut — but unpacking
    materially widens the set of designs that map.
- **liberty entry** — a `GEM_DSP48E2` cell appended to `aigpdk.lib` with
  `dont_use / dont_touch / map_only / is_macro_cell : TRUE`, copying the
  `$__RAMGEM_SYNC_` entry at [aigpdk/aigpdk.lib:641-660](aigpdk/aigpdk.lib#L641-L660)
  so DC and `abc -liberty` leave instances intact.

New synthesis step for [usage.md](usage.md), between the existing Step 1 (memory)
and Step 2 (logic):

```tcl
# after memory_libmap, before aigpdk logic mapping
wreduce; peepopt; opt_clean; alumacc; share; opt

techmap -map +/mul2dsp.v \
  -D DSP_A_MAXWIDTH=27 -D DSP_B_MAXWIDTH=18 \
  -D DSP_A_MINWIDTH=2  -D DSP_B_MINWIDTH=2 \
  -D DSP_NAME=$__MUL27X18
opt_expr -fine; wreduce; opt_clean

techmap -map +/xilinx/xcup_dsp_map.v    # $__MUL27X18 -> DSP48E2
xilinx_dsp -family xcup                 # fold pre-adder / post-adder / PREG in

techmap -map path/to/aigpdk/gemmacro_map.v   # DSP48E2 -> GEM_DSP48E2
```

> Pass and map-file names drift between Yosys releases (`xcup_dsp_map.v` vs
> `xc7_dsp_map.v`, `-family` spellings). Pin the version used and validate this
> recipe against the installed Yosys as the first frontend task — this is the
> highest-uncertainty item in the plan.

### 9. `src/bin/naive_sim.rs` — golden model

Add a `GEM_DSP48E2` arm to the cell dispatch at
[src/bin/naive_sim.rs:403-503](src/bin/naive_sim.rs#L403-L503), reusing
`hwmacro::eval_dsp48e2` on bits gathered straight from the netlist pins. This
gives an in-repo, script-independent reference — it exercises the *netlist*
semantics, whereas the emulator exercises the *script*, so agreement between them
is a genuine cross-check.

---

## Verification (no GPU required)

`--check-with-cpu` in `cuda_test` compares GPU against CPU and is unavailable here,
so the CPU emulator becomes the primary gate.

**1. Unit tests on `eval_dsp48e2`** (`#[cfg(test)]` in `src/hwmacro.rs`) against a
hand-written reference, covering the cases most likely to be wrong:

- all three `OPMODE_S` states, `USE_D` on and off, `RSTP` on and off
- signed extremes: `A = -2^26`, `B = -2^17`, `C = -2^47`
- 27-bit pre-adder wrap (`A + D` overflowing 27 bits)
- 48-bit accumulator wrap (repeated `P += M` past `2^47`)
- round-trip through `input_bit_slot` packing/unpacking for all 124 bits

**2. `src/bin/macro_test.rs`** (new, no `cuda` feature) — end-to-end differential:

- read a small hand-written gate-level `.gv` checked into `tests/data/` — a MAC
  accumulator plus surrounding `AND2_*`/`INV`/`DFF` logic and one `GEM_DSP48E2`.
  Hand-writing it avoids a Yosys dependency for the core correctness gate.
- `AIG::from_netlistdb` → `build_staged_aigs` → `RCHyperGraph` + `Partition::build_one`
  → `FlattenedScriptV1::from` (the same pipeline as
  [src/bin/cut_map_interactive.rs:64-141](src/bin/cut_map_interactive.rs#L64-L141))
- drive N random input vectors through `emulate::simulate_block_v1` and through the
  `naive_sim` behavioural evaluator; assert bit-exact agreement on every primary
  output and every `P` bit, every cycle
- include a design with **zero** macros to prove the refactor is behaviour-preserving

**3. Regression on the existing path.** Run `macro_test` on a macro-free netlist
and confirm `FlattenedScriptV1.blocks_data`'s hash is unchanged from before the
change — `cuda_test` already prints exactly this hash
([src/bin/cuda_test.rs:467-471](src/bin/cuda_test.rs#L467-L471)). This catches any
accidental perturbation of the script ABI.

**4. Frontend smoke test** (once Yosys is confirmed): synthesise a small
`always @(posedge clk) p <= a*b + p;` through the Step 1.5 recipe, confirm
`GEM_DSP48E2` instances appear with `OPMODE_S` driven as expected, and feed the
result through `macro_test`.

**5. Deferred to GPU bring-up.** `csrc/` changes cannot be compiled or run here.
Mitigate by keeping `gem_eval_dsp48e2` (CUDA) and `eval_dsp48e2` (Rust)
line-for-line parallel with a cross-reference comment in both, so the tested Rust
version is a faithful proxy. On first GPU access: `cargo build --features cuda`,
then `cuda_test --check-with-cpu` on the macro netlist, then `compute-sanitizer`
for the new shuffles and the 64-bit loads.

---

## Deferred / known limits (state these in the PR)

- **Class C macros are not implemented.** CARRY4 (part B) and SRLC32E's addressed
  read port (part C) need an intra-cycle macro evaluation slot inside the boomerang,
  or a forced major-stage split. The `MacroClass` enum reserves the distinction; the
  scheduler work is genuinely new.
- **`PREG=0` designs fall back to AIG gates.** A purely combinational `a*b` is
  Class C and is left to the boomerang. In spec for part A, which mandates PREG=1.
- **`OVERFLOW`/`UNDERFLOW` pins are not modelled**, per the problem statement. `P`
  wraps at 48 bits.
- **The wide-multiplier tiling path is untested.** `mul2dsp.v` splits multiplies
  wider than 27×18 into partial products with cascade paths; whether Yosys's
  cascading survives our `DSP48E2 -> GEM_DSP48E2` rewrite needs checking. Start
  with designs whose operands fit one tile.
