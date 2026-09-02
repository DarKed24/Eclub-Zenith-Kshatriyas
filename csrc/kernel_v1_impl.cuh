// SPDX-FileCopyrightText: Copyright (c) 2024 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0

#include <crates/ulib/includes.hpp>
#include <cstdio>
#include <cooperative_groups.h>
#include "macro_eval.cuh"

struct alignas(8) VectorRead2 {
  u32 c1, c2;

  __device__ __forceinline__ void read(const VectorRead2 *t) {
    *this = *t;
  }
};

struct alignas(16) VectorRead4 {
  u32 c1, c2, c3, c4;

  __device__ __forceinline__ void read(const VectorRead4 *t) {
    *this = *t;
  }
};

__device__ void simulate_block_v1(
  const u32 *__restrict__ script,
  usize script_size,
  const u32 *__restrict__ input_state,
  u32 *__restrict__ output_state,
  u32 *__restrict__ sram_data,
  u32 *__restrict__ shared_metadata,
  u32 *__restrict__ shared_writeouts,
  u32 *__restrict__ shared_state
  )
{
  int script_pi = 0;
  while(true) {
    VectorRead2 t2_1, t2_2;
    VectorRead4 t4_1, t4_2, t4_3, t4_4, t4_5;
    shared_metadata[threadIdx.x] = script[script_pi + threadIdx.x];
    script_pi += 256;
    t2_1.read(((const VectorRead2 *)(script + script_pi)) + threadIdx.x);
    __syncthreads();
    int num_stages = shared_metadata[0];
    if(!num_stages) {
      break;
    }
    int is_last_part = shared_metadata[1];
    int num_ios = shared_metadata[2];
    int io_offset = shared_metadata[3];
    int num_srams = shared_metadata[4];
    int sram_offset = shared_metadata[5];
    int num_global_read_rounds = shared_metadata[6];
    int num_output_duplicates = shared_metadata[7];
    // [8..12]: Class R word-level macro section. Zero for a macro-free
    // partition, in which case the legacy [normal|dup|sram] layout arithmetic
    // still holds.  [12..16] + [16+i]: Class C (combinational) macro section.
    int num_macros = shared_metadata[8];
    int num_classc_macros = shared_metadata[12];
    int wo_base_dup, wo_base_sram, wo_base_macro;
    if(num_macros || num_classc_macros) {
      wo_base_dup   = shared_metadata[9];
      wo_base_sram  = shared_metadata[10];
      wo_base_macro = shared_metadata[11];
    }
    else {
      wo_base_dup   = num_ios - num_srams - num_output_duplicates;
      wo_base_sram  = num_ios - num_srams;
      wo_base_macro = num_ios;
    }
    int classc_perm_base = 0, classc_scratch_base = 0, classc_perm_words = 0;
    if(num_classc_macros) {
      classc_perm_base    = shared_metadata[13];
      classc_scratch_base = shared_metadata[14];
      classc_perm_words   = shared_metadata[15];
    }
    u32 writeout_hook_i = shared_metadata[128 + threadIdx.x / 2];
    if(threadIdx.x % 2 == 0) {
      writeout_hook_i = writeout_hook_i & ((1 << 16) - 1);
    }
    else {
      writeout_hook_i = writeout_hook_i >> 16;
    }

    t4_1.read((const VectorRead4 *)(script + script_pi + 256 * 2 * num_global_read_rounds) + threadIdx.x);
    t4_2.read((const VectorRead4 *)(script + script_pi + 256 * 2 * num_global_read_rounds + 256 * 4) + threadIdx.x);
    t4_3.read((const VectorRead4 *)(script + script_pi + 256 * 2 * num_global_read_rounds + 256 * 4 * 2) + threadIdx.x);
    t4_4.read((const VectorRead4 *)(script + script_pi + 256 * 2 * num_global_read_rounds + 256 * 4 * 3) + threadIdx.x);
    t4_5.read((const VectorRead4 *)(script + script_pi + 256 * 2 * num_global_read_rounds + 256 * 4 * 4) + threadIdx.x);
    u32 t_global_rd_state = 0;
    for(int gr_i = 0; gr_i < num_global_read_rounds; gr_i += 2) {
      u32 idx = t2_1.c1;
      u32 mask = t2_1.c2;
      script_pi += 256 * 2;
      t2_2.read(((const VectorRead2 *)(script + script_pi)) + threadIdx.x);
      if(mask) {
        const u32 *real_input_array;
        if(idx >> 31) real_input_array = output_state - (1 << 31);
        else real_input_array = input_state;
        u32 value = real_input_array[idx];
        while(mask) {
          t_global_rd_state <<= 1;
          u32 lowbit = mask & -mask;
          if(value & lowbit) t_global_rd_state |= 1;
          mask ^= lowbit;
        }
      }

      if(gr_i + 1 >= num_global_read_rounds) break;
      idx = t2_2.c1;
      mask = t2_2.c2;
      script_pi += 256 * 2;
      t2_1.read(((const VectorRead2 *)(script + script_pi)) + threadIdx.x);
      if(mask) {
        const u32 *real_input_array;
        if(idx >> 31) real_input_array = output_state - (1 << 31);
        else real_input_array = input_state;
        u32 value = real_input_array[idx];
        while(mask) {
          t_global_rd_state <<= 1;
          u32 lowbit = mask & -mask;
          if(value & lowbit) t_global_rd_state |= 1;
          mask ^= lowbit;
        }
      }
    }
    shared_state[threadIdx.x] = t_global_rd_state;
    __syncthreads();

    for(int bs_i = 0; bs_i < num_stages; ++bs_i) {
      u32 hier_input = 0, hier_flag_xora = 0, hier_flag_xorb = 0, hier_flag_orb = 0;
#define GEMV1_SHUF_INPUT_K(k_outer, k_inner, t_shuffle) {           \
        u32 k = k_outer * 4 + k_inner;                              \
        u32 t_shuffle_1_idx = t_shuffle & ((1 << 16) - 1);          \
        u32 t_shuffle_2_idx = t_shuffle >> 16;                      \
                                                                    \
        hier_input |= (shared_state[t_shuffle_1_idx >> 5] >>        \
                       (t_shuffle_1_idx & 31) & 1) << (k * 2);      \
        hier_input |= (shared_state[t_shuffle_2_idx >> 5] >>        \
                       (t_shuffle_2_idx & 31) & 1) << (k * 2 + 1);  \
      }
#define GEMV1_SHUF_INPUT_K_4(k_outer, t_shuffle) {    \
        GEMV1_SHUF_INPUT_K(k_outer, 0, t_shuffle.c1); \
        GEMV1_SHUF_INPUT_K(k_outer, 1, t_shuffle.c2); \
        GEMV1_SHUF_INPUT_K(k_outer, 2, t_shuffle.c3); \
        GEMV1_SHUF_INPUT_K(k_outer, 3, t_shuffle.c4); \
      }
      script_pi += 256 * 4 * 5;
      GEMV1_SHUF_INPUT_K_4(0, t4_1);
      t4_1.read(((const VectorRead4 *)(script + script_pi)) + threadIdx.x);
      GEMV1_SHUF_INPUT_K_4(1, t4_2);
      t4_2.read(((const VectorRead4 *)(script + script_pi + 256 * 4)) + threadIdx.x);
      GEMV1_SHUF_INPUT_K_4(2, t4_3);
      t4_3.read(((const VectorRead4 *)(script + script_pi + 256 * 4 * 2)) + threadIdx.x);
      GEMV1_SHUF_INPUT_K_4(3, t4_4);
      t4_4.read(((const VectorRead4 *)(script + script_pi + 256 * 4 * 3)) + threadIdx.x);
#undef GEMV1_SHUF_INPUT_K
#undef GEMV1_SHUF_INPUT_K_4
      hier_flag_xora = t4_5.c1;
      hier_flag_xorb = t4_5.c2;
      hier_flag_orb = t4_5.c3;
      t4_5.read(((const VectorRead4 *)(script + script_pi + 256 * 4 * 4)) + threadIdx.x);

      __syncthreads();
      shared_state[threadIdx.x] = hier_input;
      __syncthreads();

      // hier[0]
      if(threadIdx.x >= 128) {
        u32 hier_input_a = shared_state[threadIdx.x - 128];
        u32 hier_input_b = hier_input;
        u32 ret = (hier_input_a ^ hier_flag_xora) & ((hier_input_b ^ hier_flag_xorb) | hier_flag_orb);
        shared_state[threadIdx.x] = ret;
      }
      __syncthreads();
      // hier[1..3]
      u32 tmp_cur_hi;
      for(int hi = 1; hi <= 3; ++hi) {
        int hier_width = 1 << (7 - hi);
        if(threadIdx.x >= hier_width && threadIdx.x < hier_width * 2) {
          u32 hier_input_a = shared_state[threadIdx.x + hier_width];
          u32 hier_input_b = shared_state[threadIdx.x + hier_width * 2];
          u32 ret = (hier_input_a ^ hier_flag_xora) & ((hier_input_b ^ hier_flag_xorb) | hier_flag_orb);
          tmp_cur_hi = ret;
          shared_state[threadIdx.x] = ret;
        }
        __syncthreads();
      }
      // hier[4..7], within the first warp.
      if(threadIdx.x < 32) {
        for(int hi = 4; hi <= 7; ++hi) {
          int hier_width = 1 << (7 - hi);
          u32 hier_input_a = __shfl_down_sync(0xffffffff, tmp_cur_hi, hier_width);
          u32 hier_input_b = __shfl_down_sync(0xffffffff, tmp_cur_hi, hier_width * 2);
          if(threadIdx.x >= hier_width && threadIdx.x < hier_width * 2) {
            tmp_cur_hi = (hier_input_a ^ hier_flag_xora) & ((hier_input_b ^ hier_flag_xorb) | hier_flag_orb);
          }
        }
        u32 v1 = __shfl_down_sync(0xffffffff, tmp_cur_hi, 1);
        // hier[8..12]
        if(threadIdx.x == 0) {
          u32 r8 = ((v1 << 16) ^ hier_flag_xora) & ((v1 ^ hier_flag_xorb) | hier_flag_orb) & 0xffff0000;
          u32 r9 = ((r8 >> 8) ^ hier_flag_xora) & (((r8 >> 16) ^ hier_flag_xorb) | hier_flag_orb) & 0xff00;
          u32 r10 = ((r9 >> 4) ^ hier_flag_xora) & (((r9 >> 8) ^ hier_flag_xorb) | hier_flag_orb) & 0xf0;
          u32 r11 = ((r10 >> 2) ^ hier_flag_xora) & (((r10 >> 4) ^ hier_flag_xorb) | hier_flag_orb) & 12 /* 0b1100 */;
          u32 r12 = ((r11 >> 1) ^ hier_flag_xora) & (((r11 >> 2) ^ hier_flag_xorb) | hier_flag_orb) & 2 /* 0b10 */;
          tmp_cur_hi = r8 | r9 | r10 | r11 | r12;
        }
        shared_state[threadIdx.x] = tmp_cur_hi;
      }
      __syncthreads();

      // write out
      if((writeout_hook_i >> 8) == bs_i) {
        shared_writeouts[threadIdx.x] = shared_state[writeout_hook_i & 255];
      }
    }
    __syncthreads();

    // sram & duplicate permutation
    u32 sram_duplicate_t = 0;
