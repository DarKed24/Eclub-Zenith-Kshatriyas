// SPDX-FileCopyrightText: Copyright (c) 2024 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//! Word-level hardware macro substrate.
//!
//! GEM's boomerang scheduler natively evaluates only 1-bit AIG nodes. This
//! module is the single source of truth for *word-level* macros that survive
//! the frontend and are evaluated natively on the GPU ALU (Part A: the
//! DSP48E2 MAC).
//!
//! It generalises the one-off SRAM path (`$__RAMGEM_SYNC_`,
//! [`crate::aig::RAMBlock`]): a macro's inputs are gathered out of the boomerang
//! into a handful of lanes, evaluated with a branchless `int64` datapath, and
//! its outputs are re-injected as AIG sources — for a **Class R** macro on the
//! *next cycle* (like the SRAM), for a **Class C** macro in the *next major
//! stage of the same cycle* (Part B: the CARRY4 carry chain, routed through the
//! staged-IO scratch path).
//!
//! Part C (SRLC32E) is the first primitive that is *both* at once. Rather than
//! inventing a third scheduling class, it is decomposed into the two that
//! already exist, joined by a zero-copy state feedback:
//!
//! ```text
//!     D ───────────►┌──────────────────────┐
//!    CE ──(en_iv)──►│  Srl32Shift   (R)    │─ state[0..32] ──► Q31 = state[31]
//!   CLK ──(en_iv)──►│  st' = (st<<1) | D   │        │
//!                   └──────────────────────┘        │ one committed I/O word,
//!                                                   │ read back in place
//!                        A[4:0] ──────────►┌────────▼─────────────┐
//!                                          │  Srl32Read    (C)    │──► Q
//!                                          │  q = (st >> a) & 1   │
//!                                          └──────────────────────┘
//! ```
//!
//! `Q31` *is* `state[31]` — no macro, no split, no cost. `Srl32Read`'s only
//! boomerang input is `A[4:0]`, so the Part B equation
//! `macro_level[M] = max_{b in in(M)} level_id[b]` reduces to "the level at
//! which the address is ready" — the levelized-DAG equations need no new form.
//! The 32 state bits never enter the boomerang at all: they are loaded straight
//! out of the shift half's committed state word, whose global index the
//! flattener resolves into the Class C metadata payload.
//!
//! The CUDA twins of [`eval_dsp48e2`] / [`eval_carry4`] / [`eval_srl32_shift`] /
//! [`eval_srl32_read`] live in `csrc/macro_eval.cuh` (`gem_eval_dsp48e2` /
//! `gem_eval_carry4` / `gem_eval_srl32_shift` / `gem_eval_srl32_read`) and MUST
//! be kept line-for-line in sync.

/// Scheduling class of a macro.
///
/// Part A only built [`MacroClass::Registered`]; Part B added
/// [`MacroClass::Combinational`] (CARRY4). Part C's SRLC32E needs both at once
/// and gets them by being *two* macro blocks — one of each class — rather than
/// by a third variant here.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum MacroClass {
    /// Every output bit comes from clocked state, so the macro is a cut at the
    /// cycle boundary: its inputs are boomerang endpoints, its outputs are AIG
    /// sources next cycle. It can never combinationally feed another macro in
    /// the same cycle, so the levelization equations stay acyclic.
    ///
    /// DSP48E2 with `PREG=1` is Registered.
    Registered,
    /// Outputs depend combinationally on inputs, so the macro sits inside a
    /// combinational cone and needs an intra-cycle evaluation slot. Not
    /// implemented in Part A.
    Combinational,
}

/// The kind of a hardware macro.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum MacroKind {
    /// Xilinx DSP48E2 MAC, simplified subset: 27-bit pre-adder, 45-bit
    /// multiplier, 48-bit ALU writing a clocked `P` register. `OPMODE` is
    /// pre-decoded by the frontend into a 2-bit `OPMODE_S`. **Class R.**
    #[default]
    Dsp48e2,
    /// Xilinx CARRY4 carry chain (Part B). `chain_len == 1` is a single
    /// CARRY4 slice; `chain_len == k` is a maximal `CO[3] -> CIN` cascade of
    /// `k` slices fused by the frontend into one endpoint whose native
    /// datapath ripples all `4k` carry bits in a fixed parallel prefix.
    /// Purely combinational — **Class C**.
    Carry4 { chain_len: u16 },
    /// The clocked half of a Xilinx SRLC32E (Part C): the 32-bit shift state.
    /// One input bit (`D`), 32 output bits (`state[0..32]`, ordinary AIG
    /// source pins). `en_iv = CLK & CE` gates the commit, so `CE == 0` holds
    /// the state. **Class R.** `Q31` *is* `state[31]`, so the cascade port
    /// costs nothing at all.
    Srl32Shift,
    /// The combinational half of a Xilinx SRLC32E (Part C): the dynamically
    /// addressed read port. Inputs are `state[0..32]` (the shift half's source
    /// pins) followed by `A[0..5]`; the single output bit is `Q`.
    /// **Class C.** Created only when `Q` is actually connected, so a pure
    /// `Q31 -> D` cascade / delay line costs zero forced splits.
    Srl32Read,
}

/// Largest fused CARRY4 cascade (in slices). 16 slices = 64 carry bits =
/// 130 packed input bits (8 lanes) / 128 output bits (4 state words). A longer
/// cascade is split into `<= MAX_CARRY4_CHAIN`-slice segments joined by one
/// extra macro level.
pub const MAX_CARRY4_CHAIN: usize = 16;

// ---------------------------------------------------------------------------
// DSP48E2 canonical input layout
// ---------------------------------------------------------------------------

