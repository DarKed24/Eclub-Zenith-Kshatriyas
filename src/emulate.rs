// SPDX-FileCopyrightText: Copyright (c) 2024 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//! CPU emulator for the flattened partition-executor script (version 1).
//!
//! This is the CPU twin of `simulate_block_v1` in `csrc/kernel_v1_impl.cuh`.
//! It was lifted verbatim out of the (CUDA-gated) `cuda_test` binary so that
//! the script format can be exercised — and differentially tested against the
//! `naive_sim` behavioural model — on a machine with no CUDA toolchain.
//!
//! Keep it in lock-step with the CUDA kernel. The only additions over a pure
//! move are the word-level macro evaluation paths (Class R: DSP48E2 /
//! SRLC32E shift; Class C: CARRY4 / SRLC32E read), which mirror the
//! `gem_eval_*` epilogues in the kernel.

use crate::aigpdk::AIGPDK_SRAM_SIZE;
use crate::hwmacro::{
    eval_carry4, eval_dsp48e2, eval_srl32_read, eval_srl32_shift, MacroKind,
    CLASSC_KIND_TAG_SRL32_READ, MACRO_KIND_TAG_SRL32_SHIFT,
};

/// CPU prototype partition executor for script version 1.
pub fn simulate_block_v1(
    script: &[u32],
    input_state: &[u32], output_state: &mut [u32],
    sram_data: &mut [u32],
    debug_verbose: bool,
) {
    let mut script_pi = 0;
    loop {
        let num_stages = script[script_pi];
        let is_last_part = script[script_pi + 1];
        let num_ios = script[script_pi + 2];
        let io_offset = script[script_pi + 3];
        let num_srams = script[script_pi + 4];
        let sram_offset = script[script_pi + 5];
        let num_global_read_rounds = script[script_pi + 6];
        let num_output_duplicates = script[script_pi + 7];
        // [8..12]: word-level macro section (all zero for a macro-free
        // partition, which then keeps the legacy [normal|dup|sram] layout).
        // [12..16] + [16+i]: Class C (combinational) macro section.
        let num_macros = script[script_pi + 8];
        let num_classc_macros = script[script_pi + 12];
        let (wo_base_dup, wo_base_sram, wo_base_macro) =
            if num_macros > 0 || num_classc_macros > 0 {
                (script[script_pi + 9], script[script_pi + 10], script[script_pi + 11])
            } else {
                (num_ios - num_srams - num_output_duplicates,
                 num_ios - num_srams,
                 num_ios)
            };
        let (classc_perm_base, classc_scratch_base, classc_perm_words) = if num_classc_macros > 0 {
            (script[script_pi + 13], script[script_pi + 14], script[script_pi + 15])
        } else {
            (0, 0, 0)
        };
        // [16 + i]: tagged Class C shape word (kind << 28 | payload).
        let classc_kinds: Vec<u32> = (0..num_classc_macros as usize)
            .map(|i| script[script_pi + 16 + i])
            .collect();
        // [16 + num_classc_macros + j]: Class R kind tag (0 = Dsp48e2,
        // 1 = Srl32Shift). All-zero — i.e. metadata padding — for an all-DSP
        // partition, which is why Part A / Part B script bytes are unchanged.
        let classr_kinds: Vec<u32> = (0..num_macros as usize)
            .map(|j| script[script_pi + 16 + num_classc_macros as usize + j])
            .collect();
        let mut writeout_hooks = vec![0; 256];
        for i in 0..128 {
            let t = script[script_pi + 128 + i];
            writeout_hooks[i * 2] = (t & ((1 << 16) - 1)) as u16;
            writeout_hooks[i * 2 + 1] = (t >> 16) as u16;
        }
        if num_stages == 0 {
            script_pi += 256;
            break
        }
        // assert_eq!(part.stages.len(), num_stages as usize);
        // assert_eq!(part.stages.iter().map(|s| s.write_outs.len()).sum::<usize>(), (num_ios - num_srams - num_output_duplicates) as usize);
        script_pi += 256;
        let mut writeouts = vec![0u32; num_ios as usize];

        let mut state = vec![0u32; 256];
        for _gr_i in 0..num_global_read_rounds {
            for i in 0..256 {
                let mut cur_state = state[i];
                let idx = script[script_pi + (i * 2)];
                let mut mask = script[script_pi + (i * 2 + 1)];
                if mask == 0 { continue }
                let value = match (idx >> 31) != 0 {
                    false => input_state[idx as usize],
                    true => output_state[(idx ^ (1 << 31)) as usize]
                };
                while mask != 0 {
                    cur_state <<= 1;
                    // `mask & -mask` isolates the low set bit. Must be the
                    // wrapping negation: a mask of exactly `1 << 31` makes the
                    // signed negate overflow (and panic in a debug build).
                    // The kernel's `mask & -mask` on an unsigned `u32` already
                    // wraps, so this only ever differed under `debug_assert`.
                    let lowbit = mask & mask.wrapping_neg();
                    if (value & lowbit) != 0 {
                        cur_state |= 1;
                    }
                    mask ^= lowbit;
                }
                state[i] = cur_state;
            }
            script_pi += 256 * 2;
        }

        if debug_verbose {
            println!("debug_verbose STAGE 0");
            println!("global read states:");
            for i in 0..256 {
                println!(" [{}] = {}", i, state[i]);
            }
        }

        for bs_i in 0..num_stages {
            let mut hier_inputs = vec![0; 256];
            let mut hier_flag_xora = vec![0; 256];
            let mut hier_flag_xorb = vec![0; 256];
            let mut hier_flag_orb = vec![0; 256];
            for k_outer in 0..4 {
                for i in 0..256 {
                    for k_inner in 0..4 {
                        let k = k_outer * 4 + k_inner;
                        let t_shuffle = script[script_pi + i * 4 + k_inner];
                        let t_shuffle_1_idx = (t_shuffle & ((1 << 16) - 1)) as u16;
                        let t_shuffle_2_idx = (t_shuffle >> 16) as u16;
                        hier_inputs[i] |= (state[(t_shuffle_1_idx >> 5) as usize] >> (t_shuffle_1_idx & 31) & 1) << (k * 2);
                        hier_inputs[i] |= (state[(t_shuffle_2_idx >> 5) as usize] >> (t_shuffle_2_idx & 31) & 1) << (k * 2 + 1);
                    }
                }
                script_pi += 256 * 4;
            }
            for i in 0..256 {
                hier_flag_xora[i] = script[script_pi + i * 4];
                hier_flag_xorb[i] = script[script_pi + i * 4 + 1];
                hier_flag_orb[i] = script[script_pi + i * 4 + 2];
            }
            script_pi += 256 * 4;

            if debug_verbose {
                println!("debug_verbose STAGE 1.1 bs_i {bs_i}");
                println!("after local shuffle:");
                for i in 0..256 {
                    println!(" [{}] = {}", i, hier_inputs[i]);
                }
            }

            // hier[0]
            for i in 0..128 {
                let a = hier_inputs[i];
                let b = hier_inputs[128 + i];
                let xora = hier_flag_xora[128 + i];
                let xorb = hier_flag_xorb[128 + i];
                let orb = hier_flag_orb[128 + i];
                let ret = (a ^ xora) & ((b ^ xorb) | orb);
                hier_inputs[128 + i] = ret;
            }
            // hier 1 to 7
            for hi in 1..=7 {
                let hier_width = 1 << (7 - hi);
                for i in 0..hier_width {
                    let a = hier_inputs[hier_width * 2 + i];
                    let b = hier_inputs[hier_width * 3 + i];
                    let xora = hier_flag_xora[hier_width + i];
                    let xorb = hier_flag_xorb[hier_width + i];
                    let orb = hier_flag_orb[hier_width + i];
                    let ret = (a ^ xora) & ((b ^ xorb) | orb);
                    hier_inputs[hier_width + i] = ret;
                }
            }
            // hier 8,9,10,11,12
            let v1 = hier_inputs[1];
            let xora = hier_flag_xora[0];
            let xorb = hier_flag_xorb[0];
            let orb = hier_flag_orb[0];
            let r8 = ((v1 << 16) ^ xora) & ((v1 ^ xorb) | orb) & 0xffff0000;
            let r9 = ((r8 >> 8) ^ xora) & (((r8 >> 16) ^ xorb) | orb) & 0xff00;
            let r10 = ((r9 >> 4) ^ xora) & (((r9 >> 8) ^ xorb) | orb) & 0xf0;
            let r11 = ((r10 >> 2) ^ xora) & (((r10 >> 4) ^ xorb) | orb) & 0b1100;
            let r12 = ((r11 >> 1) ^ xora) & (((r11 >> 2) ^ xorb) | orb) & 0b10;
            hier_inputs[0] = r8 | r9 | r10 | r11 | r12;

            state = hier_inputs;

            if debug_verbose {
                println!("debug_verbose STAGE 1.2 bs_i {bs_i}");
                println!("after and-invert:");
                for i in 0..256 {
                    println!(" [{}] = {}", i, state[i]);
                }
            }

            for i in 0..256 {
                let hooki = writeout_hooks[i];
                if (hooki >> 8) as u32 == bs_i {
                    writeouts[i] = state[(hooki & 255) as usize];
                }
            }
        }

        let sram_dup_perm_len =
            (num_srams * 4 + num_macros * 4 + classc_perm_words + num_output_duplicates) as usize;
        let mut sram_duplicate_perm = vec![0u32; sram_dup_perm_len];
        for k_outer in 0..4 {
            for i in 0..sram_dup_perm_len {
                for k_inner in 0..4 {
                    let k = k_outer * 4 + k_inner;
                    let t_shuffle = script[script_pi + (i * 4 + k_inner) as usize];
                    let t_shuffle_1_idx = (t_shuffle & ((1 << 16) - 1)) as u32;
                    let t_shuffle_2_idx = (t_shuffle >> 16) as u32;
                    sram_duplicate_perm[i as usize] |= (writeouts[(t_shuffle_1_idx >> 5) as usize] >> (t_shuffle_1_idx & 31) & 1) << (k * 2);
                    sram_duplicate_perm[i as usize] |= (writeouts[(t_shuffle_2_idx >> 5) as usize] >> (t_shuffle_2_idx & 31) & 1) << (k * 2 + 1);
                }
            }
            script_pi += 256 * 4;
        }
        for i in 0..sram_dup_perm_len {
            sram_duplicate_perm[i] &= !script[script_pi + i * 4 + 1];
            sram_duplicate_perm[i] ^= script[script_pi + i * 4];
        }
        script_pi += 256 * 4;

        for sram_i_u32 in 0..num_srams {
            let sram_i = sram_i_u32 as usize;
            let addrs = sram_duplicate_perm[sram_i * 4];
            let port_r_addr_iv = addrs & 0xffff;
            let port_w_addr_iv = (addrs & 0xffff0000) >> 16;
            let port_w_wr_en = sram_duplicate_perm[sram_i * 4 + 1];
            let port_w_wr_data_iv = sram_duplicate_perm[sram_i * 4 + 2];

            let sram_st = sram_offset as usize + sram_i * AIGPDK_SRAM_SIZE;
            let sram_ed = sram_st + AIGPDK_SRAM_SIZE;
            let ram = &mut sram_data[sram_st..sram_ed];
            let r = ram[port_r_addr_iv as usize];
            let w0 = ram[port_w_addr_iv as usize];
            writeouts[(wo_base_sram + sram_i_u32) as usize] = r;
            ram[port_w_addr_iv as usize] = (w0 & !port_w_wr_en) | (port_w_wr_data_iv & port_w_wr_en);
        }

        for i in 0..num_output_duplicates {
            writeouts[(wo_base_dup + i) as usize] = sram_duplicate_perm
                [(num_srams * 4 + num_macros * 4 + classc_perm_words + i) as usize];
        }

        if debug_verbose {
            println!("debug_verbose STAGE 2");
            println!("before writeout_inv:");
            for i in 0..256 {
                println!(" [{}] = {}", i, if i < num_ios as usize {
                    writeouts[i]
                } else {
                    0
                });
            }
        }

        let mut clken_perm = vec![0u32; num_ios as usize];
        let writeouts_for_clken = writeouts.clone();
        for k_outer in 0..4 {
            for i in 0..num_ios {
                for k_inner in 0..4 {
                    let k = k_outer * 4 + k_inner;
                    let t_shuffle = script[script_pi + (i * 4 + k_inner) as usize];
                    let t_shuffle_1_idx = (t_shuffle & ((1 << 16) - 1)) as u32;
                    let t_shuffle_2_idx = (t_shuffle >> 16) as u32;
                    clken_perm[i as usize] |= (writeouts_for_clken[(t_shuffle_1_idx >> 5) as usize] >> (t_shuffle_1_idx & 31) & 1) << (k * 2);
                    clken_perm[i as usize] |= (writeouts_for_clken[(t_shuffle_2_idx >> 5) as usize] >> (t_shuffle_2_idx & 31) & 1) << (k * 2 + 1);
                }
            }
            script_pi += 256 * 4;
        }
        for i in 0..num_ios as usize {
            clken_perm[i] &= !script[script_pi + i * 4 + 1];
            clken_perm[i] ^= script[script_pi + i * 4];
            writeouts[i] ^= script[script_pi + i * 4 + 2];
        }
        script_pi += 256 * 4;

        // Class R word-level macro commit. Mirrors the kernel's macro-commit
        // block: the four packed input lanes are threads num_srams*4 + m*4 +
        // {0..4} of the sram/duplicate permute; the state feedback comes from
        // the previous cycle's I/O state (the DSP's 48-bit `P`, or the SRL's
        // 32-bit shift state in the low word). Written into the macro section
        // of `writeouts` after the clken/data-inv pass (data_inv is 0 for
        // state bits) and just before the final gated commit below.
        //
        // Two Class R kinds now share this slot. Both datapaths are evaluated
        // and selected with a mask rather than branched on, mirroring the
        // kernel: different macros in one warp may differ in kind, so every
        // lane must run an identical instruction stream.
        for m_i in 0..num_macros as usize {
            let base = num_srams as usize * 4 + m_i * 4;
            let w = [
                sram_duplicate_perm[base],
                sram_duplicate_perm[base + 1],
                sram_duplicate_perm[base + 2],
                sram_duplicate_perm[base + 3],
            ];
            let p_off = (io_offset + wo_base_macro) as usize + m_i * 2;
            let p_cur = (input_state[p_off] as u64) | ((input_state[p_off + 1] as u64) << 32);
            let pn_dsp = eval_dsp48e2(w, p_cur);
            let pn_srl = eval_srl32_shift(w, p_cur);
            let sel = 0u64.wrapping_sub(
                (classr_kinds[m_i] == MACRO_KIND_TAG_SRL32_SHIFT) as u64);
            let p_next = (pn_dsp & !sel) | (pn_srl & sel);
            writeouts[wo_base_macro as usize + m_i * 2] = p_next as u32;
            writeouts[wo_base_macro as usize + m_i * 2 + 1] = (p_next >> 32) as u32;
        }

        // Class C (combinational) macro worker. Mirrors the kernel's Class C
        // block: gather the packed input lanes out of `sram_duplicate_perm`,
        // evaluate the branchless datapath, and store the packed result into
        // the staged-IO scratch section of `writeouts`. The unconditional
        // clken (en_iv == 1) makes the commit below write it every cycle; the
        // next major stage reads it back through the current-iteration
        // global-read path.
        //
        // CARRY4 is purely combinational. The SRLC32E read port additionally
        // takes a zero-copy state feedback load: its metadata payload is the
        // GLOBAL word index of the paired shift half's committed state, so all
        // 32 state bits come from `input_state` in one read rather than riding
        // the boomerang as 32 source pins. Reading `input_state` (never
        // `output_state`) is what makes `Q` see the pre-edge snapshot, and it
        // makes the result independent of which major stage each half lands in.
        {
            let mut lane_off = 0usize;
            let mut word_off = 0usize;
            for ci in 0..num_classc_macros as usize {
                let tag = classc_kinds[ci] >> 28;
                let payload = (classc_kinds[ci] & 0x0fff_ffff) as usize;
                let kind = if tag == CLASSC_KIND_TAG_SRL32_READ {
                    MacroKind::Srl32Read
                } else {
                    MacroKind::Carry4 { chain_len: payload as u16 }
                };
                let lanes = kind.num_perm_words();
                let sw = kind.num_state_words();
                let base = classc_perm_base as usize + lane_off;
                let w: Vec<u32> = (0..lanes)
                    .map(|l| sram_duplicate_perm[base + l])
                    .collect();
                if tag == CLASSC_KIND_TAG_SRL32_READ {
                    writeouts[classc_scratch_base as usize + word_off] =
                        eval_srl32_read(input_state[payload], &w);
                } else {
                    let res = eval_carry4(&w, payload);
                    for j in 0..sw {
                        writeouts[classc_scratch_base as usize + word_off + j] =
                            (res >> (32 * j)) as u32;
                    }
                }
                lane_off += lanes;
                word_off += sw;
            }
        }

        for i in 0..num_ios {
            let old_wo = input_state[(io_offset + i) as usize];
            let clken = clken_perm[i as usize];
            let wo = (old_wo & !clken) | (writeouts[i as usize] & clken);
            output_state[(io_offset + i) as usize] = wo;
        }

        if debug_verbose {
            println!("debug_verbose STAGE 3");
            println!("final writeout:");
            for i in 0..num_ios {
                println!(" [{}] [global {}] = {}", i, io_offset + i, output_state[(io_offset + i) as usize]);
            }
        }

        if is_last_part != 0 {
            break
        }
    }
    assert_eq!(script_pi, script.len());
}