#define GEMV1_SHUF_SRAM_DUPL_K(k_outer, k_inner, t_shuffle) { \
      u32 k = k_outer * 4 + k_inner;                          \
      u32 t_shuffle_1_idx = t_shuffle & ((1 << 16) - 1);      \
      u32 t_shuffle_2_idx = t_shuffle >> 16;                  \
                                                              \
      sram_duplicate_t |=                                     \
        (shared_writeouts[t_shuffle_1_idx >> 5] >>            \
         (t_shuffle_1_idx & 31) & 1) << (k * 2);              \
      sram_duplicate_t |=                                     \
        (shared_writeouts[t_shuffle_2_idx >> 5] >>            \
         (t_shuffle_2_idx & 31) & 1) << (k * 2 + 1);          \
    }
#define GEMV1_SHUF_SRAM_DUPL_K_4(k_outer, t_shuffle) {  \
      GEMV1_SHUF_SRAM_DUPL_K(k_outer, 0, t_shuffle.c1); \
      GEMV1_SHUF_SRAM_DUPL_K(k_outer, 1, t_shuffle.c2); \
      GEMV1_SHUF_SRAM_DUPL_K(k_outer, 2, t_shuffle.c3); \
      GEMV1_SHUF_SRAM_DUPL_K(k_outer, 3, t_shuffle.c4); \
    }
    script_pi += 256 * 4 * 5;
    GEMV1_SHUF_SRAM_DUPL_K_4(0, t4_1);
    t4_1.read(((const VectorRead4 *)(script + script_pi)) + threadIdx.x);
    GEMV1_SHUF_SRAM_DUPL_K_4(1, t4_2);
    t4_2.read(((const VectorRead4 *)(script + script_pi + 256 * 4)) + threadIdx.x);
    GEMV1_SHUF_SRAM_DUPL_K_4(2, t4_3);
    t4_3.read(((const VectorRead4 *)(script + script_pi + 256 * 4 * 2)) + threadIdx.x);
    GEMV1_SHUF_SRAM_DUPL_K_4(3, t4_4);
    t4_4.read(((const VectorRead4 *)(script + script_pi + 256 * 4 * 3)) + threadIdx.x);