/// Canonical input bit order used by [`crate::aig::MacroBlock::inputs_iv`]:
/// `A[0..27], D[0..27], B[0..18], C[0..48], OPMODE_S[0..2], USE_D, RSTP`.
pub const DSP_A_OFFSET: usize = 0;
pub const DSP_A_WIDTH: usize = 27;
pub const DSP_D_OFFSET: usize = DSP_A_OFFSET + DSP_A_WIDTH; // 27
pub const DSP_D_WIDTH: usize = 27;
pub const DSP_B_OFFSET: usize = DSP_D_OFFSET + DSP_D_WIDTH; // 54
pub const DSP_B_WIDTH: usize = 18;
pub const DSP_C_OFFSET: usize = DSP_B_OFFSET + DSP_B_WIDTH; // 72
pub const DSP_C_WIDTH: usize = 48;
pub const DSP_OPMODE_S_OFFSET: usize = DSP_C_OFFSET + DSP_C_WIDTH; // 120
pub const DSP_OPMODE_S_WIDTH: usize = 2;
pub const DSP_USE_D_INDEX: usize = DSP_OPMODE_S_OFFSET + DSP_OPMODE_S_WIDTH; // 122
pub const DSP_RSTP_INDEX: usize = DSP_USE_D_INDEX + 1; // 123

/// Total number of canonical DSP48E2 input bits.
pub const DSP_NUM_INPUT_BITS: usize = DSP_RSTP_INDEX + 1; // 124
/// Number of DSP48E2 output bits (`P[47:0]`).
pub const DSP_NUM_OUTPUT_BITS: usize = 48;

// ---------------------------------------------------------------------------
// CARRY4 canonical input / output layout
// ---------------------------------------------------------------------------
//
// `MacroKind::Carry4 { chain_len: k }` canonical input index order is a plain
// bit-append (so `input_bit_slot(i) == i`):
//
//     S[0..4k], DI[0..4k], CYINIT, CIN        -> num_input_bits = 8k + 2
//
// canonical output index order:
//
//     O[0..4k], CO[0..4k]                     -> num_output_bits = 8k
//
// `MacroBlock::outputs` holds `usize::MAX` for any CO bit with no consumer
// outside the fused chain (the internal `CO[3] -> CIN` cascade nets).

/// `S[b]` of slice `b/4` -> canonical input index.
#[inline]
pub fn carry4_s_index(_chain_len: usize, b: usize) -> usize { b }
/// `DI[b]` of slice `b/4` -> canonical input index.
#[inline]
pub fn carry4_di_index(chain_len: usize, b: usize) -> usize { 4 * chain_len + b }
/// head-slice `CYINIT` -> canonical input index.
#[inline]
pub fn carry4_cyinit_index(chain_len: usize) -> usize { 8 * chain_len }
/// head-slice `CIN` -> canonical input index.
#[inline]
pub fn carry4_cin_index(chain_len: usize) -> usize { 8 * chain_len + 1 }
/// `O[b]` of slice `b/4` -> canonical output index.
#[inline]
pub fn carry4_o_index(_chain_len: usize, b: usize) -> usize { b }
/// `CO[b]` of slice `b/4` -> canonical output index.
#[inline]
pub fn carry4_co_index(chain_len: usize, b: usize) -> usize { 4 * chain_len + b }

// ---------------------------------------------------------------------------
// SRLC32E canonical input / output layout (Part C)
// ---------------------------------------------------------------------------
//
// `MacroKind::Srl32Shift` (Class R):
//     inputs  (1)  : D                 -> input_bit_slot(0) = 0
//     outputs (32) : state[0..32]
//   num_perm_words  = 4  (the uniform Class R footprint; slots 1..127 unused)
//   num_state_words = 2  (state in the low 32 bits of the 64-bit slot)
//
// `MacroKind::Srl32Read` (Class C):
//     inputs  (5)  : A[0..5] @ 0..5
//     outputs (1)  : Q       @ 0
//   input_bit_slot(i) = i   (plain bit-append, exactly like CARRY4)
//   num_perm_words  = 1  -> the datapath is `a = w[0] & 0x1f`
//   num_state_words = 1
//
// The 32 state bits are NOT macro inputs: the read half takes them by a
// zero-copy feedback load straight out of the shift half's committed I/O word,
// whose *global* word index the flattener resolves and parks in the Class C
// metadata payload (`MacroBlock::feedback_pin` -> `input_map` -> word index).
// The alternative — routing the state through the boomerang as 32 ordinary
// source pins — costs 32 global-read bits, 32 packed lane slots and 32
// staged-IO write-out copies per SRL whose `Q` is read, all to re-materialise a
// word that already sits contiguously in global state.
//
// `Srl32Shift` deliberately keeps the 4-lane / 2-word Class R footprint even
// though it uses 1 input bit and 32 state bits: wasting 3 lanes and 1 word per
// SRL leaves every lane/word arithmetic in `flatten.rs`, `pe.rs`, `emulate.rs`
// and the kernel untouched, and the kernel's 64-bit `macro_p_cur` feedback load
// then delivers `state_cur` for free.

/// Width of the SRLC32E shift state.
pub const SRL_STATE_WIDTH: usize = 32;
/// Width of the SRLC32E read address `A`.
pub const SRL_ADDR_WIDTH: usize = 5;
/// canonical input offset of `A[0..5]` in [`MacroKind::Srl32Read`]. The state
/// is not an input at all — see the zero-copy feedback note above.
pub const SRL_READ_ADDR_OFFSET: usize = 0;
/// Total number of canonical [`MacroKind::Srl32Read`] input bits.
pub const SRL_READ_NUM_INPUT_BITS: usize = SRL_READ_ADDR_OFFSET + SRL_ADDR_WIDTH; // 5

impl MacroKind {
    /// The scheduling class of this macro kind.
    pub fn class(self) -> MacroClass {
        match self {
            MacroKind::Dsp48e2 | MacroKind::Srl32Shift => MacroClass::Registered,
            MacroKind::Carry4 { .. } | MacroKind::Srl32Read => MacroClass::Combinational,
        }
    }

