# Part C — Native SRLC32E shift-register-LUT macro in GEM

## Context

[.claude/question.md](question.md) requires native GPU evaluation models for three
Xilinx primitives. Two are landed:

- **Part A — DSP48E2** ([.claude/planA.md](planA.md)): `MacroClass::Registered`
  ("Class R"). Every output bit comes from a clocked register, so the macro is a
  clean cut at the cycle boundary. Inputs are boomerang endpoints; outputs are AIG
  sources *next* cycle. The scheduler needed no change; the kernel evaluates it in
  the partition epilogue ([csrc/kernel_v1_impl.cuh:254-360](../csrc/kernel_v1_impl.cuh#L254-L360)).
- **Part B — CARRY4** ([.claude/planB.md](planB.md)): `MacroClass::Combinational`
  ("Class C"). Outputs must reach downstream AIG logic *within* the same simulated
  cycle, so the macro is modelled as a **forced major-stage split at its level**;
  results land in the staged-IO scratch region and the next major stage reads them
  back ([src/staging.rs:8-27](../src/staging.rs#L8-L27),
  [csrc/kernel_v1_impl.cuh:362-391](../csrc/kernel_v1_impl.cuh#L362-L391)).

**Part C is SRLC32E**, and both prior plans flagged it as the interesting one:
*"the first macro that is Class C **and** Class R at once"*
([.claude/planB.md:566-570](planB.md#L566-L570),
[.claude/implementB.md:239-240](implementB.md#L239-L240)). The spec
([.claude/question.md:55-69](question.md#L55-L69)):

- 32-bit internal state; on the global rising edge, if `CE == 1`, the state shifts
  left (LSB→MSB) and `D` loads into index 0. → **clocked, Class R**.
- The read port outputs the bit at the dynamic address `A[4:0]` **combinationally**.
  `A` is a current-cycle combinational value, so `Q` must be available this cycle.
  → **Class C**.
- The cascade port `Q31` always outputs the bit at index 31 combinationally. `Q31`
  depends only on the *state*, not on any current-cycle combinational input.

**The key insight that makes Part C small.** Rather than inventing a third
scheduling class, decompose SRLC32E into the two classes that already exist,
joined by 32 ordinary AIG source pins:

```
        D ──────────────►┌─────────────────────┐
       CE ──(en_iv)─────►│  Srl32Shift  (R)    │── state[0..32] ──┬──► Q31 = state[31]
      CLK ──(en_iv)─────►│  st' = (st<<1)|D    │                  │
                         └─────────────────────┘                  │
                                                                  ▼
                             A[4:0] ──────────►┌─────────────────────┐
                                               │  Srl32Read   (C)    │──► Q
                                               │  q = (st >> a) & 1  │
                                               └─────────────────────┘
```

- `state[0..32]` are 32 `DriverType::Macro` AIG pins — sources, exactly like DFF `Q`
  or SRAM read data. They are read from `input_state` (the previous cycle's commit).
- `Q31` **is** `state[31]`. No macro, no split, no cost.
- `Srl32Read`'s inputs are those 32 source pins plus `A[4:0]` (37 bits, 2 packed
  lanes). Because the state pins are sources at level 0, the Part B equation
  `macro_level[M] = max_{b ∈ in(M)} level_id[b]` reduces to *"the level at which the
  address is ready"* — **the levelized-DAG equations need no new form at all.**

**Intended outcome.** `SRLC32E` survives the frontend as one cell, is evaluated as
two branchless native ALU ops (`(st<<1)|D` and `(st>>a)&1`) with no warp
divergence and no 31-deep mux tree of AIG gates, costs **zero** extra grid syncs
when only `Q31` is used (a pure cascade/delay line), and **one** forced split per
distinct address-ready level when `Q` is used.

---

## No blocking prerequisite

Parts A and B are landed and green in the working tree
([.claude/implementB.md](implementB.md)): `cargo test` = 13 lib + 7 `macro_test` on
Windows/MSVC and WSL Linux, `cargo build --features cuda` clean (nvcc 12.6),
GPU-vs-CPU differential passing on the RTX 2050. Part C builds on that tree and uses
the same two environments.

---

## Design decisions

### 1. Two macro endpoints on one netlist cell — not a third `MacroClass`

`SRLC32E` becomes **two** `MacroBlock`s in `aig.macros`:

| kind | class | inputs | outputs | state | eval |
|------|-------|--------|---------|-------|------|
| `Srl32Shift` | Registered | `D` (1 bit), `en_iv = CLK & CE` | `state[0..32]` | 2 words (Class R slot) | `(st_cur << 1) \| D` |
| `Srl32Read`  | Combinational | `state[0..32]`, `A[0..5]` (37 bits) | `Q` (1 bit) | 1 scratch word | `(st >> a) & 1` |

`Srl32Shift` keys on the real netlist cell id; `Srl32Read` keys on a **synthetic**
cell id above `netlistdb.num_cells`, the same device `fuse_carry4_chains` already
uses ([src/aig.rs:766-909](../src/aig.rs#L766-L909)).

**Why not a dual `MacroClass::RegisteredCombinational`?** One endpoint evaluated in
both epilogue slots would need a third class threaded through
`Partition::build_one` accounting ([src/pe.rs:539-545](../src/pe.rs#L539-L545)),
a dual slot allocation in `make_inputs_outputs`
([src/flatten.rs:524-597](../src/flatten.rs#L524-L597)), a new metadata region, and
a combined kernel block. The split reuses Part A's Class R path and Part B's Class C
path essentially verbatim; the only genuinely new code is two ~5-line datapaths and
a kind tag in each epilogue.

**Why route the 32 state bits through the boomerang** (rather than giving Class C a
state-feedback load like Class R's `macro_p_cur`,
[csrc/kernel_v1_impl.cuh:273-278](../csrc/kernel_v1_impl.cuh#L273-L278))? A feedback
load would need the *global* word index of the shift macro's I/O slot in the Class C
metadata — but `input_map` is filled partition-by-partition inside
`FlattenedScriptV1::from`, and the reader may be flattened before the writer. That
forces a two-pass build. Routing the state bits as ordinary macro inputs needs
**zero new plumbing**: it is exactly what a DSP whose `A`/`B`/`C` come from DFF `Q`
pins already does. Cost: 32 extra global-read bits + 32 boomerang write-out slots
per SRL whose `Q` is used. Listed as deferred perf work.

### 2. Cycle semantics — `Q` reads the *pre-edge* state

GEM's cycle *N* evaluates combinational logic from `input_state` (the value
committed at the end of cycle *N-1*) and commits endpoints into `output_state`;
i.e. a cycle is *the interval just before edge N*, and the commit is edge *N*. DFF
`Q` in cycle *N* is the value after edge *N-1*.

Therefore:

```
Q(N)   = state(N-1)[ A(N) ]        # A is this cycle's settled address
Q31(N) = state(N-1)[31]
state(N) = CE(N) ? ((state(N-1) << 1) | D(N)) : state(N-1)
```

Both reads see the same snapshot, and both match the DFF convention, so the
behavioural reference model (`macro_test`'s `Behavioural`, `naive_sim`) stays
consistent with the rest of GEM. This is *not* the SRAM's registered-read
convention ([src/emulate.rs:210-225](../src/emulate.rs#L210-L225), where read data
is visible next cycle) — SRLC32E's read port has no output register, which is
exactly why it must be Class C.

### 3. Class R keeps a uniform 4-lane / 2-word footprint

The kernel's Class R block indexes lanes as `macro_lane >> 2` with a full-mask
`__shfl_sync` over `lane0 + {0..3}`, and stores `wo_base_macro + m_i*2 + sub`
([csrc/kernel_v1_impl.cuh:259-278](../csrc/kernel_v1_impl.cuh#L259-L278),
[csrc/kernel_v1_impl.cuh:351-360](../csrc/kernel_v1_impl.cuh#L351-L360)); the
emulator does the same ([src/emulate.rs:272-285](../src/emulate.rs#L272-L285)).

`Srl32Shift` therefore keeps `num_perm_words() == 4` and `num_state_words() == 2`
even though it uses 1 input bit and 32 state bits. Wasting 3 lanes and 1 word per
SRL leaves every existing lane/word arithmetic in `flatten.rs`, `pe.rs`,
`emulate.rs` and the kernel **untouched**, and the 64-bit feedback load
(`macro_p_cur`) delivers `state_cur` for free.

### 4. Divergence-free Class R dispatch

Two Class R kinds now share one epilogue block. Rather than branch on the kind
(different macros in one warp could differ), compute both and select with a mask —
the same branchless idiom `gem_eval_dsp48e2` already uses for `OPMODE_S`
([csrc/macro_eval.cuh:54-58](../csrc/macro_eval.cuh#L54-L58)):

```cuda
unsigned long long pn_dsp = gem_eval_dsp48e2(w0, w1, w2, w3, p_cur);
unsigned long long pn_srl = gem_eval_srl32_shift(w0, p_cur);
unsigned long long sel    = 0ULL - (unsigned long long)(kind == GEM_MACRO_SRL32_SHIFT);
unsigned long long pn     = (pn_dsp & ~sel) | (pn_srl & sel);
```

Every lane runs an identical instruction stream regardless of which macro it serves
— directly answering *"Native Macro Evaluation … without SIMT warp divergence"*
([.claude/question.md:13](question.md#L13)).

The Class C dispatch sits inside the already-predicated head-lane block
([csrc/kernel_v1_impl.cuh:379-387](../csrc/kernel_v1_impl.cuh#L379-L387)); its
`perm_words`/`state_words` computation stays uniform across the block.

### 5. Metadata: additive, and byte-identical for every existing design

Script metadata today: `[8..12]` Class R section, `[12..16]` Class C section,
`[16 + i]` per-Class-C `chain_len`, then zero padding to 128
([src/flatten.rs:640-693](../src/flatten.rs#L640-L693)).

- **Class C kind tag.** Reinterpret `[16 + i]` as `kind << 28 | payload`:
  `kind 0` = `Carry4` with `payload = chain_len`; `kind 1` = `Srl32Read`
  (`payload` unused). For every Carry4 the word is numerically unchanged.
- **Class R kind table** at `[16 + num_classc_macros + j]`, `j ∈ [0, num_macros)`:
  `0` = `Dsp48e2`, `1` = `Srl32Shift`. For an all-DSP partition every word is `0` —
  *exactly the padding zeros already emitted there*, so the bytes and the pinned
  `blocks_data` hashes do not move.
- Assert `16 + num_classc_macros + num_macros <= 128`, extending the existing bound
  check at [src/flatten.rs:682-685](../src/flatten.rs#L682-L685).

### 6. Every state bit must be committed

For the DSP, an unconnected `P` bit is simply skipped
([src/flatten.rs:538-549](../src/flatten.rs#L538-L549)). For the SRL that would be a
**correctness bug**: an uncommitted state bit breaks the shift. So all 32 state
aigpins are created eagerly at parse time and all 32 get a `place_clken_datainv`
entry, whether or not anything reads them.

### 7. When `Q` is unconnected, create no Class C macro

A pure cascade/delay line (only `Q31` used, e.g. `Q31 → D` chains for a 64/96-deep
SRL) then costs **zero** forced splits and **zero** grid syncs — it is pure Class R.
This is the common FPGA usage and worth getting right.

---

## Data layout specification

Appended to the canonical layouts in
[src/hwmacro.rs:64-121](../src/hwmacro.rs#L64-L121).

### `MacroKind::Srl32Shift` (Class R)

```
inputs  (1 bit) : D                       -> input_bit_slot(0) = 0
outputs (32)    : state[0..32]
num_perm_words  = 4   (uniform Class R footprint; slots 1..127 unused)
num_state_words = 2   (state in the low 32 bits of the 64-bit slot; high word 0)
```

`en_iv = CLK_flag & CE`, built with `add_and_gate`, mirroring the DSP's
`en_iv = clk & (CEP | RSTP)` ([src/aig.rs:683-687](../src/aig.rs#L683-L687)) minus
the reset term (SRLC32E has none).

### `MacroKind::Srl32Read` (Class C)

```
inputs  (37 bits) : state[0..32] @ slots 0..32,  A[0..5] @ slots 32..37
outputs (1 bit)   : Q                            @ slot 0
input_bit_slot(i) = i                            (plain bit-append, as Carry4)
num_perm_words  = 2      (37 bits -> 2 words, power of two)
num_state_words = 1
```

The packing is chosen so the datapath is trivially `state = w[0]`,
`a = w[1] & 0x1f` — no cross-word extraction.

`en_iv = 1` (tie-high), which `query_permute_with_pin_iv` turns into
`(perm 0, inv 1, set0 1)` → `clken == 1` → the scratch slot is committed every cycle
([src/flatten.rs:384-390](../src/flatten.rs#L384-L390)).

---

## Work breakdown

### 1. `src/hwmacro.rs` — kinds, layouts, datapaths, unit tests

```rust
pub enum MacroKind {
    Dsp48e2,
    Carry4 { chain_len: u16 },
    Srl32Shift,   // Class R: the 32-bit clocked shift state
    Srl32Read,    // Class C: the dynamic-address read port
}

pub const SRL_STATE_WIDTH: usize = 32;
pub const SRL_ADDR_WIDTH:  usize = 5;
pub const SRL_READ_STATE_OFFSET: usize = 0;   // state[0..32] @ 0..32
pub const SRL_READ_ADDR_OFFSET:  usize = 32;  // A[0..5]      @ 32..37

/// CPU twin of `gem_eval_srl32_shift` in csrc/macro_eval.cuh.
/// `w[0] & 1` is D; `st_cur` is the current 32-bit state (low word of the
/// 64-bit Class R slot). Returns the next state in the low 32 bits.
pub fn eval_srl32_shift(w: [u32; 4], st_cur: u64) -> u64 {
    (((st_cur as u32) << 1) | (w[0] & 1)) as u64
}

/// CPU twin of `gem_eval_srl32_read`. `w[0]` is state[0..32], `w[1] & 0x1f`
/// is A[4:0]. Returns Q in bit 0. A is 5 bits so the shift can never be
/// out of range — no clamp, no branch.
pub fn eval_srl32_read(w: &[u32]) -> u32 { (w[0] >> (w[1] & 31)) & 1 }
```

Extend `class` / `num_input_bits` / `num_output_bits` / `num_perm_words` /
`num_state_words` / `input_bit_slot`
([src/hwmacro.rs:123-218](../src/hwmacro.rs#L123-L218)) with the two arms per the
layout table. Add `srl32_pack` helpers for the tests, mirroring `dsp48e2_pack` /
`carry4_pack`.

Unit tests (`#[cfg(test)]`, alongside the 5 CARRY4 tests):
- `srl32_read_exhaustive_addresses` — all 32 addresses × representative states.
- `srl32_shift_64_cycles_matches_u32_reference` — drive a random `D` stream through
  `eval_srl32_shift` and an independent `u32` shift; compare every cycle.
- `srl32_q31_is_state_bit_31` — after `k` shifts, bit 31 equals the bit shifted in
  32 cycles earlier.
- `srl32_packing_roundtrips_every_bit` — `input_bit_slot(i) == i` for the read
  macro; slot 0 for the shift macro's `D`.

### 2. `src/aig.rs` — parse, post-pass, endpoint order

- **Parse arm** beside the `CARRY4` arm
  ([src/aig.rs:443-457](../src/aig.rs#L443-L457)). On the first visit to an
  `SRLC32E` output pin, allocate all 32 state aigpins
  (`add_aigpin(DriverType::Macro(cellid))`) plus — only if `Q` is connected — the
  `Q` aigpin (`DriverType::Macro(synth_cellid)`), then:
  `pin2aigpin_iv[Q31] = state[31] << 1`, `pin2aigpin_iv[Q] = q << 1`.
  Allocating at first visit (not in the post-pass) keeps AIG pins in topological
  order — the invariant `macro_levels` and `from_split` rely on
  ([src/aig.rs:171-175](../src/aig.rs#L171-L175)).
- **Clock-trace filter**: add `"SRLC32E"` at
  [src/aig.rs:532-536](../src/aig.rs#L532-L536) so its `CLK` is traced.
- **Post-pass** beside the DSP/CARRY4 ones
  ([src/aig.rs:651-712](../src/aig.rs#L651-L712)): collect `D` into the shift
  macro's `inputs_iv[0]`; `en_iv = add_and_gate(clk_iv, ce_iv)`; collect `A[0..5]`
  and the 32 state pins into the read macro's `inputs_iv` in canonical order;
  `en_iv = 1`. Emit a `clilog::warn!` if `ce_iv` resolves to tie-0 (the SRL would
  never shift — the same trap the DSP's `CEP` has).
- **Synthetic cell ids**: allocate `Srl32Read` ids as `netlistdb.num_cells + j`, then
  call `fuse_carry4_chains(netlistdb.num_cells + num_srl_reads)`
  ([src/aig.rs:715](../src/aig.rs#L715)) so the two synthetic-id spaces cannot
  collide.
- **`macro_order`** ([src/aig.rs:747-756](../src/aig.rs#L747-L756)) already
  partitions Class R before Class C — no change; a Part-A/B design keeps its
  endpoint indices.

### 3. `src/aigpdk.rs` — SRLC32E leaf pins

Add arms to `direction_of` / `width_of`
([src/aigpdk.rs:63-76](../src/aigpdk.rs#L63-L76),
[src/aigpdk.rs:106-113](../src/aigpdk.rs#L106-L113)), matching the Xilinx unisim
port list exactly: `CLK`, `CE`, `D` scalar inputs; `A[4:0]` input bus; `Q`, `Q31`
scalar outputs.

### 4. `src/staging.rs` — **no change**

This is the headline result. `macro_levels`
([src/staging.rs:261-311](../src/staging.rs#L261-L311)) enumerates Class C macros
generically and computes `macro_level[M] = max input level`; the 32 state pins are
`DriverType::Macro` sources seeded at 0, so `macro_level[Srl32Read]` is exactly the
level at which `A` is ready. `from_split`'s seed / force-realize / filter logic
([src/staging.rs:124-252](../src/staging.rs#L124-L252)) and `build_staged_aigs`'s
auto-split union ([src/staging.rs:325-392](../src/staging.rs#L325-L392)) are
kind-agnostic. **Verify by test, change nothing.**

One consequence to document: a state pin live across a split becomes a
`StagedIOPin` copy in the earlier stage — existing behaviour for any live source pin
(DFF `Q` included), correct but adding write-out pressure.

### 5. `src/pe.rs` — accounting

[src/pe.rs:539-545](../src/pe.rs#L539-L545) already dispatches on `m.kind.class()`
and uses `m.kind.num_perm_words()` / `num_state_words()` for Class C. The Class R arm
hardcodes `num_macros += 1` and the reservation uses `2 * num_macros` /
`num_macros * 4` ([src/pe.rs:560-566](../src/pe.rs#L560-L566)) — correct as-is given
decision 3 (uniform Class R footprint). **Expected change: none.** Confirm by test;
if `Srl32Shift` ever gets a non-uniform footprint this is where it breaks.

### 6. `src/flatten.rs` — kind tables

- `FlatteningPart` ([src/flatten.rs:190-272](../src/flatten.rs#L190-L272)) gains
  `classr_kinds: Vec<u32>` and `classc_kinds: Vec<u32>`, filled in
  `init_afters_writeouts` ([src/flatten.rs:317-334](../src/flatten.rs#L317-L334))
  alongside `classc_chain_lens`.
- `make_inputs_outputs` ([src/flatten.rs:524-597](../src/flatten.rs#L524-L597)):
  the Class R arm is `kind`-generic already except that it skips
  `outputs[k] == usize::MAX`; per decision 6 the SRL creates all 32 pins so nothing
  is skipped. The Class C arm is already generic. **Expected change: none beyond the
  kind bookkeeping.**
- `build_script` ([src/flatten.rs:640-693](../src/flatten.rs#L640-L693)): emit
  `[16 + i] = classc_kind << 28 | payload` and the Class R kind table at
  `[16 + num_classc_macros + j]`; extend the `<= 128` assert.
- Extend the module doc-comment ABI table
  ([src/flatten.rs:34-118](../src/flatten.rs#L34-L118)) with both kind tables.

### 7. `csrc/macro_eval.cuh` + `csrc/kernel_v1_impl.cuh` + `src/emulate.rs`

- **`csrc/macro_eval.cuh`**: `gem_eval_srl32_shift(u32 w0, unsigned long long st_cur)`
  and `gem_eval_srl32_read(const u32 *w, u32 *out)`, `__host__ __device__`, kept
  line-for-line with the Rust twins per the file's standing discipline
  ([csrc/macro_eval.cuh:10](../csrc/macro_eval.cuh#L10)). Add
  `GEM_MACRO_DSP48E2 = 0` / `GEM_MACRO_SRL32_SHIFT = 1` and
  `GEM_CLASSC_CARRY4 = 0` / `GEM_CLASSC_SRL32_READ = 1` constants plus
  `gem_classc_{perm,state}_words(tag)` dispatchers.
- **`csrc/kernel_v1_impl.cuh`**:
  - Class R block
    ([csrc/kernel_v1_impl.cuh:351-360](../csrc/kernel_v1_impl.cuh#L351-L360)):
    read `kind = shared_metadata[16 + num_classc_macros + m_i]`, apply the
    branchless select of decision 4. The `macro_p_cur` load
    ([csrc/kernel_v1_impl.cuh:273-278](../csrc/kernel_v1_impl.cuh#L273-L278))
    already supplies `st_cur` in its low 32 bits — unchanged.
  - Class C block
    ([csrc/kernel_v1_impl.cuh:370-391](../csrc/kernel_v1_impl.cuh#L370-L391)):
    decode the tagged `shared_metadata[16 + ci]`, take `pw`/`sw` from the
    dispatcher, and call `gem_eval_carry4` or `gem_eval_srl32_read` in the head-lane
    branch. `wbuf[8]` is already large enough.
- **`src/emulate.rs`**: mirror both — the Class R loop
  ([src/emulate.rs:272-285](../src/emulate.rs#L272-L285)) gains the kind select, the
  Class C loop ([src/emulate.rs:295-315](../src/emulate.rs#L295-L315)) gains the tag
  decode.

### 8. Frontend — SRLC32E recognition

- **`aigpdk/gemsrl.v`** — a `(* blackbox *)` `SRLC32E` with the Xilinx unisim port
  list `(Q, Q31, A, CE, CLK, D)` and a behavioural body under a
  `` `ifndef GEM_SRLC32E_NO_BEHAVIOUR `` guard, mirroring
  [aigpdk/gemcarry.v](../aigpdk/gemcarry.v):
  ```verilog
  reg [31:0] sh;
  always @(posedge CLK) if (CE) sh <= {sh[30:0], D};
  assign Q   = sh[A];
  assign Q31 = sh[31];
  ```
- **`aigpdk/gemsrl_map.v`** — techmap `SRL16E → SRLC32E` (address zero-extended to 5
  bits; `Q31` unused) and Yosys's `$__XILINX_SHREG_` → `SRLC32E`, mirroring
  [aigpdk/gemcarry_map.v](../aigpdk/gemcarry_map.v)'s `CARRY8 → 2×CARRY4`.
- **`aigpdk/aigpdk.lib`** — an `SRLC32E` cell appended after the `CARRY4` entry
  (lines ~958-1005), with `dont_use / dont_touch / map_only / is_macro_cell : TRUE`,
  a new `gem_srl_bus_5` bus type for `A`, `pin(CLK) { clock : true }`, a
  `rising_edge` arc `CLK → Q/Q31` (the state dependency) and a combinational arc
  `A → Q` (the async read).
- **`usage.md`** — a **Step 1.7 (optional). Shift-register-LUT (SRLC32E) inference**
  after Step 1.6 ([usage.md:150](../usage.md#L150)):
  ```tcl
  read_verilog path/to/aigpdk/gemsrl.v
  shregmap -tech xilinx -minlen 3       # $dff chains -> SRL primitives
  techmap -map path/to/aigpdk/gemsrl_map.v
  ```
  With the same caveat Parts A and B both hit
  ([.claude/implementB.md:81-87](implementB.md#L81-L87)): pass names drift between
  Yosys releases, so **explicit `SRLC32E` instantiation is the validated path** and
  inference is best-effort. Pin the Yosys version and record what 0.33 actually
  emits.

### 9. `src/bin/naive_sim.rs` — SRLC32E golden arm

`SRLC32E` is clocked, so add it to all three cell filters
([src/bin/naive_sim.rs:167](../src/bin/naive_sim.rs#L167),
[src/bin/naive_sim.rs:278](../src/bin/naive_sim.rs#L278),
[src/bin/naive_sim.rs:329](../src/bin/naive_sim.rs#L329)) so its `CLK` joins
`posedge_monitor`. In the topo DFS, `Q` recurses on `A` only (`D`/`CE` are latch
inputs, not combinational fan-in of `Q`); `Q31` has no combinational fan-in. In the
propagate loop, beside the `CARRY4` arm
([src/bin/naive_sim.rs:530](../src/bin/naive_sim.rs#L530)), read `Q = state[A]` /
`Q31 = state[31]` from the pre-edge state; in the latch section, beside the DSP arm
([src/bin/naive_sim.rs:465](../src/bin/naive_sim.rs#L465)), apply
`if (CE) state = (state << 1) | D`.

### V. `src/bin/macro_test.rs` + `tests/data/*.gv` — differential harness

Extend `Behavioural`
([src/bin/macro_test.rs:89-245](../src/bin/macro_test.rs#L89-L245)) with
`srl_state: HashMap<usize, u32>`: `settle` resolves `Q`/`Q31` from the current state
(`Q` inside the fixpoint, since it depends on the combinational `A`); `latch`
applies the gated shift. Add `srls: Vec<(cellid, Vec<usize>)>` to `Harness` beside
`dsps` and compare the full 32-bit state each cycle through the shift macro's
`input_map` entries, exactly as the DSP `P` comparison does
([src/bin/macro_test.rs:446-465](../src/bin/macro_test.rs#L446-L465)). Add
`"SRLC32E"` to the clock-port discovery filter
([src/bin/macro_test.rs:306-310](../src/bin/macro_test.rs#L306-L310)).

New hand-written fixtures under [tests/data/](../tests/data/):

| fixture | what it pins down | expected stages |
|---|---|---|
| `srl_static_addr.gv` | `A` constant, `D` from a DFF, `Q` → PO. Basic shift + read. | 2 |
| `srl_dynamic_addr.gv` | `A` driven by AIG logic off a counter — a non-zero `macro_level`, exercising the forced split at a real level and the dynamic mux. | 2 |
| `srl_cascade_q31.gv` | two SRLs chained `Q31 → D` (64-deep), **`Q` unconnected on both**. Must create **zero** Class C macros and **1** major stage — the decision-7 fast path. | 1 |
| `srl_ce_gating.gv` | `CE` from a random primary input; the state must hold when `CE == 0`. | 2 |
| `heterogeneous.gv` | `SRLC32E` + `CARRY4` + `GEM_DSP48E2` + `DFF` + AIG glue in one design — the headline all-three-primitives test. | 2–3 (assert) |
| `yosys_srl.gv` | checked-in Yosys 0.33 output for an explicit `SRLC32E` design run through `synth` + `abc -liberty aigpdk_nomem.lib`, as [tests/data/yosys_carry4.gv](../tests/data/yosys_carry4.gv) does for CARRY4. | 2 |

Extend `structural_checks`
([src/bin/macro_test.rs:525-590](../src/bin/macro_test.rs#L525-L590)): one `SRLC32E`
yields exactly one `Srl32Shift` (Class R) and one `Srl32Read` (Class C); `Q31`'s
aigpin **is** `state[31]`; `srl_cascade_q31.gv` has zero Class C macros; the read
macro's `Q` appears in the next stage's `primary_inputs` and in `staged_io_map`.

**Also pin every existing hash.** `run_case` currently pins only `SIMPLE_DFF_HASH`
([src/bin/macro_test.rs:519](../src/bin/macro_test.rs#L519)); record and pin the
`mac_accumulator`, `ripple_adder`, `carry_then_logic_then_carry`, `mixed_macros`
and `yadder` hashes too, so Part C's ABI-neutrality is *asserted* rather than
claimed.

---

## Verification (no GPU required for 1-5)

1. **`hwmacro` unit tests** — the four SRL tests of work item 1, plus the existing 13
   must stay green: `cargo test`.
2. **`macro_test` differential** — all six new fixtures bit-exact on every primary
   output, every `P` bit and every SRL state bit, every cycle, through
   `emulate::simulate_block_v1` (major stage by major stage, the kernel's scan order)
   *and* the independent `Behavioural` gate-level evaluator:
   `cargo run --bin macro_test`.
3. **Structural assertions** — stage counts and the zero-Class-C cascade case above.
4. **ABI regression** — all six pinned `blocks_data` hashes unchanged. Every Part C
   path is gated on the new kinds, and the Class R kind table lands on existing
   padding zeros (decision 5).
5. **`naive_sim` ↔ emulator** — run both on `heterogeneous.gv` and diff, closing for
   SRLC32E the cross-check Parts A and B both left partially open
   ([.claude/implementB.md:374-378](implementB.md#L374-L378)).
6. **GPU bring-up (WSL2 + RTX 2050, per
   [.claude/implementB.md:6-13](implementB.md#L6-L13))** —
   `cargo build --features cuda`; `cut_map_interactive` then
   `cuda_test <design> <gemparts> <stim.vcd> 1 --check-with-cpu` on
   `srl_dynamic_addr`, `srl_cascade_q31` and `heterogeneous`; confirm each script
   hash matches the `macro_test` hash (proving the auto-derived split set is
   identical across entry points, as Part B did). Check the register/spill line from
   `nvcc` — the branchless dual-datapath select in the Class R block is the one
   change that could raise register pressure above the current 101 regs / 0 spill.
7. **Yosys 0.33 frontend** — `read_verilog aigpdk/gemsrl.v` elaborates; an explicit
   `SRLC32E` design survives `synth` + `dfflibmap`/`abc -liberty aigpdk_nomem.lib` +
   `techmap` with the instance intact, parses in GEM, and passes the 400-cycle
   differential (Path A). Record whether `shregmap -tech xilinx` infers `SRLC32E`
   from an RTL shift register (Path B) — informational, like Part B's Path B.

---

## Deferred / known limits (state these in the PR)

- **The 32 state bits ride the boomerang.** Each SRL whose `Q` is used costs 32 extra
  global-read bits and 32 write-out slots. The zero-copy alternative — a Class C
  state-feedback load carrying the state's *global* word index in metadata — needs a
  two-pass `FlattenedScriptV1::from` and is the natural follow-up (decision 1).
- **One forced major-stage split per distinct address-ready level**, inherited from
  Part B's Class C model. The intra-boomerang (zero-grid-sync) evaluation slot
  remains unimplemented
  ([.claude/implementB.md:222-223](implementB.md#L222-L223)).
- **3 lanes and 1 word wasted per `Srl32Shift`** (decision 3). Removing the waste
  means variable-width Class R lanes — an invasive change to the kernel's
  `macro_lane >> 2` indexing for no functional gain.
- **`SRL16E` maps to `SRLC32E` with a zero-extended address**; correct only when
  `Q31` is unused on that instance. `gemsrl_map.v` should reject the other case.
- **RTL inference (`shregmap`) is Yosys-version-dependent**; explicit instantiation
  is the validated path, as with Parts A and B.
- **`compute-sanitizer` still unavailable** in the partial CUDA install
  ([.claude/implementB.md:67-70](implementB.md#L67-L70)), so memcheck over the
  Class C shared-memory gather stays open.
- **Not modelled:** the `SRLC32E` `A` port is 5 bits so no address clamp is needed;
  no reset port exists on the primitive; variable-length (`SRLC16E` / cascade `Q31`
  through a LUT) topologies beyond simple `Q31 → D` chaining are out of scope.