#undef GEMV1_SHUF_SRAM_DUPL_K_4
#undef GEMV1_SHUF_SRAM_DUPL_K
    sram_duplicate_t = (sram_duplicate_t & ~t4_5.c2) ^ t4_5.c1;
    t4_5.read(((const VectorRead4 *)(script + script_pi + 256 * 4 * 4)) + threadIdx.x);

    // Word-level macro (DSP48E2) worker. Its 4 lanes are threads
    // [num_srams*4 + m*4 .. +4); because 4 divides 32 they never straddle a
    // warp boundary, so a full-mask shuffle gathers the four packed input
    // words safely. Every lane of every warp participates (uniform scope) —
    // non-macro lanes just compute values they discard, so no divergence.
    int macro_lane = (int)threadIdx.x - num_srams * 4;
    bool is_macro_thread = num_macros && macro_lane >= 0 && macro_lane < num_macros * 4;
    u32 macro_w0 = 0, macro_w1 = 0, macro_w2 = 0, macro_w3 = 0;
    if(num_macros) {
      int lane0 = (threadIdx.x & 31) & ~3;
      macro_w0 = __shfl_sync(0xffffffff, sram_duplicate_t, lane0 + 0);
      macro_w1 = __shfl_sync(0xffffffff, sram_duplicate_t, lane0 + 1);
      macro_w2 = __shfl_sync(0xffffffff, sram_duplicate_t, lane0 + 2);
      macro_w3 = __shfl_sync(0xffffffff, sram_duplicate_t, lane0 + 3);
    }
    // Issue the P feedback load early, next to the SRAM read, so its latency
    // overlaps the clock-enable permutation. All 4 lanes of a macro load the
    // same u64 (redundant but divergence-free); a warp's macros touch
    // contiguous 8-byte-aligned pairs, so it is a coalesced pair of sectors.
    unsigned long long macro_p_cur = 0;
    if(is_macro_thread) {
      int m_i = macro_lane >> 2;
      macro_p_cur = ((const unsigned long long *)
                     (input_state + io_offset + wo_base_macro))[m_i];
    }

    // Class C zero-copy state feedback. A Srl32Read takes its 32 state bits
    // straight out of the paired shift half's committed I/O word, whose GLOBAL
    // index its metadata payload carries, instead of routing those 32 bits
    // through the boomerang. Issued here rather than inside the Class C loop
    // below so every macro's load is in flight at once (each is a different
    // head lane, so they are independent) and their latency overlaps the
    // clock-enable permutation — inside the loop they would serialize, one
    // full memory round trip per Class C macro.
    //
    // The lane walk is the same uniform stride computation the loop below
    // repeats, so a thread that is head lane of macro `ci` here is head lane of
    // macro `ci` there and simply reuses this register.
    u32 classc_fb = 0;
    // Uniform role scan (perf fix, Nsight-driven): every thread walks the Class
    // C metadata ONCE with integer ops only and records whether it is the head
    // lane of some macro (tag / lane offset / scratch word offset). The
    // feedback load below and the evaluation further down then run as a
    // single predicated pass with every head lane active at the same time.
    // Before this change both were done inside the serial per-macro loop, so
    // a warp evaluated its macros one at a time with the other lanes idle and
    // issued one dependent global load per SRL read - measured at roughly
    // 0.1-0.3 us per Class C macro per cycle.
    u32 my_classc_tag = 0;
    int my_classc_lane = -1, my_classc_word = 0;
    if(num_classc_macros) {
      int lane_off = 0, word_off = 0;
      for(int ci = 0; ci < num_classc_macros; ++ci) {
        u32 tag = shared_metadata[16 + ci];
        int pw = gem_classc_perm_words(tag);
        int sw = gem_classc_state_words(tag);
        if((int)threadIdx.x == classc_perm_base + lane_off) {
          my_classc_tag = tag;
          my_classc_lane = lane_off;
          my_classc_word = word_off;
        }
        lane_off += pw;
        word_off += sw;
      }
      if(my_classc_lane >= 0 && (my_classc_tag >> 28) == GEM_CLASSC_SRL32_READ)
        classc_fb = input_state[my_classc_tag & 0x0fffffffu];
    }

    // sram read fires here.
    u32 *ram = nullptr;
    u32 r, w0;
    u32 port_w_addr_iv, port_w_wr_en, port_w_wr_data_iv;
    if(threadIdx.x < num_srams * 4) {
      u32 addrs = sram_duplicate_t;
      u32 last_tid = 32 + threadIdx.x / 32 * 32;
      u32 mask = (last_tid <= num_srams * 4)
        ? 0xffffffff : (0xffffffff >> (last_tid - num_srams * 4));
      port_w_wr_en = __shfl_down_sync(mask, sram_duplicate_t, 1);
      port_w_wr_data_iv = __shfl_down_sync(mask, sram_duplicate_t, 2);

      if(threadIdx.x % 4 == 0) {
        u32 sram_i = threadIdx.x / 4;
        u32 sram_st = sram_offset + sram_i * (1 << 13);
        // u32 sram_ed = sram_st + (1 << 13);
        u32 port_r_addr_iv = addrs & 0xffff;
        port_w_addr_iv = addrs >> 16;

        ram = sram_data + sram_st;
        r = ram[port_r_addr_iv];
        w0 = ram[port_w_addr_iv];
      }
    }
    // __syncthreads();

    // clock enable permutation
    u32 clken_perm = 0;