    /// Number of canonical input bits gathered out of the boomerang.
    pub fn num_input_bits(self) -> usize {
        match self {
            MacroKind::Dsp48e2 => DSP_NUM_INPUT_BITS, // 124
            MacroKind::Carry4 { chain_len } => 8 * chain_len as usize + 2,
            MacroKind::Srl32Shift => 1,                    // D
            MacroKind::Srl32Read => SRL_READ_NUM_INPUT_BITS, // A[4:0]
        }
    }

    /// Number of output bits re-injected as AIG sources.
    pub fn num_output_bits(self) -> usize {
        match self {
            MacroKind::Dsp48e2 => DSP_NUM_OUTPUT_BITS, // 48
            MacroKind::Carry4 { chain_len } => 8 * chain_len as usize,
            MacroKind::Srl32Shift => SRL_STATE_WIDTH, // 32
            MacroKind::Srl32Read => 1,                // Q
        }
    }

    /// Number of 32-bit permute words a macro occupies in the packed
    /// input region (one lane per word). Power of two so lanes never
    /// straddle a warp boundary.
    pub fn num_perm_words(self) -> usize {
        match self {
            // The uniform Class R footprint. `Srl32Shift` keeps it (using only
            // slot 0) so the kernel's `macro_lane >> 2` indexing and the
            // full-mask `__shfl_sync` over `lane0 + {0..3}` are untouched.
            MacroKind::Dsp48e2 | MacroKind::Srl32Shift => 4,
            MacroKind::Carry4 { chain_len } => {
                let bits = 8 * chain_len as usize + 2;
                ((bits + 31) / 32).next_power_of_two().min(8)
            }
            // 5 bits -> 1 word: `a = w[0] & 0x1f`. The state arrives by the
            // zero-copy feedback load, not through the packed lanes.
            MacroKind::Srl32Read => 1,
        }
    }

    /// Number of 32-bit state words a macro occupies in the per-partition
    /// write-out region. For Class R (DSP) this is the 64-bit-aligned `P`
    /// register; for Class C (CARRY4) this is the staged-IO scratch slot.
    pub fn num_state_words(self) -> usize {
        match self {
            // 2 for every Class R kind: the DSP's 48-bit `P` and the SRL's
            // 32-bit shift state both live in one 8-byte-aligned pair, which
            // the kernel loads back as a single `unsigned long long`.
            MacroKind::Dsp48e2 | MacroKind::Srl32Shift => 2,
            MacroKind::Carry4 { chain_len } => {
                let bits = 8 * chain_len as usize;
                ((bits + 31) / 32).next_power_of_two().min(4)
            }
            MacroKind::Srl32Read => 1,
        }
    }

    /// Map a canonical input index to its bit slot in the `num_perm_words * 32`
    /// packed input words.
    ///
    /// DSP48E2 packing (124 of 128 slots used):
    ///
    /// | lane | bits 0..              | remaining bits                                |
    /// |------|-----------------------|-----------------------------------------------|
    /// | 0    | `A[26:0]` @ 0..27     | `OPMODE_S[1:0]` @ 27..29, `USE_D` @ 29, `RSTP` @ 30 |
    /// | 1    | `D[26:0]` @ 0..27     | `C[36:32]` @ 27..32                            |
    /// | 2    | `B[17:0]` @ 0..18     | `C[47:37]` @ 18..29                            |
    /// | 3    | `C[31:0]` @ 0..32     | —                                             |
    ///
    /// CARRY4 and both SRLC32E halves use a plain bit-append: slot `i` ==
    /// canonical index `i`.
    pub fn input_bit_slot(self, i: usize) -> usize {
        match self {
            MacroKind::Carry4 { .. } | MacroKind::Srl32Shift | MacroKind::Srl32Read => {
                return i
            }
            MacroKind::Dsp48e2 => {}
        }
        match i {
            // A[0..27] -> lane 0, slots 0..27
            0..=26 => i,
            // D[0..27] -> lane 1, slots 32..59
            27..=53 => 32 + (i - DSP_D_OFFSET),
            // B[0..18] -> lane 2, slots 64..82
            54..=71 => 64 + (i - DSP_B_OFFSET),
            // C[0..48] -> lanes 3, 1, 2
            72..=119 => {
                let c = i - DSP_C_OFFSET;
                match c {
                    0..=31 => 96 + c,         // lane 3, slots 96..128
                    32..=36 => 59 + (c - 32), // lane 1, slots 59..64
                    37..=47 => 82 + (c - 37), // lane 2, slots 82..93
                    _ => unreachable!(),
                }
            }
            // OPMODE_S[0..2] -> lane 0, slots 27..29
            120 => 27,
            121 => 28,
            // USE_D -> lane 0, slot 29
            122 => 29,
            // RSTP -> lane 0, slot 30
            123 => 30,
            _ => panic!("dsp48e2 canonical input index {i} out of range"),
        }
    }
}

// ---------------------------------------------------------------------------
// Datapath
// ---------------------------------------------------------------------------

/// Sign-extend the low `w` bits of `v` to a full `i64`.
#[inline]
fn sext(v: u64, w: u32) -> i64 {
    ((v << (64 - w)) as i64) >> (64 - w)
}

