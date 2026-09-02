# Part B — Native CARRY4 carry-chain macro in GEM

## Context

Part A ([.claude/planA.md](.claude/planA.md)) landed the word-level macro substrate
and its first tenant, DSP48E2. That macro is **Class R** (`MacroClass::Registered`,
[src/hwmacro.rs:23-36](src/hwmacro.rs#L23-L36)): every output bit comes from a
clocked register, so the macro is a clean cut at the cycle boundary — its 124
input bits are boomerang endpoints, its 48 output bits are AIG sources *next*
cycle. Nothing in the scheduler had to change: [src/staging.rs](src/staging.rs)
levelization stayed acyclic, and the CUDA kernel evaluated it in the epilogue
after all boomerang stages ([csrc/kernel_v1_impl.cuh:246-352](csrc/kernel_v1_impl.cuh#L246-L352)),
in the same slot as the SRAM.

**Part B is the CARRY4 primitive, and it is a different animal.** CARRY4 is
purely combinational: `O[i] = S[i] ^ C[i]` and `CO[i] = C[i+1]` are functions of
the inputs *this* cycle, and CARRY4 blocks cascade — `CO[3]` of one drives `CIN`
of the next, so an N-bit adder is `⌈N/4⌉` CARRY4s in a carry-dependent chain.
The outputs must be available to downstream AIG logic *within the same simulated
cycle*. This is exactly the "Boomerang Scheduler Extension" the challenge asks
for: *"Modifying the Levelized DAG scheduling equations to group and schedule
mixed-width operations ... without stalling CUDA warps."*

**The key insights that make this tractable:**

1. **The evaluation machinery already exists.** Part A's epilogue — gather packed
   input lanes with `__shfl_sync`, run a branchless `int64` datapath, store the
   result — is reusable verbatim. Part B only changes *where the result goes* and
   *when it commits*.

2. **The "cycle-internal cut" already exists too.** GEM splits deep circuits into
   *major stages* at global level indices ([src/staging.rs:188-229](src/staging.rs#L188-L229));
   between them the kernel does `cooperative_groups::this_grid().sync()` and
   re-reads state, and live wires crossing the split are carried as
   `EndpointGroup::StagedIOPin` — stored to the I/O region by stage *k*, read
   back by stage *k+1* via the "current-iteration" type bit in the global read
   ([csrc/kernel_v1_impl.cuh:88-92](csrc/kernel_v1_impl.cuh#L88-L92)). A Class C
   macro is modelled as a **forced major-stage split at the macro's level**: its
   inputs are staged outputs of stage *k*, its outputs are staged inputs of
   stage *k+1*, and the macro itself is evaluated in stage *k*'s partition
   epilogue exactly like a Class R macro — only the destination (a staged-IO
   scratch slot) and the commit (unconditional, no clock gate) differ.

3. **Fuse the chain.** A naive mapping gives one CARRY4 = one level = one forced
   split; a 64-bit adder would cost 16 grid syncs. Instead the frontend fuses a
   maximal `CO[3]→CIN` cascade into **one** `MacroKind::Carry4 { chain_len: k }`
   endpoint group whose native datapath ripples all `4k` carry bits in a fixed
   6-step parallel-prefix (independent of `k`). A 64-bit adder is then one macro
   level and one split.

**Intended outcome:** `assign {co, sum} = a + b + ci;` (or an explicitly
instantiated `CARRY4` chain) maps to a single combinational macro group,
evaluated in a handful of branchless ALU ops with no warp divergence, adding at
most one major-stage split per distinct carry-chain depth in the design.

---

## No blocking prerequisite

Part A already did the `git submodule update --init --recursive` + toolchain
work ([.claude/implementA.md](.claude/implementA.md)). `cargo build` (no
features), `cargo test`, and `cargo run --bin macro_test` are green. Part B
builds on that tree.

---

## Design decisions

### Class C = combinational macro, scheduled as a cycle-internal cut

`MacroClass::Combinational` was reserved by Part A and is unused today
([src/hwmacro.rs:33-36](src/hwmacro.rs#L33-L36)). Part B implements it with the
following contract:

- A Class C macro is **both** an `EndpointGroup` (its inputs are boomerang
  endpoints) **and** a set of AIG sources (`DriverType::Macro`, its outputs) —
  structurally identical to the SRAM, which is `EndpointGroup::RAMBlock` +
  `DriverType::SRAM`.
- Unlike the SRAM/Class-R case, the outputs feed **this** cycle. They are routed
  through the existing staged-IO scratch path, not the persistent register I/O
  region.
- A forced major-stage split at `macro_level[M]` guarantees the macro's fanout
  is evaluated in a later major stage, after a grid sync has made the macro
  output visible. Two Class C macros at the same level never combinationally feed
  each other (see the levelization argument below), so no new DAG cycle is
  possible.

**Why the forced-split model and not an intra-boomerang evaluation slot.** The
alternative — evaluate the macro *between* two boomerang stages of the same
partition, with no grid sync — is strictly faster (no global re-sync per macro
level) but requires threading macro outputs into the boomerang's per-stage
`afters` / `last_pin2localpos` plumbing ([src/flatten.rs:660-731](src/flatten.rs#L660-L731))
and into `build_one_boomerang_stage`'s `realized_inputs` growth
([src/pe.rs:62-96](src/pe.rs#L62-L96)) — invasive changes to the hottest,
least-documented code in the repo. The forced-split model reuses the
`StagedIOPin` machinery that already exists and is exercised by every
`--level-split` run, and Part A's epilogue evaluator unchanged. It is the
lower-risk first cut; the intra-boomerang slot is listed as deferred perf work.

### Levelized-DAG scheduling equations — the actual extension

GEM's current levelization ([src/staging.rs:113-136](src/staging.rs#L113-L136)):

```
level_id[g]        = max over fanin f of ( level_id[f] + 1 )     # AndGate g
endpt_level_id[e]  = max over input bit b of level_id[b]          # DFF / PO / SRAM / Class-R macro
```

Part B adds, for a Class C macro `M` with input bits `in(M)` and output bits
`out(M)`:

```
macro_level[M]     = max over b in in(M) of level_id[b]           # level at which its last input is ready
level_id[o]        = macro_level[M] + 1     for every o in out(M) # the "+1" is the single native eval step
```

Downstream `AndGate` and `endpt_level_id` equations then consume `level_id[o]`
unchanged. Because `level_id[o] > macro_level[M] ≥ level_id[b]` for every input
`b`, there is no combinational path that re-enters `M` inside one major stage —
the per-stage sub-DAG stays acyclic and the boomerang's own levelization
([src/pe.rs:82-96](src/pe.rs#L82-L96)) needs no change.

- **Split-point set** = the sorted distinct `macro_level[M]` over all Class C
  macros, unioned with any user `--level-split`. `build_staged_aigs` splits
  there. `from_split` places `in(M)` before the split (as `primary_output_pins`
  → staged-IO) and seeds `out(M)` after it (as `primary_inputs`).
- **Fixpoint.** `macro_level[M]` depends on input levels, which may depend on
  *another* Class C macro's output level (unfused chain, or macro→logic→macro).
  Compute in a small fixpoint: (1) `level_id` with all macro outputs at 0,
  (2) `macro_level` / macro-output levels, (3) re-run `level_id`; repeat until
  stable. Bounded by the macro dependency depth — **one iteration** for a design
  whose carry chains are all fused (the common case).
- **The chain ripple is *not* levelized.** For a fused `Carry4 { chain_len: k }`
  the `4k` internal carry bits collapse into the single `+1` step. This is the
  point: ~`4k` AIG levels of ripple carry become one macro level.

### Divergence-free datapath

- **Single CARRY4** (`chain_len == 1`): the spec recurrence is a fixed
  4-iteration unroll of scalar bitwise ops — trivially uniform across a warp.
- **Fused chain** (`chain_len == k`): evaluate all `4k` carries with a
  **fixed 6-step** `S`-masked parallel-prefix OR (covers `k ≤ 16`, i.e. 64
  carry bits), independent of `k`. Every lane runs the identical instruction
  stream regardless of its macro's actual chain length — no `switch`, no
  data-dependent loop bound. See the datapath spec in work item 1.

### Lane / packing model — inherited from Part A

Same 4-divides-32 discipline as [.claude/planA.md](.claude/planA.md): a macro's
packed input lanes are a power-of-two count (`{1, 2, 4, 8}`) so they never
straddle a warp boundary and `__shfl_sync` full-mask gathers stay safe.
`num_perm_words(kind)` and `num_state_words(kind)` become functions of
`chain_len`. Fused segments are capped at **`chain_len ≤ 16`** (130 input bits →
8 lanes, 128 output bits → 4 state words); a longer cascade is split into
≤16-slice fused segments joined by one extra macro level.

---

## Data layout specification

### CARRY4 canonical input / output order

`MacroKind::Carry4 { chain_len: k }`, canonical input index order:

```
S[0..4k], DI[0..4k], CYINIT, CIN
```

`num_input_bits = 8k + 2`, `num_output_bits = 8k` (`O[0..4k]` then the exposed
`CO` bits — see fusion below), `num_perm_words = ((8k+2)/32).next_power_of_two()`
capped at 8, `num_state_words = ((8k)/32 + 1).next_power_of_two()` capped at 4.

`input_bit_slot` for the single-CARRY4 case (`k == 1`, one lane):

| slot | field      |
|------|------------|
| 0..4 | `S[3:0]`   |
| 4..8 | `DI[3:0]`  |
| 8    | `CYINIT`   |
| 9    | `CIN`      |

For `k > 1`, pack `S` for all slices contiguously, then `DI` for all slices,
then `CYINIT`, then `CIN` — a plain bit-append into the `num_perm_words × 32`
region. Output slot: `O[0..4k]` at slots `0..4k`, exposed `CO` bits packed
after. `input_bit_slots_are_a_permutation` / `packing_roundtrips_every_bit`
style unit tests as in Part A.

### Staged-IO scratch for Class C macro outputs

Class C outputs are **not** in the `[normal | dup | sram | macro]` I/O write-out
region (that region is persistent register state read next cycle). They are
`staged_io_map` entries — the same slot class as a `StagedIOPin`
([src/flatten.rs:517-525](src/flatten.rs#L517-L525),
[src/staging.rs:36-42](src/staging.rs#L36-L42)) — living in the I/O state for the
duration of one simulated cycle and read by the next major stage's global read
with the `idx >> 31` "current-iteration" type bit
([src/flatten.rs:606-612](src/flatten.rs#L606-L612),
[csrc/kernel_v1_impl.cuh:106-110](csrc/kernel_v1_impl.cuh#L106-L110)).

The partition epilogue writes them **unconditionally**: `en_iv` for a Class C
macro is set to aig pin `1` (tie-high), which `query_permute_with_pin_iv`
([src/flatten.rs:338-344](src/flatten.rs#L338-L344)) turns into
`(perm 0, inv 1, set0 1)` → `clken == 1` in the kernel commit
([csrc/kernel_v1_impl.cuh:357-364](csrc/kernel_v1_impl.cuh#L357-L364)), i.e.
`output = new` every cycle. No clock trace, no `CEP`.

### Metadata words

Part A used script metadata slots `[8..12]` for the macro section. Part B adds:

| slot | meaning                                                        |
|------|---------------------------------------------------------------|
| 12   | `num_classc_macros` (in this partition)                        |
| 13   | `classc_perm_base` — permute-lane base, after SRAM + Class-R   |
| 14   | `classc_scratch_base` — staged-IO scratch write-out base       |

All emitted as `0` when `num_classc_macros == 0`, so a Part-A-only or macro-free
partition's script bytes — and the pinned `blocks_data` hashes — are unchanged
([src/bin/macro_test.rs:447-450](src/bin/macro_test.rs#L447-L450)).

### Permute-slot region

`sram_duplicate_permute` becomes `[srams ×4][classR macros ×4][classC macros ×lanes][duplicates ×1]`.
`dup_perm_pos` ([src/flatten.rs:396](src/flatten.rs#L396)) and the kernel
duplicate-branch bound ([csrc/kernel_v1_impl.cuh:334-336](csrc/kernel_v1_impl.cuh#L334-L336))
shift by `Σ classC_lanes` in addition to the Part A `num_macros * 4`.

---

## Work breakdown

### 1. `src/hwmacro.rs` — `Carry4` kind + branchless datapath

```rust
pub enum MacroKind {
    Dsp48e2,
    Carry4 { chain_len: u16 },   // 1 = a single CARRY4; k = a fused CO->CIN cascade
}

impl MacroKind {
    fn class(self) -> MacroClass {
        match self {
            MacroKind::Dsp48e2 => MacroClass::Registered,
            MacroKind::Carry4 { .. } => MacroClass::Combinational,
        }
    }
    // num_input_bits / num_output_bits / num_perm_words / num_state_words /
    // input_bit_slot: match on Carry4 { chain_len } per the table above.
}

/// CPU twin of `gem_eval_carry4` in csrc/macro_eval.cuh — keep line-for-line in sync.
pub fn eval_carry4(w: &[u32], chain_len: usize) -> u64;   // returns {CO_packed, O_packed}
```

`eval_carry4` semantics (mirrors the spec exactly, [.claude/question.md:48-53](.claude/question.md#L48-L53)):

- unpack `S`, `DI` (`4k` bits each), `CYINIT`, `CIN` into `u64`s (`k ≤ 16`)
- `c_in_bit = CYINIT | CIN`
- **carry vector** `C[1..4k+1]`, native parallel-prefix:
  - `gen  = DI & ~S`                          (where `S=0`, the carry becomes `DI`)
  - `prop = S`
  - seed bit 0 with `c_in_bit`: `carry = gen | (prop & c_in_bit)` … then
  - `for shift in [1,2,4,8,16,32]: carry |= prop & (carry << shift) & prop_run_mask`
    (fixed 6 iterations; `prop_run_mask` keeps the fill inside runs of `S=1`)
  - the closed form: `C[i+1]` = the nearest `gen` bit at or below `i` across a
    run of `prop`, else `c_in_bit`
- `O   = S ^ C[0..4k]`   where `C[0] = c_in_bit`, `C[i] = carry_vector[i-1]`
- `CO  = C[1..4k+1]`     (i.e. `carry_vector`)
- return `(CO_masked_to_4k << num_output_O_bits) | O`

> The parallel-prefix form is the divergence-free core: 6 shifts + ORs whatever
> the chain length. A `chain_len == 1` fast path (the literal 4-step unroll) is
> optional and only a readability aid — the prefix form already handles it.

Unit tests (`#[cfg(test)]`): all 16 `S` patterns × representative `DI` ×
`{CYINIT, CIN}` for a single CARRY4 against a hand-written 4-step reference;
a `k = 16` ripple-carry adder (`S = a^b`, `DI = a`) vs `u64` addition; the
prefix form vs an explicit `4k`-iteration ripple for random `S`/`DI`; the
packing round-trip for every canonical bit.

### 2. `src/aig.rs` — Class C macros in the AIG

`MacroBlock` ([src/aig.rs:46-74](src/aig.rs#L46-L74)) already carries `kind` /
`inputs_iv` / `outputs` / `en_iv`; for `Carry4` set `en_iv = 1`.

- **Parse arm** in `dfs_netlistdb_build_aig` beside the `GEM_DSP48E2` arm
  ([src/aig.rs:424-430](src/aig.rs#L424-L430)): a `CARRY4` cell — each `O[k]` /
  `CO[k]` output pin gets `add_aigpin(DriverType::Macro(cellid))` recorded into
  `outputs[]`.
- **No clock-trace arm.** CARRY4 has no `CLK` pin, so it is *not* added to the
  clock-tracing cell filter at [src/aig.rs:505-509](src/aig.rs#L505-L509) or the
  `matches!` at [src/aig.rs:507](src/aig.rs#L507). It *is* added to the endpoint
  cell filter.
- **Post-pass** beside the DSP one ([src/aig.rs:624-662](src/aig.rs#L624-L662)):
  collect `S` / `DI` / `CYINIT` / `CI` into `inputs_iv` in canonical order; leave
  `en_iv = 1`.
- **Chain fusion pass** (new, runs right after the CARRY4 post-pass): build the
  `CO[3] → CIN` adjacency over all `CARRY4` `MacroBlock`s (compare the driving
  aigpin of slice *i*'s `CO[3]` output against slice *j*'s `CIN` input); union
  maximal chains; for each chain of length `k > 1`, replace the `k` blocks with
  one `MacroBlock { kind: Carry4 { chain_len: k }, .. }` whose `inputs_iv`
  concatenates every slice's `S`/`DI` (+ the head's `CYINIT`/`CIN`) and whose
  `outputs` are every slice's `O` plus any `CO[3]` bit that has fanout *outside*
  the chain. Only fuse when the internal `CO[3]→CIN` net has no other consumer;
  otherwise keep that `CO` as an exposed output and still fuse. Cap fused
  segments at `k = 16`.
- `num_endpoint_groups` / `get_endpoint_group` ([src/aig.rs:735-759](src/aig.rs#L735-L759)):
  Class C macros share the `macros: IndexMap` with Class R; ordering is
  PO → DFF → SRAM → **Class R macros → Class C macros** (partition by
  `kind.class()`), so an existing `.gemparts` for a design with no Class C macro
  keeps its endpoint indices.
- **`topo_traverse_generic`** ([src/aig.rs:698-733](src/aig.rs#L698-L733)):
  `DriverType::Macro` output pins are already treated as leaves (no `AndGate`
  fan-in recursion, line 710). That is still correct — the *levelization*, not
  the traversal, is where Class C output levels get set (work item 4).

### 3. `src/aigpdk.rs` — CARRY4 leaf pins

Add `CARRY4` arms to `direction_of` / `width_of`
([src/aigpdk.rs:63-105](src/aigpdk.rs#L63-L105)):
`S` `[3:0]`, `DI` `[3:0]` input buses; `CYINIT`, `CI` scalar inputs;
`O` `[3:0]`, `CO` `[3:0]` output buses. No `CLK`.

### 4. `src/staging.rs` — Class C levelization + auto-split (**the scheduler extension**)

This is the core of Part B. New/changed:

- **`macro_levels(aig) -> IndexMap<macro_endpt, usize>`**: the fixpoint from
  *Design decisions* — `level_id` over the full AIG with macro outputs seeded at
  0, then `macro_level[M] = max(level_id[b] for b in in(M))`, then macro output
  levels `= macro_level[M] + 1`, repeat until stable.
- **`auto_level_split(aig, user_splits) -> Vec<usize>`**: sorted distinct
  `macro_level[M]` for every Class C macro, merged with `user_splits`. Wired into
  `cut_map_interactive` and `cuda_test` so the split set is derived, not
  hand-passed (keep `--level-split` as an override that unions in).
- **`StagedAIG`** gains `classc_macros: Vec<usize>` — the Class C macro endpoint
  indices *evaluated in this stage* (inputs realized here, outputs seed the
  next).
- **`from_split`** ([src/staging.rs:96-178](src/staging.rs#L96-L178)):
  - `level_id` pre-seeded with Class C macro output levels (from `macro_levels`)
    so the `AndGate` recurrence at [src/staging.rs:119-128](src/staging.rs#L119-L128)
    propagates them.
  - a Class C macro `M` with `macro_level[M] ∈ (last_split, cur_split]` is added
    to `staged.classc_macros`; its input bits are appended to
    `endpoints_before_split` / its endpoint group is realized this stage (so its
    124-analog input bits are co-partitioned boomerang endpoints).
  - `M`'s output pins are added to the running `primary_inputs` for later stages
    (they are `DriverType::Macro`, already stop the traversal) — same mechanism
    as `primary_output_pins` at [src/staging.rs:208-211](src/staging.rs#L208-L211).
  - `M`'s input pins that are still live past the split are already covered by
    the existing `nodes_at_split` logic; no double count.
- **`build_staged_aigs`** ([src/staging.rs:188-229](src/staging.rs#L188-L229)):
  unchanged in shape — it already iterates a split list and threads
  `primary_inputs` forward. It only needs to also thread `classc_macros`
  through and to accept the auto-derived split list.

### 5. `src/pe.rs` — Class C endpoint accounting

`Partition::build_one` ([src/pe.rs:508-573](src/pe.rs#L508-L573)) already counts
`num_macros` for Class R. Add `num_classc_macros` and generalize the reservation
([src/pe.rs:550-557](src/pe.rs#L550-L557)):

- permute slots: `+ Σ kind.num_perm_words()` over Class C macros (alongside the
  Part A `num_macros * 4`)
- reserved write-outs: Class C outputs live in staged-IO scratch, so they count
  as `Σ kind.num_state_words()` extra write-out words, plus the `+1` alignment
  hole shared with Part A's macro region.
- `comb_outputs_activations` ([src/pe.rs:518-541](src/pe.rs#L518-L541)) gets a
  `EndpointGroup::Macro` arm for the Class C case only if we route outputs
  through `get_or_place_output_with_activation`; the plan routes them straight
  to a scratch slot (work item 6), so no activation entry is needed — mirror the
  `RAMBlock` arm (`num_srams += 1`) with `num_classc_macros += 1`.

`build_one_boomerang_stage` needs **no change**: Class C macro output pins are
non-`AndGate` drivers, so `topo_traverse_generic` stops at them and they are
never scheduled into a fold; they enter the next stage as realized inputs via
the major-stage split, not via the boomerang.

### 6. `src/flatten.rs` — staged-IO routing + unconditional commit

- **`FlatteningPart`** ([src/flatten.rs:177-244](src/flatten.rs#L177-L244)): add
  `num_classc_macros`, `classc_perm_base`, `classc_scratch_base`. Compute in
  `init_afters_writeouts` ([src/flatten.rs:255-335](src/flatten.rs#L255-L335)):
  `classc_scratch_base` sits after `wo_base_macro + 2*num_macros`, its size is
  `Σ kind.num_state_words()`.
- **`make_inputs_outputs`** ([src/flatten.rs:414-545](src/flatten.rs#L414-L545)):
  extend the `EndpointGroup::Macro` arm ([src/flatten.rs:473-507](src/flatten.rs#L473-L507))
  to branch on `m.kind.class()`:
  - Class R: exactly as today (P-register slot, `en_iv`-gated).
  - Class C: place `kind.num_perm_words()` packed input lanes at
    `classc_perm_base + cur_classc_id * lanes`; for each output bit `k`,
    `staged_io_map.insert(outputs[k], (state_start + classc_scratch_base + …)*32 + …)`
    and `input_map.insert(outputs[k], same)` (so both the next stage's global
    read and any same-stage consumer resolve it), and
    `place_clken_datainv(scratch_local*32 + k, <perm of en_iv=1>, 1, 1, 0)` for
    the unconditional commit.
- **`build_script`** ([src/flatten.rs:547-597](src/flatten.rs#L547-L597)): emit
  metadata slots `[12..15]` (zero when `num_classc_macros == 0`).
- **State allocation** ([src/flatten.rs:862-879](src/flatten.rs#L862-L879),
  [src/flatten.rs:936-940](src/flatten.rs#L936-L940)): the even-alignment
  rounding already triggered by `num_macros > 0` should also trigger on
  `num_classc_macros > 0` (the packed-lane / state-word alignment argument is
  identical). Keep it gated so nothing moves for designs without Class C macros.
- Extend the module doc-comment ABI table
  ([src/flatten.rs:21-124](src/flatten.rs#L21-L124)) with the scratch region and
  the `[12..15]` slots.

### 7. `src/emulate.rs` + `csrc/macro_eval.cuh` + `csrc/kernel_v1_impl.cuh`

- **`csrc/macro_eval.cuh`**: add `gem_eval_carry4(const u32 *w, int chain_len)`
  `__host__ __device__`, the branchless prefix datapath, cross-referenced
  line-for-line with `eval_carry4` in `src/hwmacro.rs` (same discipline as
  `gem_eval_dsp48e2` ↔ `eval_dsp48e2`,
  [csrc/macro_eval.cuh:7-9](csrc/macro_eval.cuh#L7-L9)).
- **`csrc/kernel_v1_impl.cuh`**: in the macro epilogue
  ([csrc/kernel_v1_impl.cuh:246-352](csrc/kernel_v1_impl.cuh#L246-L352)), after
  the Class R block, add a Class C block:
  - read metadata `[12..15]`
  - the lane gather is the same `__shfl_sync(0xffffffff, sram_duplicate_t, …)`
    idiom, looped over `kind.num_perm_words()` lanes
  - **no** early 64-bit feedback load (combinational — there is no `P_current`)
  - evaluate `gem_eval_carry4`, predicate-store the `num_state_words` result
    words into `shared_writeouts[classc_scratch_base + …]` from the low lanes
  - this lands *before* the final `__syncthreads()` + commit
    ([csrc/kernel_v1_impl.cuh:354-364](csrc/kernel_v1_impl.cuh#L354-L364)); the
    unconditional `clken` from work item 6 makes the commit write the scratch
    slot every cycle
  - shift the duplicate-lane range bound by `Σ classc_lanes`
- **`src/emulate.rs`**: mirror it after the Class R macro loop
  ([src/emulate.rs:254-273](src/emulate.rs#L254-L273)) — read the packed lanes
  out of `sram_duplicate_perm`, call `eval_carry4`, write the result words into
  `writeouts[classc_scratch_base + …]` before the final gated commit
  ([src/emulate.rs:275-280](src/emulate.rs#L275-L280)). The next major-stage
  invocation reads them back via the staged-IO global-read path
  ([src/emulate.rs:66-69](src/emulate.rs#L66-L69)).

### 8. Frontend — CARRY4 recognition and chain fusion

New `aigpdk/` files, mirroring `gemmacro.v` / `gemmacro_map.v`:

- **`aigpdk/gemcarry.v`** — a `(* blackbox *)` `CARRY4` model with a behavioural
  body (the exact spec recurrence) for reference simulation, and a
  `_TECHMAP_REPLACE_`-friendly form.
- **`aigpdk/gemcarry_map.v`** — techmap `Xilinx CARRY4 → CARRY4` (identity port
  rename / `CI`↔`CIN`, `CYINIT` passthrough). Xilinx 7-series `CARRY4` already
  has exactly `S[3:0] DI[3:0] CI CYINIT → O[3:0] CO[3:0]`, so this is close to a
  rename; also accept `CARRY8` by splitting into two `CARRY4` (deferred).
- **liberty**: a `CARRY4` cell appended to `aigpdk.lib` with
  `dont_use / dont_touch / map_only / is_macro_cell : TRUE` and new
  `gem_carry_bus_4` bus types, copying the `GEM_DSP48E2` entry
  ([aigpdk/aigpdk.lib:915-956](aigpdk/aigpdk.lib#L915-L956)). Combinational
  timing arcs `S/DI/CI/CYINIT → O/CO` (no `related_pin : CLK`).

**Chain fusion** is done in Rust (work item 2), not Yosys — Yosys emits the
individual `CARRY4` cells with explicit `CO→CI` nets, and the AIG post-pass sees
the whole netlist at once, so `CO[3]→CIN` union-find there is simpler and more
robust than a Yosys pattern match. (An `opt`/`wreduce` pass in Yosys may still
help expose `$alu`/`$add` as CARRY4 in the first place.)

New synthesis step for [usage.md](usage.md), a **Step 1.6** after the DSP step:

```tcl
# Xilinx carry-chain inference, then rewrite to GEM's CARRY4
read_verilog path/to/aigpdk/gemcarry.v
techmap -map +/techmap.v              # $alu -> gate-level is the WRONG way; instead:
# synth_xilinx path up to 'alumacc; ... ; techmap -map +/xilinx/arith_map.v'
techmap -map +/xilinx/arith_map.v     # $alu/$add -> CARRY4 chains
techmap -map path/to/aigpdk/gemcarry_map.v   # Xilinx CARRY4 -> CARRY4
```

> As with the DSP flow, pass and map-file names drift between Yosys releases.
> Pin the version and validate `arith_map.v` produces `CARRY4` (not `$lut`
> carry) against the installed Yosys as the first frontend task. Explicit
> `CARRY4` instantiation is the dependable path; RTL `+` inference is
> version-dependent (Part A hit the same wall with `xilinx_dsp`,
> [.claude/implementA.md:175-189](.claude/implementA.md#L175-L189)).

### 9. `src/bin/naive_sim.rs` — CARRY4 golden arm

Add `CARRY4` to the combinational-cell dispatch in the topo propagate loop
([src/bin/naive_sim.rs:528-553](src/bin/naive_sim.rs#L528-L553)) — it is *not* a
clocked endpoint (do not add it to the `posedge_monitor` / latch-section filters
at [src/bin/naive_sim.rs:165-167](src/bin/naive_sim.rs#L165-L167),
[src/bin/naive_sim.rs:463](src/bin/naive_sim.rs#L463)). Evaluate the 4-bit slice
from its pins with a shared helper; for a chain, `naive_sim` sees the individual
slices and the `CO→CI` nets, so it needs no fusion — it just settles them in
topo order, which is a genuine independent cross-check against the fused native
evaluator.

### V. `src/bin/macro_test.rs` + `tests/data/*.gv` — differential harness

Extend the Part A harness ([src/bin/macro_test.rs](src/bin/macro_test.rs)). New
hand-written `.gv` cases:

- **`ripple_adder.gv`** — an 8- or 16-bit adder built from `CARRY4` slices
  (`S = a^b` from `AND2`/`INV` glue, `DI = a`), output = sum bits. One fused
  chain, one macro level → the pipeline must produce **one** major stage. Assert
  bit-exact vs the `Behavioural` evaluator over N random vectors.
- **`carry_select.gv`** — `DI` driven by something other than `a` (a mux select),
  to exercise the general `C[i+1] = S ? C[i] : DI[i]` recurrence, not just
  addition.
- **`carry_then_logic_then_carry.gv`** — a `CARRY4` whose `O`/`CO` feed AIG
  logic that feeds a *second, unrelated* `CARRY4`. This must produce **two**
  major stages with the first macro's outputs routed as staged IO into the
  second. Assert the stage count and the differential.
- **`mixed_macros.gv`** — `CARRY4` + `GEM_DSP48E2` + `DFF` + AIG glue in one
  design, to prove Class C and Class R coexist.

The harness ([src/bin/macro_test.rs:218-247](src/bin/macro_test.rs#L218-L247))
currently asserts `stageds.len() == 1`; generalize it to drive the real
multi-major-stage `simulate_v1_noninteractive` scan order
([src/bin/cuda_test.rs:436-445](src/bin/cuda_test.rs#L436-L445)) through
`emulate::simulate_block_v1` stage by stage.

Add `Carry4` handling to the harness's `Behavioural` model — settle the CARRY4
slices in topo order alongside the AIG gates (like `naive_sim`), so the
reference is fusion-independent.

---

## Verification (no GPU required)

1. **`eval_carry4` unit tests** — spec recurrence for a single slice (all `S`,
   representative `DI`, `CYINIT`/`CIN`); `k = 16` vs `u64` add; prefix form vs
   explicit ripple; packing round-trip. (`src/hwmacro.rs` `#[cfg(test)]`.)
2. **`macro_test` differential** — the four `.gv` cases above, bit-exact on every
   primary output every cycle, through `emulate::simulate_block_v1` **and** the
   independent `Behavioural` gate-level evaluator.
3. **Staging assertions** — `ripple_adder.gv` → exactly 1 major stage;
   `carry_then_logic_then_carry.gv` → exactly 2, with the first CARRY4's outputs
   present in stage 2's `primary_inputs` and in `staged_io_map`.
4. **ABI regression** — `simple_dff.gv` (`SIMPLE_DFF_HASH`) and
   `mac_accumulator.gv` (Part A) `blocks_data` hashes unchanged: every Part B
   code path is gated on `num_classc_macros > 0` / `MacroClass::Combinational`.
5. **`naive_sim` ↔ emulator** — run both on `mixed_macros.gv` and diff, closing
   the cross-check the Part A review flagged as not wired up
   ([.claude/implementA.md:431-436](.claude/implementA.md#L431-L436)).
6. **Deferred to GPU bring-up** — `csrc/` cannot be built here. Mitigate by
   keeping `gem_eval_carry4` ↔ `eval_carry4` line-for-line parallel. On first
   GPU access: `cargo build --features cuda`; `cuda_test --check-with-cpu` on
   `mixed_macros.gv`; `compute-sanitizer` over the new Class C lane shuffles and
   the extra grid syncs; confirm the staged-IO scratch slot is not perturbed by
   `writeout_inv ^= c3` (needs `c3 == 0` for scratch bit positions, structurally
   true via `place_clken_datainv(…, data_inv = 0)`).

---

## Deferred / known limits (state these in the PR)

- **Intra-boomerang evaluation slot is not implemented.** Every distinct carry
  chain *depth* in the design costs one forced major-stage split (one extra grid
  sync per simulated cycle). For datapath-heavy designs with adders at many
  depths this is real overhead. The zero-grid-sync alternative — evaluating the
  macro between boomerang stages of one partition — is the natural follow-up and
  is where the `afters` / `last_pin2localpos` threading work lives.
- **Fused segments capped at `chain_len ≤ 16`.** Longer cascades split into
  ≤16-slice segments joined by one extra macro level. 128-bit adders pay two
  levels.
- **Un-fused `CARRY4` (fusion pattern miss) is a correctness-safe fallback but
  slow** — each slice becomes its own macro level / split. The fusion pass
  should log every slice it could not fold.
- **`CARRY8` is split to two `CARRY4` in the frontend; not yet done.**
- **RTL `+` inference depends on the Yosys version's `arith_map.v`.** Explicit
  `CARRY4` instantiation is the validated path (same situation as Part A's DSP
  inference).
- **Part C (SRLC32E)** slots into this substrate: its addressed-read port is
  Class C (reuses everything here), its `Q31` cascade port is Class R (reuses
  Part A). The 32-bit shift state is a new `MacroKind` with both an
  `en_iv`-gated Class R state word *and* a Class C combinational read output —
  the first macro that is both classes at once.