#define GEMV1_SHUF_CLKEN_K(k_outer, k_inner, t_shuffle) { \
      u32 k = k_outer * 4 + k_inner;                      \
      u32 t_shuffle_1_idx = t_shuffle & ((1 << 16) - 1);  \
      u32 t_shuffle_2_idx = t_shuffle >> 16;              \
                                                          \
      clken_perm |=                                       \
        (shared_writeouts[t_shuffle_1_idx >> 5] >>        \
         (t_shuffle_1_idx & 31) & 1) << (k * 2);          \
      clken_perm |=                                       \
        (shared_writeouts[t_shuffle_2_idx >> 5] >>        \
         (t_shuffle_2_idx & 31) & 1) << (k * 2 + 1);      \
    }
#define GEMV1_SHUF_CLKEN_K_4(k_outer, t_shuffle) {  \
      GEMV1_SHUF_CLKEN_K(k_outer, 0, t_shuffle.c1); \
      GEMV1_SHUF_CLKEN_K(k_outer, 1, t_shuffle.c2); \
      GEMV1_SHUF_CLKEN_K(k_outer, 2, t_shuffle.c3); \
      GEMV1_SHUF_CLKEN_K(k_outer, 3, t_shuffle.c4); \
    }
    script_pi += 256 * 4 * 5;
    GEMV1_SHUF_CLKEN_K_4(0, t4_1);
    GEMV1_SHUF_CLKEN_K_4(1, t4_2);
    GEMV1_SHUF_CLKEN_K_4(2, t4_3);
    GEMV1_SHUF_CLKEN_K_4(3, t4_4);