/// CPU twin of `gem_eval_dsp48e2` in `csrc/macro_eval.cuh` — **keep
/// line-for-line in sync**.
///
/// `w` holds the four packed input lanes (see [`MacroKind::input_bit_slot`]);
/// `p_cur` is the current value of the `P` register (only the low 48 bits
/// matter). Returns `P_next` masked to 48 bits.
///
/// Semantics (PREG=1, all other registers combinational):
/// - sign-extend `A`/`D` from 27, `B` from 18, `C` from 48, `p_cur` from 48
/// - pre-adder `AD = sext27((A + (D & -USE_D)) & 0x7ffffff)` — wraps at 27 bits
///   exactly as the real DSP48E2 pre-adder, keeping `M = AD * B` exactly 45 bits
/// - `M = AD * B`
/// - branchless ALU select on the 2-bit state:
///   `s1 = -(state==1)`, `s2 = -(state==2)`, `s0 = ~(s1|s2)`;
///   `P_next = (s0 & C) | (s1 & M) | (s2 & (P + M))`
/// - synchronous reset: `P_next &= ~(-RSTP)`
/// - return `P_next & 0xffff_ffff_ffff` (OVERFLOW/UNDERFLOW ignored per spec)
pub fn eval_dsp48e2(w: [u32; 4], p_cur: u64) -> u64 {
    let a = sext((w[0] & 0x07ff_ffff) as u64, 27);
    let d = sext((w[1] & 0x07ff_ffff) as u64, 27);
    let b = sext((w[2] & 0x0003_ffff) as u64, 18);
    let c_raw = (w[3] as u64)
        | (((w[1] >> 27) as u64) << 32)
        | ((((w[2] >> 18) & 0x7ff) as u64) << 37);
    let c = sext(c_raw, 48);
    let p = sext(p_cur, 48);

    let state = (w[0] >> 27) & 3;
    let use_d = ((w[0] >> 29) & 1) as i64;
    let rstp = ((w[0] >> 30) & 1) as i64;

    let ad = sext((a.wrapping_add(d & -use_d) as u64) & 0x07ff_ffff, 27);
    let m = ad.wrapping_mul(b); // 45-bit product

    let s1 = -((state == 1) as i64);
    let s2 = -((state == 2) as i64);
    let s0 = !(s1 | s2);
    let mut pn = (s0 & c) | (s1 & m) | (s2 & p.wrapping_add(m));
    pn &= !(-rstp);
    (pn as u64) & 0x0000_ffff_ffff_ffff
}

/// Pack canonical DSP48E2 field values into the four permute lanes. Convenience
/// for the golden model and tests; the flattener packs bit-by-bit instead.
pub fn dsp48e2_pack(
    a: u32, d: u32, b: u32, c: u64, opmode_s: u32, use_d: bool, rstp: bool,
) -> [u32; 4] {
    let mut w = [0u32; 4];
    let k = MacroKind::Dsp48e2;
    let mut set = |canon: usize, bit: u32| {
        if bit & 1 != 0 {
            let slot = k.input_bit_slot(canon);
            w[slot / 32] |= 1 << (slot % 32);
        }
    };
    for i in 0..DSP_A_WIDTH {
        set(DSP_A_OFFSET + i, a >> i);
    }
    for i in 0..DSP_D_WIDTH {
        set(DSP_D_OFFSET + i, d >> i);
    }
    for i in 0..DSP_B_WIDTH {
        set(DSP_B_OFFSET + i, b >> i);
    }
    for i in 0..DSP_C_WIDTH {
        set(DSP_C_OFFSET + i, (c >> i) as u32);
    }
    for i in 0..DSP_OPMODE_S_WIDTH {
        set(DSP_OPMODE_S_OFFSET + i, opmode_s >> i);
    }
    set(DSP_USE_D_INDEX, use_d as u32);
    set(DSP_RSTP_INDEX, rstp as u32);
    w
}

// ---------------------------------------------------------------------------
// CARRY4 datapath
// ---------------------------------------------------------------------------

/// CPU twin of `gem_eval_carry4` in `csrc/macro_eval.cuh` — **keep the
/// algorithm line-for-line in sync**.
///
/// `w` holds the packed input lanes (see the CARRY4 canonical layout above:
/// `input_bit_slot(i) == i`, so `S[0..4k]` occupy bits `0..4k`, `DI[0..4k]`
/// bits `4k..8k`, `CYINIT` bit `8k`, `CIN` bit `8k+1`). Returns the packed
/// outputs: `O[0..4k]` in bits `0..4k`, `CO[0..4k]` in bits `4k..8k`.
///
/// Semantics (mirrors the spec, [`.claude/question.md`] primitive B):
/// - `C[0] = CYINIT | CIN`
/// - `C[i+1] = (S[i] & C[i]) | (~S[i] & DI[i])`   for `i in 0..4k`
/// - `O[i]  = S[i] ^ C[i]`
/// - `CO[i] = C[i+1]`
///
/// The `4k` carries are rippled with a fixed `⌈log2(4k)⌉`-step parallel-prefix
/// carry-lookahead on `(generate, propagate) = (DI & ~S, S)`, so every lane of
/// a warp runs an identical instruction stream regardless of `chain_len` — no
/// data-dependent loop bound, no divergence. A fused `CO[3]->CIN` cascade is
/// just a longer continuous ripple: the internal slice boundaries carry no
/// special meaning because `CO[3]` of slice `j` is exactly `C[4(j+1)]`, the
/// carry into slice `j+1`.
pub fn eval_carry4(w: &[u32], chain_len: usize) -> u128 {
    let n = 4 * chain_len; // number of carry bits, <= 64
    debug_assert!(n <= 64 && chain_len >= 1);
    let bit = |i: usize| -> u64 { ((w[i / 32] >> (i % 32)) & 1) as u64 };

    let mut s = 0u64;
    let mut di = 0u64;
    for i in 0..n {
        s |= bit(i) << i;
        di |= bit(n + i) << i;
    }
    let c_in_bit = bit(2 * n) | bit(2 * n + 1); // CYINIT | CIN

    // Parallel-prefix carry-lookahead. (g, p) means "carry_out = g | (p & cin)".
    // Kogge-Stone scan: the shift feeds in the AND-identity (1) for `p` at the
    // low `d` positions so `P[0..=i]` stays correct for `i < d`; the OR-identity
    // (0) for `g` is already what `g << d` shifts in.
    let mut g = di & !s; // generate: where S=0, the carry becomes DI
    let mut p = s; // propagate
    let mut d = 1usize;
    while d < n {
        let low_d_ones = (1u64 << d) - 1;
        g |= p & (g << d);
        p &= (p << d) | low_d_ones;
        d <<= 1;
    }
    let mask_n = if n == 64 { u64::MAX } else { (1u64 << n) - 1 };
    let cin_mask = 0u64.wrapping_sub(c_in_bit); // all-ones iff c_in_bit == 1
    let carryout = (g | (p & cin_mask)) & mask_n; // C[i+1] for i in 0..n
    let c_vec = ((carryout << 1) | c_in_bit) & mask_n; // C[i] for i in 0..n
    let o = (s ^ c_vec) & mask_n;
    let co = carryout;
    ((co as u128) << n) | (o as u128)
}

