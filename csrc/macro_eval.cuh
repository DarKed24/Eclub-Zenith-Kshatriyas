// SPDX-FileCopyrightText: Copyright (c) 2024 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Branchless native datapaths for word-level hardware macros:
//   Part A: the DSP48E2 MAC   (gem_eval_dsp48e2     <-> eval_dsp48e2)
//   Part B: the CARRY4 chain  (gem_eval_carry4      <-> eval_carry4)
//   Part C: the SRLC32E SRL   (gem_eval_srl32_shift <-> eval_srl32_shift,
//                              gem_eval_srl32_read  <-> eval_srl32_read)
// Marked __host__ __device__ so the exact same code is host-unit-testable
// once nvcc is available.
//
// CPU twins live in `src/hwmacro.rs`. KEEP THESE LINE-FOR-LINE IN SYNC.

#pragma once

#include <crates/ulib/includes.hpp>

/// Sign-extend the low `w` bits of `v` to a full 64-bit signed value.
__host__ __device__ __forceinline__ long long gem_sext(unsigned long long v, int w) {
  return ((long long)(v << (64 - w))) >> (64 - w);
}

/// Evaluate one DSP48E2 MAC (PREG=1, every other register combinational,
/// OPMODE pre-decoded by the frontend into the 2-bit state carried in
/// `w0[28:27]`).
///
/// `w0..w3` are the four packed input lanes; see `MacroKind::input_bit_slot`
/// in `src/hwmacro.rs` for the exact bit assignment:
///   lane 0: A[26:0] @ 0, OPMODE_S[1:0] @ 27, USE_D @ 29, RSTP @ 30
///   lane 1: D[26:0] @ 0, C[36:32] @ 27
///   lane 2: B[17:0] @ 0, C[47:37] @ 18
///   lane 3: C[31:0] @ 0
///
/// `p_cur` is the current `P` register (only the low 48 bits matter). Returns
/// `P_next` masked to 48 bits (OVERFLOW/UNDERFLOW ignored per spec).
__host__ __device__ __forceinline__ unsigned long long gem_eval_dsp48e2(
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

  u32 state = (w0 >> 27) & 3u;
  u32 use_d = (w0 >> 29) & 1u;
  u32 rstp  = (w0 >> 30) & 1u;

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

// --------------------------------------------------------------------------
// CARRY4 carry chain (Part B).  CPU twin: `eval_carry4` in src/hwmacro.rs.
// --------------------------------------------------------------------------

/// packed input lanes a fused `Carry4 { chain_len: k }` occupies (power of two).
__host__ __device__ __forceinline__ int gem_carry4_perm_words(int k) {
  int w = (8 * k + 2 + 31) / 32;
  int p = 1;
  while (p < w) p <<= 1;
  return p > 8 ? 8 : p;
}
/// staged-IO scratch words a fused `Carry4 { chain_len: k }` occupies.
__host__ __device__ __forceinline__ int gem_carry4_state_words(int k) {
  int w = (8 * k + 31) / 32;
  int p = 1;
  while (p < w) p <<= 1;
  return p > 4 ? 4 : p;
}

/// Evaluate one CARRY4 slice / fused cascade. `w` holds the packed input lanes
/// (S[0..4k] at bits 0..4k, DI[0..4k] at 4k..8k, CYINIT at 8k, CIN at 8k+1);
/// `chain_len` is `k` (<= 16, so `n = 4k <= 64`). Writes
/// `gem_carry4_state_words(k)` result words to `out`: O[0..4k] in bits 0..4k,
/// CO[0..4k] in bits 4k..8k.
///
/// Kogge-Stone parallel-prefix carry-lookahead on (g,p) = (DI & ~S, S). The
/// `p` scan feeds in the AND-identity (1) at the low `d` bits so a carry-in
/// through S=1 at bit 0 is not dropped.
__host__ __device__ __forceinline__ void gem_eval_carry4(
    const u32 *w, int chain_len, u32 *out)
{
  int n = 4 * chain_len;
  unsigned long long s = 0, di = 0;
  for (int i = 0; i < n; ++i) {
    s  |= (unsigned long long)((w[i >> 5] >> (i & 31)) & 1u) << i;
    di |= (unsigned long long)((w[(n + i) >> 5] >> ((n + i) & 31)) & 1u) << i;
  }
  unsigned long long c_in_bit =
      (unsigned long long)((w[(2 * n) >> 5] >> ((2 * n) & 31)) & 1u) |
      (unsigned long long)((w[(2 * n + 1) >> 5] >> ((2 * n + 1) & 31)) & 1u);

  unsigned long long g = di & ~s;
  unsigned long long p = s;
  for (int d = 1; d < n; d <<= 1) {
    unsigned long long low_d_ones = (1ULL << d) - 1ULL;
    g |= p & (g << d);
    p &= (p << d) | low_d_ones;
  }
  unsigned long long mask_n = (n == 64) ? ~0ULL : ((1ULL << n) - 1ULL);
  unsigned long long cin_mask = 0ULL - c_in_bit;
  unsigned long long carryout = (g | (p & cin_mask)) & mask_n; // C[i+1]
  unsigned long long c_vec = ((carryout << 1) | c_in_bit) & mask_n; // C[i]
  unsigned long long o = (s ^ c_vec) & mask_n;
  unsigned long long co = carryout;

  unsigned long long v_lo, v_hi;
  if (n == 64) { v_lo = o; v_hi = co; }
  else { v_lo = o | (co << n); v_hi = co >> (64 - n); }

  u32 res[4] = { (u32)v_lo, (u32)(v_lo >> 32), (u32)v_hi, (u32)(v_hi >> 32) };
  int words = gem_carry4_state_words(chain_len);
  for (int j = 0; j < words; ++j) out[j] = res[j];
}

// --------------------------------------------------------------------------
// SRLC32E shift-register LUT (Part C).
// CPU twins: `eval_srl32_shift` / `eval_srl32_read` in src/hwmacro.rs.
//
// One SRLC32E is two macro endpoints joined by a zero-copy state feedback: a
// Class R half owning the 32-bit shift state, and a Class C half doing the
// dynamically addressed read straight out of the shift half's committed I/O
// word. `Q31` is literally `state[31]`, so it needs no evaluation at all.
// --------------------------------------------------------------------------

/// Class R macro kind tags, script metadata [16 + num_classc_macros + j].
/// DSP48E2 is 0 so an all-DSP partition's table is the metadata padding.
#define GEM_MACRO_DSP48E2      0
#define GEM_MACRO_SRL32_SHIFT  1

/// Class C macro kind tags, the high nibble of script metadata [16 + i].
/// CARRY4 is 0 with `chain_len` as its payload, so Part B words are unchanged.
/// SRL32_READ's payload is instead the GLOBAL 32-bit word index of the paired
/// shift half's committed state, which the read half loads zero-copy out of
/// `input_state` rather than taking those 32 bits through the boomerang.
#define GEM_CLASSC_CARRY4      0
#define GEM_CLASSC_SRL32_READ  1

/// Evaluate the clocked half of one SRLC32E: `state' = (state << 1) | D`.
///
/// `w0` is the single packed input lane (`D` in bit 0); `st_cur` is the
/// current state, arriving in the low 32 bits of the same 64-bit Class R
/// feedback load the DSP uses for `P`. Returns the next state in the low 32
/// bits, with the high 32 clear.
///
/// `CE` is NOT applied here — it rides `en_iv = CLK & CE` and is applied by
/// the write-out clock enable, so `CE == 0` simply holds the committed state.
__host__ __device__ __forceinline__ unsigned long long gem_eval_srl32_shift(
    u32 w0, unsigned long long st_cur)
{
  return (unsigned long long)(((u32)st_cur << 1) | (w0 & 1u));
}

/// packed input lanes / scratch words a `Srl32Read` occupies. Fixed: its only
/// boomerang input is `A[4:0]`, so 5 input bits pack into 1 lane, and the 1
/// output bit into 1 scratch word. The 32 state bits are NOT lanes — they
/// arrive by the zero-copy feedback load.
__host__ __device__ __forceinline__ int gem_srl32_read_perm_words() { return 1; }
__host__ __device__ __forceinline__ int gem_srl32_read_state_words() { return 1; }

/// Evaluate the combinational read port of one SRLC32E: `Q = state[A]`.
///
/// `st_cur` is state[0..32], loaded straight out of the shift half's committed
/// I/O word (the previous cycle's commit, i.e. the pre-edge snapshot, exactly
/// like a DFF `Q`); `w[0] & 0x1f` is A[4:0]. `A` is 5 bits so the shift can
/// never be out of range — no clamp, no branch, no divergence.
__host__ __device__ __forceinline__ void gem_eval_srl32_read(
    u32 st_cur, const u32 *w, u32 *out)
{
  out[0] = (st_cur >> (w[0] & 31u)) & 1u;
}

/// packed input lanes of Class C macro with tagged metadata word
/// `tag = kind << 28 | payload`.
__host__ __device__ __forceinline__ int gem_classc_perm_words(u32 tag) {
  return (tag >> 28) == GEM_CLASSC_SRL32_READ
    ? gem_srl32_read_perm_words()
    : gem_carry4_perm_words((int)(tag & 0x0fffffffu));
}
/// staged-IO scratch words of Class C macro with tagged metadata word `tag`.
__host__ __device__ __forceinline__ int gem_classc_state_words(u32 tag) {
  return (tag >> 28) == GEM_CLASSC_SRL32_READ
    ? gem_srl32_read_state_words()
    : gem_carry4_state_words((int)(tag & 0x0fffffffu));
}