#undef GEMV1_SHUF_CLKEN_K
#undef GEMV1_SHUF_CLKEN_K_4

    // sram commit
    if(threadIdx.x < num_srams * 4) {
      if(threadIdx.x % 4 == 0) {
        u32 sram_i = threadIdx.x / 4;
        shared_writeouts[wo_base_sram + sram_i] = r;
        ram[port_w_addr_iv] = (w0 & ~port_w_wr_en) | (port_w_wr_data_iv & port_w_wr_en);
      }
    }
    else if(threadIdx.x >= num_srams * 4 + num_macros * 4 + classc_perm_words &&
            threadIdx.x < num_srams * 4 + num_macros * 4 + classc_perm_words + num_output_duplicates) {
      shared_writeouts[wo_base_dup + (threadIdx.x - num_srams * 4 - num_macros * 4 - classc_perm_words)] = sram_duplicate_t;
    }

    // Class R macro commit: evaluate the branchless datapaths (all 4 lanes,
    // redundantly) and predicate-store the state lo/hi from lanes sub<2. This
    // lands in shared_writeouts after both permutation gathers have read it
    // and before the __syncthreads() below, so the final commit picks it up.
    //
    // Two Class R kinds share this block (DSP48E2 and the SRLC32E shift half).
    // Different macros within one warp may differ in kind, so rather than
    // branching on it we compute BOTH datapaths and select with a mask — the
    // same idiom gem_eval_dsp48e2 already uses for OPMODE_S. Every lane runs
    // an identical instruction stream, so the block stays divergence-free.
    if(is_macro_thread) {
      int m_i = macro_lane >> 2;
      int sub = macro_lane & 3;
      u32 kind = shared_metadata[16 + num_classc_macros + m_i];
      unsigned long long pn_dsp =
        gem_eval_dsp48e2(macro_w0, macro_w1, macro_w2, macro_w3, macro_p_cur);
      unsigned long long pn_srl = gem_eval_srl32_shift(macro_w0, macro_p_cur);
      unsigned long long sel =
        0ULL - (unsigned long long)(kind == GEM_MACRO_SRL32_SHIFT);
      unsigned long long pn = (pn_dsp & ~sel) | (pn_srl & sel);
      u32 pv = sub ? (u32)(pn >> 32) : (u32)pn;
      if(sub < 2) {
        shared_writeouts[wo_base_macro + m_i * 2 + sub] = pv;
      }
    }

    // Class C (combinational) macro worker. The packed input lanes may not be
    // warp-aligned, so instead of a shuffle gather every thread parks its
    // permute word in shared_state and the head lane of each Class C macro
    // reads its lanes back, evaluates the branchless datapath, and
    // predicate-stores the packed result into the staged-IO scratch section of
    // shared_writeouts (which the unconditional clken below then commits every
    // cycle; the next major stage reads it via the current-iteration global
    // read). CARRY4 is purely combinational; the SRLC32E read port folds in
    // `classc_fb`, the zero-copy state word loaded above.
    //
    // The metadata word is tagged `kind << 28 | payload`, so the lane/word
    // stride walk stays uniform across the whole block; only the datapath
    // call inside the already-predicated head-lane branch differs.
    if(num_classc_macros) {
      __syncthreads();
      shared_state[threadIdx.x] = sram_duplicate_t;
      __syncthreads();
      // One parallel pass: every head lane (found by the role scan above)
      // gathers its own packed input lanes, evaluates, and stores to its own
      // disjoint scratch words. Head lanes in the same warp execute this
      // together under SIMT instead of one per loop iteration.
      if(my_classc_lane >= 0) {
        u32 tag = my_classc_tag;
        int pw = gem_classc_perm_words(tag);
        int sw = gem_classc_state_words(tag);
        u32 wbuf[8];
        for(int l = 0; l < pw; ++l)
          wbuf[l] = shared_state[classc_perm_base + my_classc_lane + l];
        u32 res[4];
        if((tag >> 28) == GEM_CLASSC_SRL32_READ)
          gem_eval_srl32_read(classc_fb, wbuf, res);
        else gem_eval_carry4(wbuf, (int)(tag & 0x0fffffffu), res);
        for(int j = 0; j < sw; ++j)
          shared_writeouts[classc_scratch_base + my_classc_word + j] = res[j];
      }
    }

    __syncthreads();
    u32 writeout_inv = shared_writeouts[threadIdx.x];

    clken_perm = (clken_perm & ~t4_5.c2) ^ t4_5.c1;
    writeout_inv ^= t4_5.c3;

    if(threadIdx.x < num_ios) {
      u32 old_wo = input_state[io_offset + threadIdx.x];
      u32 wo = (old_wo & ~clken_perm) | (writeout_inv & clken_perm);
      output_state[io_offset + threadIdx.x] = wo;
    }

    if(is_last_part) break;
  }
  assert(script_size == script_pi);
}