/// Pack canonical CARRY4 field values into the input lanes. Convenience for
/// the golden model and tests; the flattener packs bit-by-bit instead.
pub fn carry4_pack(chain_len: usize, s: u64, di: u64, cyinit: bool, cin: bool) -> Vec<u32> {
    let kind = MacroKind::Carry4 { chain_len: chain_len as u16 };
    let mut w = vec![0u32; kind.num_perm_words()];
    let n = 4 * chain_len;
    let mut set = |slot: usize, v: u64| {
        if v & 1 != 0 {
            w[slot / 32] |= 1 << (slot % 32);
        }
    };
    for i in 0..n {
        set(i, s >> i);
        set(n + i, di >> i);
    }
    set(2 * n, cyinit as u64);
    set(2 * n + 1, cin as u64);
    w
}

// ---------------------------------------------------------------------------
// SRLC32E datapath (Part C)
// ---------------------------------------------------------------------------

/// Class R kind tag emitted into script metadata `[16 + num_classc + j]`.
/// `0` is [`MacroKind::Dsp48e2`] — which is what the metadata padding already
/// holds, so an all-DSP partition's script bytes do not move.
pub const MACRO_KIND_TAG_DSP48E2: u32 = 0;
/// Class R kind tag for [`MacroKind::Srl32Shift`].
pub const MACRO_KIND_TAG_SRL32_SHIFT: u32 = 1;
/// Class C kind tag (the high nibble of script metadata `[16 + i]`) for
/// [`MacroKind::Carry4`]; the low 28 bits carry `chain_len`. Tag `0` keeps
/// every existing CARRY4 word numerically unchanged.
pub const CLASSC_KIND_TAG_CARRY4: u32 = 0;
/// Class C kind tag for [`MacroKind::Srl32Read`]. Its 28-bit payload carries
/// the **global 32-bit word index** of the paired [`MacroKind::Srl32Shift`]'s
/// committed state, so the kernel can load all 32 state bits with one
/// `input_state[payload]` instead of routing them through the boomerang.
pub const CLASSC_KIND_TAG_SRL32_READ: u32 = 1;
/// Largest global state word index a Class C payload can address (28 bits).
pub const CLASSC_PAYLOAD_LIMIT: u32 = 1 << 28;

/// CPU twin of `gem_eval_srl32_shift` in `csrc/macro_eval.cuh` — **keep
/// line-for-line in sync**.
///
/// `w[0] & 1` is `D`; `st_cur` is the current 32-bit state, delivered in the
/// low word of the 64-bit Class R slot (the same `macro_p_cur` load the DSP
/// uses for `P`). Returns the next state in the low 32 bits.
///
/// Semantics ([`.claude/question.md`] primitive C): on the rising edge, if
/// `CE == 1` the state shifts left (LSB -> MSB) and `D` loads into index 0.
/// The `CE` gate is *not* here — it rides `MacroBlock::en_iv` (`CLK & CE`) and
/// is applied by the write-out clock enable, so `CE == 0` simply holds the
/// committed state, exactly like a disabled DFF.
pub fn eval_srl32_shift(w: [u32; 4], st_cur: u64) -> u64 {
    (((st_cur as u32) << 1) | (w[0] & 1)) as u64
}

/// CPU twin of `gem_eval_srl32_read` in `csrc/macro_eval.cuh` — **keep
/// line-for-line in sync**.
///
/// `st_cur` is `state[0..32]`, arriving by the zero-copy feedback load from the
/// paired shift half's committed I/O word (the previous cycle's commit, i.e.
/// the pre-edge snapshot, exactly like a DFF `Q`). `w[0] & 0x1f` is `A[4:0]`.
/// Returns `Q` in bit 0.
///
/// `A` is 5 bits so the shift can never be out of range — no clamp, no branch,
/// no divergence, and no 31-deep mux tree of AIG gates.
pub fn eval_srl32_read(st_cur: u32, w: &[u32]) -> u32 {
    (st_cur >> (w[0] & 31)) & 1
}

/// Pack `D` into the [`MacroKind::Srl32Shift`] input lanes. Convenience for
/// the golden model and tests; the flattener packs bit-by-bit instead.
pub fn srl32_shift_pack(d: bool) -> [u32; 4] {
    let mut w = [0u32; 4];
    let slot = MacroKind::Srl32Shift.input_bit_slot(0);
    if d {
        w[slot / 32] |= 1 << (slot % 32);
    }
    w
}

