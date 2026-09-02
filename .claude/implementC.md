# Part C implementation log — Native SRLC32E shift-register-LUT macro in GEM

Tracks the implementation of [.claude/planC.md](planC.md) (the SRLC32E
primitive: the first macro that is **Class R and Class C at once**, decomposed
into the two classes that already exist rather than into a third).

## Environment

- Windows 11, MSVC toolchain, `cargo` at `~/.cargo/bin` (prefix
  `export PATH="$HOME/.cargo/bin:$PATH"`) — CPU-only verification.
- **WSL2 Ubuntu-24.04** (same box as Parts A and B): **Yosys 0.68**
  (`git sha1 38e001a6f`, built from `~/yosys`, binary `~/yosys/build/yosys`)
  — the pinned version for this project; `/usr/bin/yosys` is the Ubuntu 0.33
  package and must not be used. Plus a dpkg-extracted CUDA 12.6 `nvcc`
  (`~/cudaenv.sh` sets `CUDA_HOME` etc.) and an RTX 2050 (`sm_86`). This is
  where the Yosys frontend and the GPU numeric differential were validated.
  Repo used in place at `/mnt/c/Users/Devansh/GEM`; Linux builds go to
  `CARGO_TARGET_DIR=~/gem-target`.
- Parts A and B are landed (uncommitted in the working tree) and green.

## Status — everything landed and validated

`cargo test` = **18 lib + 14 `macro_test`** green on Windows/MSVC **and**
Linux; `cargo build --features cuda` clean; GPU `cuda_test --check-with-cpu`
passes on every Part C design; `naive_sim` and the GPU agree bit-exactly on
every primary output of every Part C design.

| # | Item | Status |
|---|------|--------|
| 1 | `src/hwmacro.rs` — `Srl32Shift`/`Srl32Read` kinds, layouts, branchless datapaths, unit tests | ✅ 5 SRL unit tests (planC asked for 4) |
| 2 | `src/aig.rs` — SRLC32E parse arm, clock trace, post-pass, synthetic cell ids, endpoint order | ✅ macro_test + `srl_structural_checks` + Yosys-output parse |
| 3 | `src/aigpdk.rs` — SRLC32E leaf pins | ✅ matches the Xilinx unisim port list and Yosys's emitted instance |
| 4 | `src/staging.rs` — **no change** (the headline result) | ✅ **zero lines changed**, verified by 6 differential fixtures + structural asserts |
| 5 | `src/pe.rs` — accounting | ✅ **zero lines changed**, as planC predicted |
| 6 | `src/flatten.rs` — Class C kind tags + Class R kind table | ✅ ABI regression (6 pinned hashes) + GPU differential |
| 7 | `csrc/macro_eval.cuh` + `csrc/kernel_v1_impl.cuh` + `src/emulate.rs` | ✅ **nvcc 12.6 clean (101 regs, 0 spill, 1 barrier, 3328 B smem — identical to Part B) and the GPU-vs-CPU differential passes on the RTX 2050** |
| 8 | Frontend — `aigpdk/gemsrl.v`, `gemsrl_map.v`, liberty, `usage.md` Step 1.7 | ✅ Yosys 0.68 end-to-end, both paths; `SRL16E -> SRLC32E` techmap exercised; **0.33's dropped-`CE` defect is gone in 0.68** |
| 9 | `src/bin/naive_sim.rs` — SRLC32E golden arm | ✅ runs, and its VCD matches the GPU's on all 4 designs |
| V | `src/bin/macro_test.rs` + `tests/data/*.gv` — differential harness | ✅ 6 new differential fixtures + 1 new structural check |

## Test results (`cargo run --bin macro_test`)

```
[simple_dff]                  1 stage,  0 macro   -> PASS  hash 4367312310778971198 (pinned, unchanged)
[mac_accumulator]             1 stage,  1 macro   -> PASS  hash 1388458641175822285  (pinned, unchanged)
[ripple_adder]                2 stages, 1 macro   -> PASS  hash 2711449478675204136  (pinned, unchanged)
[carry_then_logic_then_carry] 3 stages, 2 macros  -> PASS  hash 17676642068319116734 (pinned, unchanged)
[mixed_macros]                2 stages, 2 macros  -> PASS  hash 9165969952107539029  (pinned, unchanged)
[yadder]                      2 stages, 1 macro   -> PASS  hash 2719296911916927336  (pinned; tracks the Yosys version)
[srl_static_addr]             2 stages, 2 macros  -> PASS  400 cyc
[srl_dynamic_addr]            2 stages, 2 macros  -> PASS  400 cyc
[srl_cascade_q31]             1 stage,  2 macros  -> PASS  400 cyc  <-- zero Class C, zero splits
[srl_ce_gating]               2 stages, 2 macros  -> PASS  400 cyc
[heterogeneous]               3 stages, 4 macros  -> PASS  400 cyc  <-- DSP + CARRY4 + SRL
[ysrl]                        2 stages, 2 macros  -> PASS  400 cyc  <-- Yosys 0.68 frontend
[structural]                  Class C staging checks passed
[structural]                  Part C SRLC32E decomposition checks passed
```

Every case is bit-exact on every primary output, every DSP `P` bit **and every
one of the 32 SRL state bits**, every cycle, through
`emulate::simulate_block_v1` (major stage by major stage, the kernel's scan
order) *and* through the independent `Behavioural` gate-level evaluator.

**All six pre-existing hashes are now pinned assertions**, not claims — Part
C's ABI-neutrality (design decision 5) is enforced by the test suite. planC
asked for exactly this.

One caveat on reading that table: five of the six pins are *ABI* pins and must
never move. `yadder`'s is different — its netlist **is** Yosys output, so it is
regenerated whenever the pinned Yosys version changes. It moved from
`299385802866267780` (0.33's netlist) to `2719296911916927336` (0.68's) purely
because cell ordering shifted; the design, the stage count, the macro count and
the differential are identical, and the five ABI pins did not move. This is
documented at the constant in `macro_test.rs`.

## GPU numeric differential (WSL, RTX 2050)

```
cargo build --features cuda                                  clean
nvcc 12.6, sm_86:  101 registers, 0 bytes spill, 1 barrier, 3328 B smem
                   (byte-for-byte the same as Part B — the branchless
                    dual-datapath Class R select cost nothing)

cut_map_interactive <design>.gv -> <design>.gemparts          num_parts == 1
cuda_test <design>.gv <gemparts> <stim.vcd> 1 --check-with-cpu

  srl_dynamic_addr   81 cyc -> "sanity test passed!"  hash 14037616067786070230 (== macro_test)
  srl_cascade_q31    81 cyc -> "sanity test passed!"  hash 8673665729173642157  (== macro_test)
  srl_ce_gating      81 cyc -> "sanity test passed!"  hash 9935050838755082729  (== macro_test)
  heterogeneous      81 cyc -> "sanity test passed!"  hash 8082681192424292305  (== macro_test)
```