__global__ void simulate_v1_noninteractive_simple_scan(
  usize num_blocks,
  usize num_major_stages,
  const usize *__restrict__ blocks_start,
  const u32 *__restrict__ blocks_data,
  u32 *__restrict__ sram_data,
  usize num_cycles,
  usize state_size,
  u32 *__restrict__ states_noninteractive
  )
{
  assert(num_blocks == gridDim.x);
  assert(256 == blockDim.x);
  __shared__ u32 shared_metadata[256];
  __shared__ u32 shared_writeouts[256];
  __shared__ u32 shared_state[256];
  __shared__ u32 script_starts[32], script_sizes[32];
  assert(num_major_stages <= 32);
  if(threadIdx.x < num_major_stages) {
    script_starts[threadIdx.x] = blocks_start[threadIdx.x * num_blocks + blockIdx.x];
    script_sizes[threadIdx.x] = blocks_start[threadIdx.x * num_blocks + blockIdx.x + 1] - script_starts[threadIdx.x];
  }
  __syncthreads();
  for(usize cycle_i = 0; cycle_i < num_cycles; ++cycle_i) {
    for(usize stage_i = 0; stage_i < num_major_stages; ++stage_i) {
      simulate_block_v1(
        blocks_data + script_starts[stage_i],
        script_sizes[stage_i],
        states_noninteractive + cycle_i * state_size,
        states_noninteractive + (cycle_i + 1) * state_size,
        sram_data,
        shared_metadata, shared_writeouts, shared_state
        );
      cooperative_groups::this_grid().sync();
    }
  }
}