/// Pack `A[4:0]` into the [`MacroKind::Srl32Read`] input lanes. Convenience for
/// the golden model and tests; the state is not packed — it rides the zero-copy
/// feedback load and is passed to [`eval_srl32_read`] separately.
pub fn srl32_read_pack(addr: u32) -> Vec<u32> {
    let kind = MacroKind::Srl32Read;
    let mut w = vec![0u32; kind.num_perm_words()];
    for i in 0..SRL_ADDR_WIDTH {
        if (addr >> i) & 1 != 0 {
            let slot = kind.input_bit_slot(SRL_READ_ADDR_OFFSET + i);
            w[slot / 32] |= 1 << (slot % 32);
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent behavioural reference for one DSP48E2 evaluation.
    fn reference(
        a: i64, d: i64, b: i64, c: i64, state: u32, use_d: bool, rstp: bool, p_cur: i64,
    ) -> i64 {
        let ad_full = a + if use_d { d } else { 0 };
        let ad = sext((ad_full as u64) & 0x07ff_ffff, 27);
        let m = ad * b;
        let mut pn = match state {
            1 => m,
            2 => p_cur + m,
            _ => c,
        };
        if rstp {
            pn = 0;
        }
        pn & 0x0000_ffff_ffff_ffff
    }

    fn check(a: i64, d: i64, b: i64, c: i64, state: u32, use_d: bool, rstp: bool, p_cur: i64) {
        let w = dsp48e2_pack(
            a as u32, d as u32, b as u32, c as u64, state, use_d, rstp,
        );
        let got = eval_dsp48e2(w, p_cur as u64);
        let want = reference(a, d, b, c, state, use_d, rstp, p_cur) as u64
            & 0x0000_ffff_ffff_ffff;
        assert_eq!(
            got, want,
            "a={a} d={d} b={b} c={c} state={state} use_d={use_d} rstp={rstp} p_cur={p_cur}"
        );
    }

    #[test]
    fn input_bit_slots_are_a_permutation() {
        let k = MacroKind::Dsp48e2;
        let mut seen = [false; 128];
        for i in 0..k.num_input_bits() {
            let s = k.input_bit_slot(i);
            assert!(s < 128, "slot {s} for index {i} out of range");
            assert!(!seen[s], "slot {s} used twice (index {i})");
            seen[s] = true;
        }
        assert_eq!(seen.iter().filter(|b| **b).count(), 124);
    }

    #[test]
    fn packing_roundtrips_every_bit() {
        // set exactly one canonical bit at a time, confirm it lands in the
        // slot input_bit_slot() promised and nowhere else.
        let k = MacroKind::Dsp48e2;
        for i in 0..k.num_input_bits() {
            let (mut a, mut d, mut b, mut c, mut op) = (0u32, 0u32, 0u32, 0u64, 0u32);
            let (mut use_d, mut rstp) = (false, false);
            match i {
                0..=26 => a = 1 << i,
                27..=53 => d = 1 << (i - DSP_D_OFFSET),
                54..=71 => b = 1 << (i - DSP_B_OFFSET),
                72..=119 => c = 1 << (i - DSP_C_OFFSET),
                120..=121 => op = 1 << (i - DSP_OPMODE_S_OFFSET),
                122 => use_d = true,
                123 => rstp = true,
                _ => unreachable!(),
            }
            let w = dsp48e2_pack(a, d, b, c, op, use_d, rstp);
            let slot = k.input_bit_slot(i);
            for (word_i, word) in w.iter().enumerate() {
                for bit in 0..32 {
                    let set = (word >> bit) & 1 != 0;
                    let expect = word_i * 32 + bit == slot;
                    assert_eq!(set, expect, "index {i} slot {slot} word {word_i} bit {bit}");
                }
            }
        }
    }

    #[test]
    fn opmode_states() {
        // state 0: bypass -> P = C
        check(0, 0, 0, 12345, 0, false, false, 999);
        check(0, 0, 0, -12345, 0, false, false, 999);
        // state 1: multiply-only -> P = A*B
        check(7, 0, 9, 0, 1, false, false, 999);
        check(-7, 0, 9, 0, 1, false, false, 999);
        // state 2: multiply-accumulate -> P = P + A*B
        check(7, 0, 9, 0, 2, false, false, 1000);
        check(-7, 0, -9, 0, 2, false, false, -1000);
    }

    #[test]
    fn use_d_pre_adder() {
        // USE_D off: AD = A
        check(100, 5, 3, 0, 1, false, false, 0);
        // USE_D on: AD = A + D
        check(100, 5, 3, 0, 1, true, false, 0);
        check(100, -5, 3, 0, 1, true, false, 0);
    }

    #[test]
    fn rstp_zeroes_output() {
        check(7, 0, 9, 0, 2, false, true, 123456);
        check(0, 0, 0, 999, 0, false, true, 0);
    }

    #[test]
    fn signed_extremes() {
        let a_min = -(1i64 << 26);
        let b_min = -(1i64 << 17);
        let c_min = -(1i64 << 47);
        check(a_min, 0, b_min, 0, 1, false, false, 0);
        check(a_min, 0, b_min, 0, 2, false, false, c_min);
        check(0, 0, 0, c_min, 0, false, false, 0);
        check((1 << 26) - 1, 0, (1 << 17) - 1, 0, 1, false, false, 0);
    }

    #[test]
    fn pre_adder_wraps_at_27_bits() {
        // A + D overflowing 27 bits must wrap (mod 2^27) then sign-extend.
        let a = (1i64 << 26) - 1;
        let d = (1i64 << 26) - 1;
        check(a, d, 1, 0, 1, true, false, 0);
        check(a, d, -1, 0, 1, true, false, 0);
    }

    #[test]
    fn accumulator_wraps_at_48_bits() {
        // repeatedly accumulate a large product past 2^47.
        let w = dsp48e2_pack(
            ((1u32 << 26) - 1), 0, ((1u32 << 17) - 1), 0, 2, false, false,
        );
        let mut p_hw: u64 = 0;
        let mut p_ref: i64 = 0;
        let ad = ((1i64 << 26) - 1) as i64;
        let bb = ((1i64 << 17) - 1) as i64;
        for _ in 0..64 {
            p_hw = eval_dsp48e2(w, p_hw);
            p_ref = (p_ref + ad * bb) & 0x0000_ffff_ffff_ffff;
            let want = sext(p_ref as u64, 48) as u64 & 0x0000_ffff_ffff_ffff;
            assert_eq!(p_hw, want);
        }
    }

    #[test]
    fn negative_accumulator_feedback() {
        // P_current sign-extends from 48 bits: a "negative" P must feed back
        // as negative.
        check(3, 0, 4, 0, 2, false, false, -5);
        check(3, 0, 4, 0, 2, false, false, -(1 << 40));
        // and the raw 48-bit-wrapped form of -5 must behave identically.
        let wrapped = ((-5i64) as u64) & 0x0000_ffff_ffff_ffff;
        let w = dsp48e2_pack(3, 0, 4, 0, 2, false, false);
        assert_eq!(eval_dsp48e2(w, wrapped), eval_dsp48e2(w, (-5i64) as u64));
    }

    // -------------------- CARRY4 --------------------

    /// Reference: the spec recurrence, iterated bit by bit over `4*chain_len`
    /// carries. Returns `(O, CO)` each `4*chain_len` bits wide.
    fn carry4_reference(chain_len: usize, s: u64, di: u64, cyinit: bool, cin: bool) -> (u64, u64) {
        let n = 4 * chain_len;
        let mut c = (cyinit as u64) | (cin as u64); // C[0]
        let mut o = 0u64;
        let mut co = 0u64;
        for i in 0..n {
            let si = (s >> i) & 1;
            let dii = (di >> i) & 1;
            o |= (si ^ c) << i; // O[i] = S[i] ^ C[i]
            let c_next = (si & c) | ((si ^ 1) & dii); // C[i+1]
            co |= c_next << i; // CO[i] = C[i+1]
            c = c_next;
        }
        (o, co)
    }

    fn carry4_check(chain_len: usize, s: u64, di: u64, cyinit: bool, cin: bool) {
        let n = 4 * chain_len;
        let w = carry4_pack(chain_len, s, di, cyinit, cin);
        let got = eval_carry4(&w, chain_len);
        let mask = if n == 64 { u64::MAX } else { (1u64 << n) - 1 };
        let (want_o, want_co) = carry4_reference(chain_len, s & mask, di & mask, cyinit, cin);
        let got_o = (got & (mask as u128)) as u64;
        let got_co = ((got >> n) & (mask as u128)) as u64;
        assert_eq!(got_o, want_o, "O mismatch k={chain_len} s={s:#x} di={di:#x} cyinit={cyinit} cin={cin}");
        assert_eq!(got_co, want_co, "CO mismatch k={chain_len} s={s:#x} di={di:#x} cyinit={cyinit} cin={cin}");
    }

    #[test]
    fn carry4_single_slice_exhaustive_s() {
        // all 16 S patterns x representative DI x every {CYINIT, CIN}
        for s in 0u64..16 {
            for &di in &[0u64, 0xf, 0x5, 0xa, 0x3, 0xc] {
                for cyinit in [false, true] {
                    for cin in [false, true] {
                        carry4_check(1, s, di, cyinit, cin);
                    }
                }
            }
        }
    }

    #[test]
    fn carry4_ripple_adder_matches_u64_add() {
        // a k=16 ripple-carry adder: S = a ^ b, DI = a, CYINIT/CIN = carry-in.
        // O must equal the low 64 bits of a + b + cin; the top CO is the
        // carry-out.
        let cases: [(u64, u64, bool); 6] = [
            (0, 0, false),
            (u64::MAX, 1, false),
            (u64::MAX, 0, true),
            (0x0123_4567_89ab_cdef, 0xfedc_ba98_7654_3210, false),
            (0xffff_ffff, 0xffff_ffff, true),
            (0x8000_0000_0000_0000, 0x8000_0000_0000_0000, false),
        ];
        for (a, b, cin) in cases {
            let s = a ^ b;
            let di = a;
            let got = eval_carry4(&carry4_pack(16, s, di, false, cin), 16);
            let o = got as u64;
            let co_top = ((got >> 64) >> 63) & 1;
            let (sum, carry1) = a.overflowing_add(b);
            let (sum, carry2) = sum.overflowing_add(cin as u64);
            assert_eq!(o, sum, "sum mismatch a={a:#x} b={b:#x} cin={cin}");
            assert_eq!(co_top as u8, (carry1 | carry2) as u8, "carry-out mismatch a={a:#x} b={b:#x}");
        }
    }

    #[test]
    fn carry4_prefix_matches_reference_random() {
        // prefix form vs the explicit bit-by-bit ripple, random S/DI, every
        // fused chain length.
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for chain_len in 1..=16usize {
            for _ in 0..64 {
                let s = next();
                let di = next();
                let cyinit = next() & 1 != 0;
                let cin = next() & 1 != 0;
                carry4_check(chain_len, s, di, cyinit, cin);
            }
        }
    }

    #[test]
    fn carry4_packing_roundtrips_every_bit() {
        for chain_len in [1usize, 2, 4, 8, 16] {
            let kind = MacroKind::Carry4 { chain_len: chain_len as u16 };
            let n = kind.num_input_bits();
            let mut seen = vec![false; kind.num_perm_words() * 32];
            for i in 0..n {
                let slot = kind.input_bit_slot(i);
                assert_eq!(slot, i, "carry4 packing must be a plain bit-append");
                assert!(!seen[slot], "slot {slot} reused");
                seen[slot] = true;
            }
        }
    }

    // -------------------- SRLC32E (Part C) --------------------

    #[test]
    fn srl32_read_exhaustive_addresses() {
        // every address x a spread of representative states.
        let states: [u32; 8] = [
            0x0000_0000,
            0xffff_ffff,
            0x5555_5555,
            0xaaaa_aaaa,
            0x8000_0001,
            0x0000_00ff,
            0xff00_0000,
            0x9e37_79b9,
        ];
        for &st in &states {
            for a in 0u32..32 {
                let w = srl32_read_pack(a);
                let got = eval_srl32_read(st, &w);
                let want = (st >> a) & 1;
                assert_eq!(got, want, "state={st:#010x} a={a}");
            }
        }
    }

    #[test]
    fn srl32_shift_64_cycles_matches_u32_reference() {
        // drive a pseudorandom D stream through eval_srl32_shift and an
        // independent u32 shift register; compare every cycle.
        let mut rng = 0x1234_5678u32;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            rng
        };
        let mut hw: u64 = 0;
        let mut refr: u32 = 0;
        for cyc in 0..64 {
            let d = next() & 1 != 0;
            hw = eval_srl32_shift(srl32_shift_pack(d), hw);
            refr = (refr << 1) | (d as u32);
            assert_eq!(hw, refr as u64, "cycle {cyc}");
            assert_eq!(hw >> 32, 0, "cycle {cyc}: the high word must stay clear");
            // the read port must agree with the reference at every address.
            for a in 0u32..32 {
                assert_eq!(
                    eval_srl32_read(hw as u32, &srl32_read_pack(a)),
                    (refr >> a) & 1,
                    "cycle {cyc} addr {a}"
                );
            }
        }
    }

    #[test]
    fn srl32_q31_is_state_bit_31() {
        // Q31 = state[31] must be the bit that entered at D 32 cycles ago.
        let stream: Vec<bool> = (0..96).map(|i| (i * 7 + 3) % 5 < 2).collect();
        let mut hw: u64 = 0;
        for (k, &d) in stream.iter().enumerate() {
            hw = eval_srl32_shift(srl32_shift_pack(d), hw);
            let q31 = ((hw >> 31) & 1) != 0;
            if k >= 31 {
                assert_eq!(q31, stream[k - 31], "cycle {k}: Q31 must be D delayed 32 cycles");
            } else {
                assert!(!q31, "cycle {k}: Q31 must still be the initial 0");
            }
            // Q31 is literally the same bit the addressed read returns at A=31.
            assert_eq!(
                eval_srl32_read(hw as u32, &srl32_read_pack(31)),
                q31 as u32
            );
        }
    }

    #[test]
    fn srl32_packing_roundtrips_every_bit() {
        // both halves pack as a plain bit-append, with no slot reused and
        // everything inside the declared lane count.
        for kind in [MacroKind::Srl32Shift, MacroKind::Srl32Read] {
            let mut seen = vec![false; kind.num_perm_words() * 32];
            for i in 0..kind.num_input_bits() {
                let slot = kind.input_bit_slot(i);
                assert_eq!(slot, i, "{kind:?} packing must be a plain bit-append");
                assert!(slot < seen.len(), "{kind:?} slot {slot} outside its lanes");
                assert!(!seen[slot], "{kind:?} slot {slot} reused");
                seen[slot] = true;
            }
        }
        assert_eq!(MacroKind::Srl32Shift.num_input_bits(), 1);
        assert_eq!(MacroKind::Srl32Shift.num_output_bits(), SRL_STATE_WIDTH);
        assert_eq!(MacroKind::Srl32Read.num_input_bits(), SRL_READ_NUM_INPUT_BITS);
        assert_eq!(MacroKind::Srl32Read.num_output_bits(), 1);
        // the Class R footprint must stay uniform: the kernel indexes lanes as
        // `macro_lane >> 2` and stores `wo_base_macro + m_i*2 + sub`.
        assert_eq!(
            MacroKind::Srl32Shift.num_perm_words(),
            MacroKind::Dsp48e2.num_perm_words()
        );
        assert_eq!(
            MacroKind::Srl32Shift.num_state_words(),
            MacroKind::Dsp48e2.num_state_words()
        );
        // the read port's chosen packing is what makes the datapath trivial:
        // `A` sits in the low 5 bits of a single lane, and the 32 state bits
        // are not lanes at all — they arrive by the zero-copy feedback load.
        assert_eq!(MacroKind::Srl32Read.input_bit_slot(SRL_READ_ADDR_OFFSET), 0);
        assert_eq!(MacroKind::Srl32Read.num_input_bits(), SRL_ADDR_WIDTH);
        assert_eq!(MacroKind::Srl32Read.num_perm_words(), 1);
        assert_eq!(MacroKind::Srl32Read.num_state_words(), 1);
    }

    #[test]
    fn srl32_shift_datapath_is_branchless_select_safe() {
        // The kernel computes *both* Class R datapaths on every lane and
        // selects with a mask (`pn = (pn_dsp & ~sel) | (pn_srl & sel)`), so
        // neither may trap or depend on the other's lane contents. Feed the
        // SRL datapath DSP-shaped lanes and vice versa; both must be total.
        let dsp_w = dsp48e2_pack(0x7ff_ffff, 0x7ff_ffff, 0x3ffff, u64::MAX, 2, true, false);
        let srl_w = srl32_shift_pack(true);
        for (w, p) in [(dsp_w, u64::MAX), (srl_w, u64::MAX), (dsp_w, 0), (srl_w, 0)] {
            let pn_dsp = eval_dsp48e2(w, p);
            let pn_srl = eval_srl32_shift(w, p);
            let sel = 0u64.wrapping_sub(1); // pretend "this lane is an SRL"
            assert_eq!((pn_dsp & !sel) | (pn_srl & sel), pn_srl);
            let sel = 0u64;
            assert_eq!((pn_dsp & !sel) | (pn_srl & sel), pn_dsp);
        }
    }
}