Every script hash is identical between `cut_map_interactive` + `cuda_test` and
the standalone `macro_test` harness, so the auto-derived split set is
deterministic and consistent across entry points — the same property Part B
established. The GPU run exercises the whole new kernel surface: the branchless
Class R kind select, `gem_eval_srl32_shift`, `gem_eval_srl32_read`, the tagged
Class C metadata decode, and the 32 state bits riding the boomerang.

`compute-sanitizer` is still **not** in this partial CUDA install (the
extracted `~/cuda/.../bin` holds only `nvcc`/`ptxas`/`nvlink`/`cudafe++`/
`fatbinary`/`bin2c`), so the memcheck/racecheck pass over the Class C
shared-memory gather remains open — the same gap Parts A and B left.

## `naive_sim` <-> emulator/GPU cross-check — **closes the item Parts A and B left open**

planC verification item 5 asked for this, and it is now done for all three
primitives at once. Both simulators were driven from the same stimulus VCD and
their output VCDs compared signal-by-signal:

```
naive_sim  <design>.gv <stim>.vcd <design>.naive.vcd
cuda_test  <design>.gv <gemparts> <stim>.vcd <design>.out.vcd 1 --check-with-cpu

  srl_dynamic_addr   2 signals,  160 samples, 0 mismatches
  srl_cascade_q31    1 signal,    80 samples, 0 mismatches
  srl_ce_gating      2 signals,  160 samples, 0 mismatches
  heterogeneous      8 signals,  640 samples, 0 mismatches   <-- DSP + CARRY4 + SRL
```

The comparison applies a **constant one-clock-period shift** and nothing else:
`naive_sim` latches at edge *T* and then propagates, dumping at *T*, whereas
GEM's cycle *N* is the interval *just before* edge *N*. That is the documented
convention offset (design decision 2), it is the same for every signal and
every design, and with it applied the two agree on every sample.

## Key design decisions / deviations from planC.md

### The plan's central claim held: `staging.rs` and `pe.rs` needed **zero** changes

This was planC's headline bet — that decomposing SRLC32E into an existing
Class R endpoint plus an existing Class C endpoint, joined by 32 ordinary AIG
source pins, would leave the boomerang scheduler untouched. It did:

- `macro_levels` ([src/staging.rs:261-311](../src/staging.rs#L261-L311))
  enumerates Class C macros generically. The 32 state pins are
  `DriverType::Macro` sources seeded at level 0, so `macro_level[Srl32Read]`
  comes out as exactly "the level at which `A` is ready" with no new equation.
  `srl_static_addr` (constant `A`) lands at level 0; `srl_dynamic_addr`
  (`ra ^ rb`) at level 2; `heterogeneous` (address off a CARRY4) at level 3,
  one above the CARRY4's level 2 — the macro→macro fixpoint case, asserted.
- `Partition::build_one` ([src/pe.rs:539-566](../src/pe.rs#L539-L566)) needed
  nothing because `Srl32Shift` keeps the uniform 4-lane / 2-word Class R
  footprint (design decision 3).

### `Q31` really is free, and the cascade fast path really is zero-cost

`pin2aigpin_iv[Q31] = state[31] << 1` — the cascade port *is* the Class R
source pin, so it is not a macro output, needs no evaluation and forces no
split. `srl_cascade_q31.gv` (two SRLs chained `Q31 -> D`, `Q` unconnected on
both) produces **zero Class C macros and exactly one major stage**, asserted in
`srl_structural_checks`. Both the emulator and the GPU run it at one grid sync
per cycle, identical to a macro-free design.

Detecting "Q unconnected" is done before the DFS by checking
`netlistdb.net2pin.len(net_of_Q) > 1`. In practice Yosys/hand-written netlists
that omit `.Q(...)` create no `Q` pin at all, which the same scan handles.

### Metadata is additive and provably ABI-neutral

- **Class C kind tag**: `[16 + i] = kind << 28 | payload`, `kind 0 = Carry4`
  (`payload = chain_len`), `kind 1 = Srl32Read`. Tag 0 makes every CARRY4 word
  numerically identical to Part B's.
- **Class R kind table** at `[16 + num_classc_macros + j]`, `0 = Dsp48e2`,
  `1 = Srl32Shift`. It is emitted whenever `num_macros > 0`, and for an all-DSP
  partition every word is `0` — which is exactly the metadata padding that
  already sat there. Hence `mac_accumulator` and `mixed_macros` hash
  identically to before Part C.
- The `<= 128` bound is now
  `16 + num_classc_macros + num_macros <= 128`, asserted before any of the
  section is emitted (so it cannot be tripped after a partial write).

### Divergence-free Class R dispatch (planC decision 4, implemented as written)

Two Class R kinds now share one epilogue slot. The kernel computes **both**
datapaths on every lane and selects with a mask:

```cuda
u32 kind = shared_metadata[16 + num_classc_macros + m_i];
unsigned long long pn_dsp = gem_eval_dsp48e2(w0, w1, w2, w3, macro_p_cur);
unsigned long long pn_srl = gem_eval_srl32_shift(w0, macro_p_cur);
unsigned long long sel = 0ULL - (unsigned long long)(kind == GEM_MACRO_SRL32_SHIFT);
unsigned long long pn  = (pn_dsp & ~sel) | (pn_srl & sel);
```

`src/emulate.rs` mirrors it line-for-line (including the mask, not a branch) so
the two stay auditable against each other. The unit test
`srl32_shift_datapath_is_branchless_select_safe` feeds each datapath the
other's lane shapes to confirm both are total — a mask select evaluates both,
so neither may trap on foreign input. Measured cost: **0 extra registers, 0
spill** versus Part B.

The Class C dispatch sits inside the already-predicated head-lane branch; the
`perm_words`/`state_words` stride walk stays uniform across the block via
`gem_classc_{perm,state}_words(tag)`.

### `CE` gating rides the write-out clock enable

`en_iv = add_and_gate(clk_iv, ce_iv)`, mirroring the DSP's
`clk & (CEP | RSTP)` minus the reset term (SRLC32E has none). The datapath
therefore stays unconditional — `state' = (state << 1) | D` with no branch —
and `CE == 0` simply holds the committed state, exactly like a disabled DFF.
`srl_ce_gating.gv` drives `CE` from a random primary input and compares all 32
state bits every cycle. A `CE` that resolves to tie-0 raises a
`clilog::warn!(SRL_CE_TIE0, ...)`, the same trap the DSP's `CEP` has.

### The 32 state bits do cross splits as staged IO (as planC predicted)

When the read macro is realized in a later stage than the shift macro — as in
`heterogeneous`, where the address comes off the CARRY4 — the 32 state source
pins are live at the split, so `from_split` turns them into `StagedIOPin`
copies in the earlier stage. That is pre-existing behaviour for any live source
pin (a DFF `Q` included), it is correct (the copy carries the same pre-edge
snapshot), and it is the write-out pressure planC listed as deferred perf work.

## Two real bugs found and fixed while testing (outside planC's scope)

Both were latent before Part C and are fixed here because Part C's fixtures are
the first designs that reach them.

### 1. `src/emulate.rs` — signed-negate overflow in the global-read gather

`let lowbit = mask & (-(mask as i32)) as u32;` panics in a debug build when a
round's mask is exactly `1 << 31` (`-i32::MIN` overflows). The CUDA kernel's
`mask & -mask` on an unsigned `u32` already wraps, so the two had silently
diverged only under `debug_assert`. Now `mask & mask.wrapping_neg()`.
`srl_static_addr` was the first design to produce such a mask.

### 2. `src/flatten.rs` — a primary output driven by a macro state bit resolved to the wrong slot

The Class R arm did `output_map.insert(d << 1, macro_state_bitpos)` for each
output bit. When that same AIG pin is *also* a primary output — SRLC32E's
`Q31 = state[31]` wired straight to a port is the natural case — the macro arm
(endpoint-ordered last) clobbered the `PrimaryOutput` arm's entry, so
`output_map` pointed at the macro's **post-edge** state word instead of the
port's own **pre-edge** write-out slot. `cuda_test` uses `output_map` for
exactly this lookup, so a design like `srl_static_addr` would have been
compared against a reference VCD one cycle out of phase.

Fixed by making the macro arm `output_map.entry(d << 1).or_insert(bitpos)`
while the `PrimaryOutput` arm keeps a plain `insert`, so the primary output
wins in either endpoint order. The differential caught it on cycle 32 —
the first cycle `state[31]` becomes non-zero.

The identical pattern exists in the upstream SRAM arm
(`output_map.insert(d << 1, sram_rd_data_...)` for `PORT_R_RD_DATA`); it is
left untouched, since changing upstream SRAM semantics is out of Part C's
scope. Noted below as a known pre-existing quirk.

## Yosys 0.68 frontend validation

Everything below was re-run against **Yosys 0.68** (`git sha1 38e001a6f`); the
original pass used the distro's 0.33 and is superseded. Part C is where the
version bump actually changes a finding — see Path B.

- `read_verilog aigpdk/gemsrl.v` — the `(* blackbox *)` `SRLC32E` elaborates,
  both with and without the behavioural body.
- `read_verilog aigpdk/gemsrl_map.v` — elaborates clean.
- **Path A (validated, recommended)** — `ysrl.v` (explicit `SRLC32E`,
  registered data and address) through `synth -flatten` +
  `dfflibmap`/`abc -liberty aigpdk_nomem.lib` + `opt_clean`: the `SRLC32E`
  **survives intact** (blackbox + `dont_touch`) alongside `DFF`/`INV` for the
  glue, with `A` connected as a bus — 1 `SRLC32E`, 6 `DFF`, 3 `INV`. The
  resulting `.gv` ([tests/data/yosys_srl.gv](../tests/data/yosys_srl.gv),
  regenerated from 0.68 and checked in) parses in GEM, produces 2 major stages
  and passes the 400-cycle differential — a full frontend → GEM → simulation
  loop. The 0.68 netlist is structurally identical to the 0.33 one it replaces.
- **Path B — now genuinely end-to-end; 0.33's dropped-`CE` defect is fixed.**
  `synth_xilinx -flatten` on plain RTL (`always @(posedge clk) sh <=
  {sh[N-2:0], d};`) infers SRLs with no map file needed, and the depth table is
  the same as 0.33's:

  | RTL depth | Yosys 0.68 emits |
  |---|---|
  | 2–16 | `SRL16E` (scalar `A0..A3`; depth 8 → `A = 4'b0111`) |
  | 18–32 | `SRLC32E` (bus `A[4:0]`; depth 20 → `A = 5'h13`, depth 32 → `5'h1f`) |
  | > 32 | a `Q31 -> D` cascade (depth 48 → `SRLC32E(A=5'h0f)` → `SRL16E(A=4'b1111)`) |

  **The clock enable now survives.** On 0.33, `if (ce) sh <= {...}` came out as
  `SRLC32E` with `.CE(1'b1)` and `ce` left dangling — depth and data right, the
  enable silently lost. On 0.68 the same RTL gives `SRLC32E ... .CE(ce)`, and
  `xilinx_srl`'s own help text now states that a chain must match on "clock,
  clock polarity, **enable**, and enable polarity". The addressed-read form is
  handled too: `if (ce) sh <= {...}; assign q = sh[a];` yields a single
  `SRLC32E` with a **dynamic 5-bit `A` bus and a live `CE`** — which is exactly
  GEM's `Srl32Shift` (Class R) + `Srl32Read` (Class C) decomposition, inferred
  from ordinary RTL with no hand-instantiation.

  Two mechanical caveats stand between that and a GEM-readable netlist, both
  easy to fix in the script:

  1. `synth_xilinx` inserts `IBUF`/`OBUF`/`BUFG` pads. Pass
     `-noiopad -noclkbuf`.
  2. Yosys attaches `#(.INIT(...), .IS_CLK_INVERTED(...))` to the inferred cell
     (and `INIT` can come out as `32'hxxxxxxxx`). **GEM's netlist parser has no
     parameter support at all** — `sverilogparse` fails the whole file with
     `NL_SV_PARSE` on the `#(` token. Strip them before writing:

     ```tcl
     synth_xilinx -flatten -noiopad -noclkbuf
     setparam -unset INIT -unset IS_CLK_INVERTED t:SRLC32E
     setparam -unset INIT -unset IS_CLK_INVERTED t:SRL16E
     techmap -map aigpdk/gemsrl_map.v      ;# SRL16E -> SRLC32E
     write_verilog -noattr design.gv
     ```

     Verified: with the parameters stripped the netlist is
     `SRLC32E _0_ (.A(a), .CE(ce), .CLK(clk), .D(d), .Q(q));` and GEM parses it
     (`level_test`: clock inferred, 42 aigpins at level 0, 1 at level 1). With
     the parameters left on, the identical netlist fails to parse. This caveat
     did not matter in the 0.33 write-up because Path B was not usable anyway;
     now that it is, it is the one thing that will bite a user.
- `aigpdk/gemsrl_map.v`'s `SRL16E -> SRLC32E` rule was **exercised**: an
  `SRL16E` with `A3..A0 = 4'b0111` techmaps to `SRLC32E` with `A = 5'h07`.

  planC guessed at the `$__XILINX_SHREG_` interface; it was corrected against
  the real `techlibs/xilinx/cells_map.v` — ports are
  `C, D, L[31:0], E, Q, SO` (not `SH`), params `DEPTH, INIT, CLKPOL, ENPOL`
  with `ENPOL` `0/1/2` = active-low / active-high / no-enable. `L` is the tap
  index (`DEPTH-1` for the fixed-length form), which is exactly SRLC32E's `A`,
  and `SO` is the cascade shift-out, i.e. `Q31`. **That interface is unchanged
  in 0.68**, so `gemsrl_map.v`'s fallback rule needed no edit. planC also
  assumed `shregmap -tech xilinx`; that option does not exist in 0.68 either
  (`shregmap` only knows `-tech greenpak4`; the Xilinx pass is `xilinx_srl`,
  which `synth_xilinx` runs internally).

## Verification status vs planC.md

1. **`hwmacro` unit tests** — done, 5 not 4:
   `srl32_read_exhaustive_addresses` (all 32 addresses × 8 states),
   `srl32_shift_64_cycles_matches_u32_reference` (random `D` stream vs an
   independent `u32` shift, plus all 32 read addresses every cycle),
   `srl32_q31_is_state_bit_31`, `srl32_packing_roundtrips_every_bit` (plus
   asserts that the Class R footprint stayed uniform), and the extra
   `srl32_shift_datapath_is_branchless_select_safe`. All 13 pre-existing lib
   tests stay green (18 total).
2. **`macro_test` differential** — done, all 6 planned fixtures bit-exact on
   every primary output, every `P` bit and every SRL state bit, every cycle,
   through both evaluators.
3. **Structural assertions** — done (`srl_structural_checks`): one SRLC32E
   yields exactly one `Srl32Shift` (Class R, real cell id) and one `Srl32Read`
   (Class C, synthetic cell id above `netlistdb.num_cells`); all 32 state pins
   exist and are `DriverType::Macro`; the read macro's first 32 inputs *are*
   those pins un-inverted; `Q31`'s aigpin **is** `state[31]`;
   `srl_cascade_q31` has zero Class C macros and one stage; `heterogeneous`
   has all four macro kinds at two distinct Class C levels; Class R endpoints
   still all precede Class C ones.
4. **ABI regression** — done and *strengthened*: all six pre-existing
   `blocks_data` hashes are now pinned constants asserted by `run_case`
   (planC asked for exactly this), and all six are unchanged.
5. **`naive_sim` <-> emulator** — **done** (see above). This closes the
   cross-check Parts A and B both left partially open, and closes it for all
   three primitives via `heterogeneous.gv`.
6. **GPU bring-up** — done: `cargo build --features cuda` clean;
   `cut_map_interactive` + `cuda_test --check-with-cpu` pass on
   `srl_dynamic_addr`, `srl_cascade_q31`, `srl_ce_gating` and
   `heterogeneous`; every script hash matches `macro_test`; register/spill
   unchanged at 101/0.
7. **Yosys 0.68 frontend** — done, both paths (see above); Path B is now a
   working end-to-end route, not just a characterisation.

## Deferred / known limits (state these in the PR)

> **The first item below was closed after this section was written — see
> [Follow-up: the zero-copy Class C state-feedback load](#follow-up-the-zero-copy-class-c-state-feedback-load)
> at the end of this file, which also corrects the staged-IO claim in it.**

- **The 32 state bits ride the boomerang.** Each SRL whose `Q` is used costs 32
  extra global-read bits, and 32 staged-IO write-out slots whenever the read
  macro lands in a later stage than the shift macro. The zero-copy alternative
  — a Class C state-feedback load carrying the state's *global* word index in
  metadata — needs a two-pass `FlattenedScriptV1::from` and is the natural
  follow-up (planC decision 1).
- **One forced major-stage split per distinct address-ready level**, inherited
  from Part B's Class C model. The intra-boomerang (zero-grid-sync) evaluation
  slot remains unimplemented. A design that only uses `Q31` pays nothing.
- **3 lanes and 1 word wasted per `Srl32Shift`** (planC decision 3). Removing
  the waste means variable-width Class R lanes — an invasive change to the
  kernel's `macro_lane >> 2` indexing for no functional gain.
- **Path B needs `-noiopad -noclkbuf` and a `setparam -unset` pass** before GEM
  can read the netlist (see above) — GEM's `sverilogparse` has no parameter
  support. The 0.33-era "Path B drops a non-constant `CE`" defect is **fixed**
  in 0.68; explicit instantiation is still the simplest route, but no longer
  the only correct one.
- **`compute-sanitizer` still unavailable** in the partial CUDA install, so
  memcheck/racecheck over the Class C shared-memory gather stays open.
- **Pre-existing quirk left alone:** the SRAM arm of `make_inputs_outputs`
  still clobbers `output_map` for a `PORT_R_RD_DATA` pin that is also a primary
  output, the same way the macro arm did before the fix above. No test in the
  tree reaches it; fixing it would change upstream SRAM behaviour.
- **Gotcha for future `.gv` fixtures:** the sverilog parser rejects an *empty*
  `//` comment line (it fails the whole file with a `NL_SV_PARSE` error whose
  reported position is that line). Write `// ---` instead of a bare `//`. This
  cost a debugging cycle here.
- **Not modelled:** `A` is 5 bits so no address clamp is needed; SRLC32E has no
  reset port; variable-length topologies beyond simple `Q31 -> D` chaining
  (e.g. a cascade tapped through a LUT) are out of scope; `SRL16E` maps to
  `SRLC32E` with a zero-extended address (exact for stages 0..15).

---

# Part C verification checklist (independent review, 2026-08-30)

Cross-checked `planC.md` and `question.md` against the actual tree. Re-ran on
Windows/MSVC (no features) and WSL Linux (Yosys 0.68, CUDA 12.6 / RTX 2050):

```
cargo build --bins                 exit 0
cargo test                         18 lib + 14 macro_test = all pass  (Windows + Linux)
cargo build --features cuda        exit 0  (nvcc 12.6: 101 regs, 0 spill, 1 barrier, 3328 B smem)
cargo run --bin macro_test         12 differential + 2 structural, all PASS
  6 pinned pre-Part-C blocks_data hashes all unchanged  (ABI provably neutral)
cuda_test --check-with-cpu  on srl_dynamic_addr / srl_cascade_q31 /
                               srl_ce_gating / heterogeneous
  -> "sanity test passed!"  (GPU == CPU, script hashes == macro_test)
naive_sim vs cuda_test VCDs on the same 4 designs
  -> 1040 samples, 0 mismatches (constant 1-cycle convention offset applied)
```

## A. `question.md` — SRLC32E (Primitive C) semantics

- [x] 1-bit `D`, 1-bit `CE`, 5-bit `A[4:0]` inputs; `Q` and `Q31` outputs —
      `src/aigpdk.rs` (`direction_of` / `width_of`), the canonical layout in
      `src/hwmacro.rs`, unit test `srl32_packing_roundtrips_every_bit`.
- [x] "On the global rising edge, if `CE == 1`, the internal 32-bit state
      shifts left (LSB to MSB), and `D` is loaded into index 0" —
      `eval_srl32_shift` = `(st << 1) | (w0 & 1)` with `CE` folded into
      `en_iv = CLK & CE` on the write-out clock enable. Unit test
      `srl32_shift_64_cycles_matches_u32_reference` (random stream vs an
      independent `u32` shift, 64 cycles); design test `srl_ce_gating.gv`
      (random `CE`, all 32 state bits compared every cycle for 400 cycles).
- [x] "The read port natively outputs the bit at the dynamic address `A[4:0]`
      **combinationally**" — `eval_srl32_read` = `(w[0] >> (w[1] & 31)) & 1`,
      scheduled as a Class C macro so `Q` is visible in the same simulated
      cycle as the address. Unit test `srl32_read_exhaustive_addresses` (all
      32 addresses × 8 representative states); design test
      `srl_dynamic_addr.gv` (address two AIG levels deep, so the split lands
      at a real level).
- [x] "The cascade port `Q31` always outputs the bit at index 31
      combinationally" — `Q31` *is* `state[31]`, asserted structurally
      (`aig.pin2aigpin_iv[Q31] == shift.outputs[31] << 1`) and behaviourally
      by `srl32_q31_is_state_bit_31` and `srl_cascade_q31.gv`.
- [x] "Native Macro Evaluation … **without SIMT warp divergence**" — both
      datapaths are single straight-line expressions (`(st<<1)|D` and
      `(st>>a)&1`), `A` is 5 bits so the shift can never be out of range (no
      clamp, no branch), and the two Class R kinds are selected by a mask, not
      a branch, so every warp lane runs an identical instruction stream.
      A 31-deep AIG mux tree is replaced by one shift and one AND.
- [x] "Boomerang Scheduler Extension … mixed-width operations … without
      stalling CUDA warps" — the levelized-DAG equations were **not extended
      at all** for Part C: the 32-bit state is exposed as 32 ordinary 1-bit
      AIG source pins, so `macro_level[M] = max input level` already says the
      right thing. `staging.rs` is byte-identical to Part B.
- [x] "Heterogeneous Memory Allocator … 1-bit boolean states alongside 64-bit
      aligned contiguous blocks" — `Srl32Shift` reuses Part A's even-aligned
      2-word Class R slot, so the kernel's single `unsigned long long` feedback
      load delivers the whole state coalesced; the read port's 37 input bits
      pack into 2 lanes chosen so the datapath is `w[0]` / `w[1] & 0x1f` with
      no cross-word extraction.

## B. `planC.md` work-breakdown items

- [x] 1 — `src/hwmacro.rs`: `Srl32Shift` / `Srl32Read` kinds; `SRL_*`
      constants; `class` / `num_input_bits` / `num_output_bits` /
      `num_perm_words` / `num_state_words` / `input_bit_slot` arms;
      `eval_srl32_shift` / `eval_srl32_read`; `srl32_shift_pack` /
      `srl32_read_pack`; the kind-tag constants shared with the flattener and
      the kernel; 5 unit tests.
- [x] 2 — `src/aig.rs`: parse arm allocating all 32 state pins (and `Q`) at
      first visit to keep aigpins topological; `"SRLC32E"` added to the
      clock-trace filter; post-pass collecting `D` / `CE` / `A[4:0]` and
      building `en_iv = CLK & CE`; `SRL_CE_TIE0` warning; synthetic
      `Srl32Read` cell ids at `netlistdb.num_cells + j` with
      `fuse_carry4_chains(netlistdb.num_cells + srl_read_cid.len())` so the
      two synthetic-id spaces cannot collide; `macro_order` unchanged.
- [x] 3 — `src/aigpdk.rs`: `SRLC32E` in `direction_of` + `width_of`
      (`CLK`/`CE`/`D` scalar in, `A[4:0]` bus in, `Q`/`Q31` scalar out).
- [x] 4 — `src/staging.rs`: **no change**, verified by test rather than
      assumed.
- [x] 5 — `src/pe.rs`: **no change**, verified by test rather than assumed.
- [x] 6 — `src/flatten.rs`: `classc_kinds` / `classr_kinds` on
      `FlatteningPart`, filled in `init_afters_writeouts`; tagged `[16 + i]`
      and the Class R table at `[16 + num_classc_macros + j]`; extended bound
      assert; ABI doc-comment table extended. `make_inputs_outputs` needed no
      change beyond the bookkeeping, as planC predicted (all 32 state pins
      exist, so nothing is skipped). Plus the `output_map` fix above.
- [x] 7 — `csrc/macro_eval.cuh` (`gem_eval_srl32_shift` /
      `gem_eval_srl32_read`, the `GEM_MACRO_*` / `GEM_CLASSC_*` tags and the
      `gem_classc_{perm,state}_words` dispatchers);
      `csrc/kernel_v1_impl.cuh` (branchless Class R select, tagged Class C
      decode); `src/emulate.rs` mirrors both. **Compiles under nvcc 12.6 and
      the GPU differential passes.**
- [x] 8 — `aigpdk/gemsrl.v` (blackbox + behavioural body),
      `aigpdk/gemsrl_map.v` (`SRL16E` and `$__XILINX_SHREG_` -> `SRLC32E`),
      `aigpdk.lib` `SRLC32E` cell (new `gem_srl_bus_5` bus type,
      `pin(CLK) { clock : true }`, `rising_edge` arcs `CLK -> Q/Q31` and a
      combinational arc `A -> Q`, `dont_use`/`dont_touch`/`map_only`/
      `is_macro_cell`), `usage.md` Step 1.7. **Validated against Yosys 0.68.**
- [x] 9 — `src/bin/naive_sim.rs`: `"SRLC32E"` added to all three cell filters
      and `"CE"` to the sequential-input filter; `Q31` pre-marked and
      refreshed in the latch phase (a pure register read); `Q` left in `topo`
      and settled in the propagate loop from `A`; the gated shift applied in
      the latch section beside the DSP arm. **Runs, and its VCD matches the
      GPU's.**
- [x] V — `src/bin/macro_test.rs`: `Behavioural` gained `srl_state` with `Q`
      resolved inside the settle fixpoint and `Q31` as a state bit, and the
      gated shift in `latch`; `Harness` gained `srls` and compares the full
      32-bit state every cycle; `"SRLC32E"` added to clock-port discovery;
      `srl_structural_checks`; 6 new `.gv` fixtures; all six pre-existing
      hashes pinned.

## C. `planC.md` design decisions / data layout

- [x] 1 — two macro endpoints on one netlist cell, not a third `MacroClass`.
      `Srl32Shift` keys on the real cell id, `Srl32Read` on a synthetic one.
      State routed through the boomerang as ordinary macro inputs (zero new
      plumbing); the zero-copy feedback load stays deferred.
- [x] 2 — cycle semantics: `Q(N) = state(N-1)[A(N)]`,
      `Q31(N) = state(N-1)[31]`, `state(N) = CE ? (state(N-1)<<1)|D : state(N-1)`.
      Both reads see the same pre-edge snapshot; matches the DFF convention,
      not the SRAM's registered-read one. Cross-validated three ways
      (`Behavioural`, `emulate`, `naive_sim`) — and the `output_map` bug above
      was found precisely because it broke this for `Q31`.
- [x] 3 — uniform 4-lane / 2-word Class R footprint, asserted in
      `srl32_packing_roundtrips_every_bit`.
- [x] 4 — divergence-free Class R dispatch by mask select, in both the kernel
      and the emulator; 0 extra registers measured.
- [x] 5 — additive metadata, byte-identical for every existing design; all six
      hashes pinned.
- [x] 6 — every state bit committed: all 32 pins created eagerly at parse time
      and all 32 get a `place_clken_datainv` entry; asserted in `run()` (no
      `usize::MAX` in the state vector) and by the 400-cycle state comparison.
- [x] 7 — no Class C macro when `Q` is unconnected; `srl_cascade_q31.gv`
      asserts zero Class C macros and one major stage.
- [x] Data layout: `Srl32Shift` 1 input bit / 32 output bits / 4 perm words /
      2 state words, `en_iv = CLK & CE`; `Srl32Read` 37 input bits
      (`state[0..32] @ 0..32`, `A @ 32..37`) / 1 output bit / 2 perm words /
      1 state word, `input_bit_slot(i) == i`, `en_iv = 1`.

## D. `planC.md` verification section

- [x] 1 — unit tests (5/4).
- [x] 2 — `macro_test` differential, 6/6 fixtures.
- [x] 3 — structural assertions, including the zero-Class-C cascade case.
- [x] 4 — ABI regression: all six pinned hashes unchanged.
- [x] 5 — `naive_sim` <-> emulator/GPU: **done**, 0 mismatches over 1040
      samples on 4 designs (closing Parts A and B's open item).
- [x] 6 — GPU bring-up: build clean, 4/4 designs pass `--check-with-cpu`,
      hashes match, register pressure unchanged.
- [x] 7 — Yosys 0.68 frontend: Path A end-to-end and checked in as
      `tests/data/yosys_srl.gv`; Path B characterised **and run end-to-end**
      (the 0.33 `CE` defect is fixed; the remaining friction is
      `-noiopad -noclkbuf` plus `setparam -unset`, both scripted above).

## E. Out of scope for Part C / not implemented

- [ ] Zero-copy Class C state-feedback load (the 32 state bits still ride the
      boomerang).
- [ ] Intra-boomerang (zero-grid-sync) Class C evaluation slot.
- [ ] Variable-width Class R lanes (the 3 wasted lanes / 1 wasted word).
- [ ] `compute-sanitizer` over the Class C shared-memory gather.
- [ ] The upstream SRAM `output_map` clobber (same shape as the macro bug
      fixed here).
- [ ] Cascade topologies beyond simple `Q31 -> D` chaining.

*(Two of these boxes are now closed — see the follow-up section below: the
zero-copy Class C state-feedback load is implemented, and the SRAM
`output_map` clobber is fixed. The rest stand.)*

## Verdict

Part C as scoped in `planC.md` is **implemented and fully verified**, and the
plan's central bet paid off: modelling SRLC32E as a Class R shift half plus a
Class C read half joined by 32 ordinary AIG source pins required **zero**
changes to the boomerang scheduler (`staging.rs`) and **zero** changes to
partition accounting (`pe.rs`). `Q31` costs nothing, a pure cascade costs no
grid syncs, the datapaths match `question.md` bit-for-bit under exhaustive
unit test, the metadata additions are provably ABI-neutral (six pinned
hashes), the CUDA kernel compiles at unchanged register pressure and the
**GPU-vs-CPU numeric differential passes on real hardware**, the
`naive_sim` cross-check that Parts A and B left open is now closed for all
three primitives at once, and the Yosys 0.68 frontend is validated end-to-end
on both paths.
Two latent bugs (an emulator debug-build panic and a primary-output mapping
clobber) were found and fixed. The residual gaps are `compute-sanitizer` (tool
absent from the partial CUDA install) and the deferred perf work.

## Files changed (Part C)

New: `tests/data/{srl_static_addr,srl_dynamic_addr,srl_cascade_q31,srl_ce_gating,heterogeneous,yosys_srl}.gv`,
`aigpdk/gemsrl.v`, `aigpdk/gemsrl_map.v`. (`yosys_srl.gv` is Yosys **0.68**
output — `synth -flatten` + `dfflibmap`/`abc -liberty aigpdk_nomem.lib` on an
explicit-`SRLC32E` source — checked in as a frontend regression fixture. It was
regenerated when the pinned Yosys moved from 0.33 to 0.68; the netlist is
structurally identical and `ysrl` is not hash-pinned, so nothing in
`macro_test.rs` needed to change for it.)

Modified: `src/hwmacro.rs` (`Srl32Shift`/`Srl32Read` kinds, layout constants,
`eval_srl32_shift`/`eval_srl32_read`, pack helpers, kind tags, 5 tests),
`src/aig.rs` (SRLC32E parse arm + clock trace + post-pass + synthetic cell
ids, `fuse_carry4_chains` base bumped), `src/aigpdk.rs` (SRLC32E leaf pins),
`src/flatten.rs` (`classc_kinds`/`classr_kinds`, tagged `[16+i]` + the Class R
kind table, extended bound assert, ABI doc, the `output_map` fix),
`src/emulate.rs` (Class R mask select, tagged Class C decode, the
`wrapping_neg` fix), `csrc/macro_eval.cuh` (`gem_eval_srl32_*`, kind tags,
Class C shape dispatchers), `csrc/kernel_v1_impl.cuh` (branchless Class R
select, tagged Class C decode), `src/bin/macro_test.rs` (SRL `Behavioural` +
state comparison + `srl_structural_checks` + pinned hashes),
`src/bin/naive_sim.rs` (SRLC32E golden arm), `aigpdk/aigpdk.lib` (SRLC32E
cell), `usage.md` (Step 1.7).

**Unchanged, and that is the result:** `src/staging.rs`, `src/pe.rs`.

---

# Follow-up: the zero-copy Class C state-feedback load

*(Implemented after the review above. This closes the first "Deferred / known
limits" bullet and planC decision 1's named follow-up, plus the SRAM
`output_map` quirk from section E. Everything else in section E stands.)*

## Status — landed and validated on both platforms

`cargo test` = **18 lib + 14 `macro_test`** green on Windows/MSVC **and** Linux;
`cargo build --features cuda` clean; GPU `cuda_test --check-with-cpu` passes on
all four Part C GPU designs with script hashes identical to `macro_test`;
`naive_sim` and the GPU agree on **1107 samples with 0 mismatches**. All six
pinned pre-Part-C `blocks_data` hashes are still unchanged.

## What changed and why

Part C originally routed SRLC32E's 32 state bits into the read half as 32
ordinary macro inputs — the "zero new plumbing" option planC chose, with the
zero-copy alternative deferred because *"`input_map` is filled
partition-by-partition inside `FlattenedScriptV1::from`, and the reader may be
flattened before the writer. That forces a two-pass build."*

**That two-pass build already existed.** `build_flattened_script_v1`
([src/flatten.rs:1011-1080](../src/flatten.rs#L1011-L1080)) runs
`init_afters_writeouts` + state allocation + `make_inputs_outputs` over *every*
part of *every* major stage, and only then
([src/flatten.rs:1082-1116](../src/flatten.rs#L1082-L1116)) loops again to call
`build_script`. So `build_script` already sees a fully populated `input_map`,
and a read half may safely be flattened before its shift half. The blocking
prerequisite planC assumed was not there.

So `Srl32Read` now takes its state by a direct load instead:

| | before | after |
|---|---|---|
| `Srl32Read` inputs | 37 bits (`state[0..32]` + `A[4:0]`) | **5 bits** (`A[4:0]`) |
| `num_perm_words` | 2 | **1** |
| state delivery | 32 boomerang source pins | one `input_state[word]` load |
| Class C payload | unused | **global state word index** |

The 32 state bits of one `Srl32Shift` already land contiguously in a single
32-bit I/O word (`make_inputs_outputs` maps state bit `k` to
`macro_state_global_word * 32 + k`), so the whole state is one aligned `u32`
read. `MacroBlock::feedback_pin` records state bit 0; `build_script` resolves
`input_map[feedback_pin] / 32` and ORs it into the Class C metadata payload
(28 bits, so the reg/io state may hold up to 2^28 words — asserted).

### Why reading `input_state` is the correct semantics

`simulate_v1_noninteractive_simple_scan`
([csrc/kernel_v1_impl.cuh:476-488](../csrc/kernel_v1_impl.cuh#L476-L488)) gives
**every major stage of cycle `N`** the same `input_state = states + N*size` and
`output_state = states + (N+1)*size`. So `input_state[feedback_word]` is
`state(N-1)` — the pre-edge snapshot — no matter which major stage either half
lands in. That is exactly design decision 2's `Q(N) = state(N-1)[A(N)]`, and it
is *more* robust than the boomerang route: it cannot be perturbed by stage
ordering at all.

### The load is hoisted, not left in the Class C loop

The Class C epilogue walks its macros serially with one active head lane each.
A global load placed inside that walk would serialize one full memory round
trip per Class C macro. Instead the kernel does a separate uniform lane walk
next to the existing `macro_p_cur` load
([csrc/kernel_v1_impl.cuh:273-303](../csrc/kernel_v1_impl.cuh#L273-L303)), so
every macro's feedback load is in flight at once and their latency overlaps the
clock-enable permutation. A thread that is head lane of macro `ci` in the
hoisting walk is head lane of macro `ci` in the evaluation walk (identical
stride arithmetic), so it simply reuses the register.

Measured cost: **103 registers, 0 bytes spill, 1 barrier, 3328 B smem**
(nvcc 12.6, sm_86) versus 101/0/1/3328 before — **+2 registers, still no
spill**, and the extra walk is uniform across the block so it adds no
divergence.

## Correction to the original write-up

The deferred bullet claimed the state bits cost *"32 staged-IO write-out slots
whenever the read macro lands in a later stage than the shift macro"*, and the
"The 32 state bits do cross splits as staged IO" section said `heterogeneous`
exercised exactly that. **That was wrong.** A state pin is a
`DriverType::Macro` *source*, i.e. it is fetched from global state by the
ordinary global-read path, which is available in **every** major stage — so it
never needs a `StagedIOPin` copy, the way a pin *computed* in an earlier stage
does. This was checked directly: with the state routed back through
`for_each_input`, `heterogeneous` still produces **zero** staged-IO copies of
any state bit.

What the boomerang route actually cost, and what the zero-copy load removes:

- 32 gathered input bits per read macro, in the global read and the boomerang
  permutation network;
- a second packed lane per read macro (2 perm words -> 1);
- the corresponding `place_sram_duplicate` slots.

It does **not** shrink `reg_io_state_size` or the script size — those are
fixed-shape per major stage (unchanged at 260/394 words and 34304/50432 bytes
for `srl_ce_gating` / `heterogeneous`). The win is boomerang *occupancy*, not
footprint. Stating it as a footprint win would have been an overclaim.

## Also fixed: the SRAM `output_map` clobber (section E item 5)

`make_inputs_outputs`'s RAMBlock arm did
`output_map.insert(d << 1, sram_rd_data_global_start * 32 + k)` for each
`PORT_R_RD_DATA` bit — the same shape as the macro-arm bug fixed during Part C,
where a pin that is *also* a primary output gets its `PrimaryOutput` entry
clobbered and resolves to the registered read-data slot (next cycle's value)
instead of its own write-out slot (this cycle's). Now
`entry(...).or_insert(...)`, so the primary output wins in either endpoint
order, matching the macro arm
([src/flatten.rs:540-547](../src/flatten.rs#L540-L547)). No fixture in the tree
reaches it, so this is a latent fix, not a behaviour change any test observes.

## Files changed in the follow-up

- `src/hwmacro.rs` — `Srl32Read` layout (5 input bits, 1 perm word),
  `SRL_READ_ADDR_OFFSET = 0`, `SRL_READ_STATE_OFFSET` removed,
  `eval_srl32_read(st_cur, w)`, `srl32_read_pack(addr)`,
  `CLASSC_PAYLOAD_LIMIT`, module + layout docs; the 3 SRL unit tests updated to
  the new signature and `srl32_packing_roundtrips_every_bit` re-pinned to the
  new shape.
- `src/aig.rs` — `MacroBlock::feedback_pin`; the read half's `inputs_iv` is now
  `A[4:0]` only and `feedback_pin = state[0]`; `for_each_input` deliberately
  does **not** enumerate `feedback_pin` (the property the test asserts).
- `src/flatten.rs` — `FlatteningPart::classc_feedback_pins`; payload resolution
  + word-alignment and 28-bit-range asserts in `build_script`; ABI doc-comment
  table; the SRAM `output_map` fix.
- `csrc/macro_eval.cuh` — `gem_eval_srl32_read(st_cur, w, out)`,
  `gem_srl32_read_perm_words() == 1`.
- `csrc/kernel_v1_impl.cuh` — hoisted `classc_fb` feedback-load walk; Class C
  dispatch passes it.
- `src/emulate.rs` — mirrors both (`eval_srl32_read(input_state[payload], &w)`).
- `src/bin/macro_test.rs` — structural checks rewritten for the new invariant:
  the read half's inputs are `A[4:0]` only, no state pin appears in its
  `inputs_iv` or its `for_each_input`, `feedback_pin == state[0]`, and across
  all 3 stages of `heterogeneous` exactly **one** state bit is gathered into
  the boomerang (`state[31]`, legitimately, via `Q31 -> q31_out`).

**Still unchanged:** `src/staging.rs`, `src/pe.rs`, `src/bin/naive_sim.rs`,
`aigpdk/*`, `usage.md`. The scheduler and the partition accounting did not move
for this either — `macro_level[Srl32Read]` was already "the level at which `A`
is ready", and dropping the level-0 state inputs cannot change a `max`.

## Verification (this follow-up)

```
Windows/MSVC   cargo test                 18 lib + 14 macro_test   all pass
Linux (WSL)    cargo test                 18 lib + 14 macro_test   all pass
Linux (WSL)    cargo build --features cuda                         clean
               nvcc 12.6 sm_86: 103 regs, 0 spill, 1 barrier, 3328 B smem
                                (was 101 regs; +2 for the hoisted load)

cut_map_interactive -> num_parts == 1;  cuda_test ... 1 --check-with-cpu
  srl_dynamic_addr   "sanity test passed!"  hash 1922814486707700671
  srl_cascade_q31    "sanity test passed!"  hash 8673665729173642157
  srl_ce_gating      "sanity test passed!"  hash 10270892352907827083
  heterogeneous      "sanity test passed!"  hash 8654351287595417102
        ^ every hash identical to the standalone macro_test run

naive_sim vs cuda_test output VCDs, sampled on the shared rising-edge grid:
  srl_dynamic_addr   2 signals,  158 samples, 0 mismatches
  srl_cascade_q31    1 signal,   159 samples, 0 mismatches   (160 cyc, so the
                                                              64-deep cascade
                                                              actually reaches
                                                              its output)
  srl_ce_gating      2 signals,  158 samples, 0 mismatches
  heterogeneous      8 signals,  632 samples, 0 mismatches
                                ----
                                1107 samples, 0 mismatches

ABI regression: all six pinned pre-Part-C hashes UNCHANGED
  simple_dff 4367312310778971198   mac_accumulator 1388458641175822285
  ripple_adder 2711449478675204136 carry_then_logic_then_carry 17676642068319116734
  mixed_macros 9165969952107539029 yadder 299385802866267780
```

`srl_cascade_q31`'s hash is **also** unchanged (8673665729173642157) — a pure
`Q31` cascade has no Class C macro at all, so the zero-copy path cannot touch
it. The four hashes that did move are exactly the four designs that read `Q`.

### Note on the `naive_sim` comparison method

The original write-up applied a "constant one-clock-period shift" to line the
two VCDs up. With a stimulus whose rising edges are at t = 5, 15, 25 ... ns
(what the scratchpad `genstim.sh` produces), **no shift is needed**: both
simulators dump on those same edges and their bodies are line-for-line
identical apart from (a) `cuda_test` emitting its initial `$dumpvars` at `#0`
where `naive_sim` emits it at `#5`, and (b) `naive_sim` running one edge past
the GPU's cycle budget. Both are excluded from the comparison explicitly rather
than absorbed into an offset. The offset in the original run was presumably an
artifact of a differently-phased stimulus; either way, what is reported above
is what this stimulus measures.

## Still open after this follow-up

- Intra-boomerang (zero-grid-sync) Class C evaluation slot — a design that
  reads `Q` still costs one forced major-stage split per distinct
  address-ready level.
- Variable-width Class R lanes (3 lanes + 1 word still wasted per
  `Srl32Shift`).
- `compute-sanitizer` over the Class C shared-memory gather — the tool is still
  absent from this partial CUDA install (`~/cuda/.../bin` has only
  `nvcc`/`ptxas`/`nvlink`/`cudafe++`/`fatbinary`/`bin2c`). The new global load
  is a plain in-bounds `input_state[word]` read with a range assert at build
  time, but it is not memcheck-verified.
- Cascade topologies beyond simple `Q31 -> D` chaining.
- A `-r` (release) CUDA build could not be produced in this WSL instance — it
  is SIGTERM'd part way through, and the WSL service itself crashed twice
  during this session (`Wsl/Service/E_UNEXPECTED`, recovered with
  `wsl --shutdown`). All GPU results above are from the **debug** CUDA build,
  which is the same kernel at the same `-O3` / `-maxrregcount=128` nvcc flags
  (`build.rs` does not vary them by profile), so the register report and the
  numeric results carry over; only host-side speed differs.
