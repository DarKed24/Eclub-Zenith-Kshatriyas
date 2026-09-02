// SPDX-FileCopyrightText: Copyright (c) 2024 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//! Partition scheduler and flattener

use crate::aig::{AIG, EndpointGroup, DriverType};
use crate::aigpdk::AIGPDK_SRAM_ADDR_WIDTH;
use crate::pe::{Partition, BOOMERANG_NUM_STAGES};
use crate::staging::StagedAIG;
use indexmap::IndexMap;
use std::collections::BTreeMap;
use ulib::UVec;

pub const NUM_THREADS_V1: usize = 1 << (BOOMERANG_NUM_STAGES - 5);

/// A flattened script, for partition executor version 1.
/// See [FlattenedScriptV1::blocks_data] for the format details.
///
/// Generally, a script contains a number of major stages.
/// Each stage consists of the same number of blocks.
/// Each block contains a list of flattened partitions.
pub struct FlattenedScriptV1 {
    /// the number of blocks
    pub num_blocks: usize,
    /// the number of major stages
    pub num_major_stages: usize,
    /// the CSR start indices of stages and blocks.
    ///
    /// this length is num_blocks * num_major_stages + 1
    pub blocks_start: UVec<usize>,
    /// the partition instructions.
    ///
    /// the instructions follow a special format.
    /// it consists of zero or more partitions.
    /// 1. metadata section [1x256]:
    ///    the number of boomerang stages.
    ///      32-bit
    ///      if this is zero, the stage will not run. only happens
    ///      when the block has no partition mapped onto it.
    ///    is this the last boomerang stage?
    ///      32-bit, only 0 or 1
    ///    the number of valid write-outs.
    ///      32-bit
    ///    the write-out destination offset.
    ///      32-bit
    ///      (at this offset, we put all FFs and other outputs.)
    ///    the srams count and offsets.
    ///      32-bit count
    ///      32-bit memory offset for the first mem.
    ///    the number of global read rounds
    ///      32-bit count
    ///    the number of output-duplicate writeouts
    ///      32-bit count
    ///      this is used when one output pin is used by either
    ///      <both output and FFs>, or <multiple FFs with different
    ///      enabling conditions>.
    ///    the Class R word-level macro section [4x], slots [8..12]:
    ///      32-bit num_macros
    ///      32-bit wo_base_dup    - I/O write-out base of the duplicate section
    ///      32-bit wo_base_sram   - I/O write-out base of the SRAM read section
    ///      32-bit wo_base_macro  - I/O write-out base of the macro-state
    ///                              section (even-aligned; 2 words per macro)
    ///      emitted as ZERO when num_macros == 0 AND num_classc_macros == 0, in
    ///      which case the legacy [normal | dup | sram] layout arithmetic
    ///      (num_ios - num_srams - ...) still applies. the full I/O region is:
    ///        [0,                  N )  N = num_normal_writeouts (boomerang)
    ///        [wo_base_dup,        +D )  duplicated outputs
    ///        [wo_base_sram,       +S )  SRAM read data, 1 word each
    ///        [wo_base_macro,      +2M)  Class R macro P register, 64-bit pairs
    ///        [classc_scratch_base,+C )  Class C macro staged-IO scratch words
    ///      a Class R macro's 4 packed input lanes sit in the sram/duplicate
    ///      permute region at threads [num_srams*4 + m*4 .. +4).
    ///    the Class C (combinational) macro section, slots [12..16]:
    ///      32-bit num_classc_macros
    ///      32-bit classc_perm_base    - permute-lane base, after SRAM+Class R
    ///      32-bit classc_scratch_base - I/O write-out base of the scratch
    ///      32-bit classc_perm_words   - total lanes; the duplicate permute
    ///                                   region begins at
    ///                                   classc_perm_base + classc_perm_words
    ///      slots [16 + i] hold the TAGGED shape word of Class C macro i
    ///      (part.endpoints order):
    ///        kind << 28 | payload
    ///        kind 0 = Carry4     payload = chain_len
    ///        kind 1 = Srl32Read  payload = GLOBAL state word index of the
    ///                            paired Srl32Shift's committed 32-bit state,
    ///                            which the kernel loads zero-copy out of
    ///                            input_state instead of routing those 32 bits
    ///                            through the boomerang (28 bits, so the reg/io
    ///                            state may hold up to 2^28 words).
    ///      Tag 0 keeps every CARRY4 word numerically identical to Part B.
    ///      All emitted as ZERO when num_classc_macros == 0, so a Part-A /
    ///      macro-free partition's script bytes are unchanged. A Class C
    ///      macro's outputs are committed unconditionally (en_iv is tied
    ///      high) to the scratch region and read back by a later major stage.
    ///    the Class R kind table, slots [16 + num_classc_macros + j],
    ///      j in [0, num_macros):
    ///        0 = Dsp48e2, 1 = Srl32Shift
    ///      Emitted only when num_macros > 0. For an all-DSP partition every
    ///      word is 0 — exactly the padding zeros already emitted there — so
    ///      the bytes (and the pinned blocks_data hashes) do not move. The
    ///      kernel selects between the two Class R datapaths with a mask
    ///      rather than a branch, so both are still divergence-free.
    ///    padding towards 128
    ///    the location of early write-outs, excl. mem.
    ///      256 * [#boomstage & 0~256 id], compressed to 128
    /// 2. initial read-global permutation [2x]*rounds
    ///    32-bit indices*1 for each of the 256 threads.
    ///    32-bit valid mask for each of the 256 threads.
    ///    if the valid mask is zero, the memory is not read.
    ///    the result should be stored "like pext instruction",
    ///    but "reversed", and then appended to the low bits
    ///    in each round.
    ///    the index is encoded with a type bit at the highest bit:
    ///      if it is 0: it means it is offset from previous iteration.
    ///      if it is 1: it is offset from current iteration
    ///        which means it is an intermediate value coming from
    ///        the same cycle but a previous major stage.
    /// 3. boomerang sections, repeat below N*16
    ///    1. local shuffle permutation
    ///       32-bit indices * 16 for each of the 256 threads.
    ///    2. input (with inv) * 8192bits * (3+1padding)
    ///       32-bit * 256 threads * (3+1): xora, xorb, orb, [0 padding]
    ///       0xy: and gate, out = (a^x)&(b^y).
    ///       100: passthrough, out = a.
    ///       111: invalid, can be skipped.
    ///    -. write out, according to rest
    /// 4. global write-outs.
    ///    1. sram & additional endpoint copy permutations, inv. [16x].
    ///       only the inputs within sram and endpoint copy
    ///       range will be considered.
    ///       followed by [4x] invert, set0, and two 0 paddings.
    ///    2. permutation for the write-out enabler pins, inv. [16]
    ///       include itself inv and data inv.
    ///       followed by [3(+1padding)x]
    ///         clock invert, clock set0, data invert, and 0 padding
    ///    -. commit the write-out
    pub blocks_data: UVec<u32>,
    /// the state size including DFF and I/O states only.
    ///
    /// the inputs are always at front.
    pub reg_io_state_size: u32,
    /// the u32 array length for storing SRAMs.
    pub sram_storage_size: u32,
    /// expected input AIG pins layout
    pub input_layout: Vec<usize>,
    /// maps from primary outputs, FF:D and SRAM:PORT_R_RD_DATA AIG pins
    /// to state offset, index with invert.
    pub output_map: IndexMap<usize, u32>,
    /// maps from primary inputs, FF:Q/SRAM:* input AIG pins to state offset,
    /// index WITHOUT invert.
    pub input_map: IndexMap<usize, u32>,
    /// (for debug purpose) the relation between major stage, block and
    /// part indices as given in construction.
    pub stages_blocks_parts: Vec<Vec<Vec<usize>>>,
}

fn map_global_read_to_rounds(
    inputs_taken: &BTreeMap<u32, u32>
) -> Vec<Vec<(u32, u32)>> {
    let inputs_taken = inputs_taken.iter()
        .map(|(&a, &b)| (a, b)).collect::<Vec<_>>();
    // the larger the sorting chunk size, the better the successful chance,
    // but the less efficient due to worse cache coherency.
    let mut chunk_size = inputs_taken.len();
    while chunk_size >= 1 {
        let mut slices = inputs_taken.chunks(chunk_size).collect::<Vec<_>>();
        slices.sort_by_cached_key(|&slice| {
            u32::MAX - slice.iter()
                .map(|(_, mask)| mask.count_ones()).sum::<u32>()
        });
        let mut rounds_idx_masks: Vec<Vec<(u32, u32)>> = vec![vec![]; NUM_THREADS_V1];
        let mut round_map_j = 0;
        let mut fail = false;
        for slice in slices {
            for &(offset, mask) in slice {
                let wrap_fail_j = round_map_j;
                while rounds_idx_masks[round_map_j].iter().map(|(_, mask)| mask.count_ones()).sum::<u32>() + mask.count_ones() > 32 {
                    round_map_j += 1;
                    if round_map_j == NUM_THREADS_V1 {
                        round_map_j = 0;
                    }
                    if round_map_j == wrap_fail_j {
                        // panic!("failed to map at part {} mem offset {}", i, offset);
                        fail = true;
                        break
                    }
                }
                if fail { break }
                rounds_idx_masks[round_map_j].push((offset, mask));
                round_map_j += 1;
                if round_map_j == NUM_THREADS_V1 {
                    round_map_j = 0;
                }
            }
            if fail { break }
        }
        if !fail {
            // let max_rounds = rounds_idx_masks.iter().map(|v| v.len()).max().unwrap();
            // println!("max_rounds: {}, round_map_j: {}, inputs_taken len {}", max_rounds, round_map_j, inputs_taken.len());
            return rounds_idx_masks
        }
        chunk_size /= 2;
    }
    panic!("cannot map global init to any multiples of rounds.");
}

/// temporaries for a part being flattened. will be discarded after built.
#[derive(Debug, Clone, Default)]
struct FlatteningPart {
    /// for each boomerang stage, the result bits layout.
    afters: Vec<Vec<usize>>,
    /// for each partition, the output bits layout not containing sram outputs yet.
    parts_after_writeouts: Vec<usize>,
    /// mapping from aig pin index to writeout position (0~8192)
    after_writeout_pin2pos: IndexMap<usize, u16>,
    /// the number of SRAMs to simulate in this part.
    num_srams: u32,
    /// the number of Class R (registered) word-level macros in this part.
    num_macros: u32,
    /// the number of Class C (combinational) macros in this part.
    num_classc_macros: u32,
    /// total packed input lanes over all Class C macros in this part.
    classc_perm_words: u32,
    /// total staged-IO scratch words over all Class C macros in this part.
    classc_state_words: u32,
    /// permute-lane base of the Class C packed-input region
    /// (`num_srams*4 + num_macros*4`).
    classc_perm_base: u32,
    /// I/O write-out base of the Class C staged-IO scratch region
    /// (`wo_base_macro + 2*num_macros`).
    classc_scratch_base: u32,
    /// tagged shape word (`kind << 28 | payload`) of each Class C macro, in
    /// `part.endpoints` order — emitted into script metadata
    /// `[16 + i]` so the emulator / kernel can recompute per-macro lane and
    /// scratch-word offsets and pick the right datapath.
    classc_kinds: Vec<u32>,
    /// For each Class C macro (same order as `classc_kinds`), the AIG pin whose
    /// global state word supplies the macro's **zero-copy feedback load**, or
    /// `usize::MAX` when the kind takes none.
    ///
    /// Only `Srl32Read` uses it. It cannot be resolved in
    /// `init_afters_writeouts` because the paired `Srl32Shift` may live in a
    /// later partition — or a later major stage — and its `input_map` entry
    /// only exists once `make_inputs_outputs` has run for *every* part. So the
    /// pin is parked here and turned into a word index in `build_script`, which
    /// the second pass of [`build_flattened_script_v1`] runs after the whole
    /// map is populated.
    classc_feedback_pins: Vec<usize>,
    /// kind tag (`0` = Dsp48e2, `1` = Srl32Shift) of each Class R macro, in
    /// `part.endpoints` order — emitted into script metadata
    /// `[16 + num_classc_macros + j]`.
    classr_kinds: Vec<u32>,
    /// number of normal writeouts
    num_normal_writeouts: u32,
    /// number of writeout slots for output duplication
    num_duplicate_writeouts: u32,
    /// number of total writeouts
    num_writeouts: u32,
    /// I/O write-out region base (in u32 words, relative to `state_start`)
    /// of the duplicated-output section. Always == `num_normal_writeouts`.
    wo_base_dup: u32,
    /// I/O write-out region base of the SRAM read-data section.
    wo_base_sram: u32,
    /// I/O write-out region base of the macro-state section. Rounded up to
    /// an even word so the 64-bit `P` slot is 8-byte aligned. Equals
    /// `wo_base_sram + num_srams` when there are no macros.
    wo_base_macro: u32,
    /// the outputs categorized into activations
    comb_outputs_activations: IndexMap<usize, IndexMap<usize, Option<u16>>>,
    /// the current (placed) count of duplicate permutes
    cnt_placed_duplicate_permute: u32,

    /// the starting offset for FFs, outputs, and SRAM read results.
    state_start: u32,
    /// the starting offset of SRAM storage.
    sram_start: u32,

    /// the partial permutation instructions for
    /// 1. sram inputs
    /// 2. duplicated output pins due to difference in polarity/clock en.
    ///
    /// len: 8192
    sram_duplicate_permute: Vec<u16>,
    /// invert bit for sram_duplicate.
    ///
    /// len: 256
    sram_duplicate_inv: Vec<u32>,
    /// set-0 bit for sram_duplicate.
    ///
    /// len: 256
    sram_duplicate_set0: Vec<u32>,
    /// the permutation for clock enable pins.
    ///
    /// len: 8192
    clken_permute: Vec<u16>,
    /// invert bit for clken
    ///
    /// len: 256
    clken_inv: Vec<u32>,
    /// set-0 bit for clken
    ///
    /// len: 256
    clken_set0: Vec<u32>,
    /// invert bit for data corresponding to clken
    ///
    /// len: 256
    data_inv: Vec<u32>,
}

/// Class R kind tag emitted into script metadata
/// `[16 + num_classc_macros + j]`. `Dsp48e2` is 0, which is what the metadata
/// padding already held, so an all-DSP partition's bytes do not move.
fn classr_kind_tag(kind: crate::hwmacro::MacroKind) -> u32 {
    use crate::hwmacro::{MacroKind, MACRO_KIND_TAG_DSP48E2, MACRO_KIND_TAG_SRL32_SHIFT};
    match kind {
        MacroKind::Dsp48e2 => MACRO_KIND_TAG_DSP48E2,
        MacroKind::Srl32Shift => MACRO_KIND_TAG_SRL32_SHIFT,
        other => panic!("{other:?} is not a Class R macro kind"),
    }
}

/// Tagged Class C shape word emitted into script metadata `[16 + i]`:
/// `kind << 28 | payload`. `Carry4`'s tag is 0 and its payload is `chain_len`,
/// so every word a Part-B design emitted stays numerically identical.
///
/// `Srl32Read`'s payload is left zero here and OR'd in by `build_script`, which
/// is the first point at which `input_map` can resolve its feedback pin to a
/// global state word index.
fn classc_kind_word(kind: crate::hwmacro::MacroKind) -> u32 {
    use crate::hwmacro::{
        MacroKind, CLASSC_KIND_TAG_CARRY4, CLASSC_KIND_TAG_SRL32_READ,
    };
    match kind {
        MacroKind::Carry4 { chain_len } => {
            (CLASSC_KIND_TAG_CARRY4 << 28) | chain_len as u32
        }
        MacroKind::Srl32Read => CLASSC_KIND_TAG_SRL32_READ << 28,
        other => panic!("{other:?} is not a Class C macro kind"),
    }
}

fn set_bit_in_u32(v: &mut u32, pos: u32, bit: u8) {
    if bit != 0 {
        *v |= 1 << pos;
    }
    else {
        *v &= !(1 << pos);
    }
}

impl FlatteningPart {
    fn init_afters_writeouts(
        &mut self, aig: &AIG, staged: &StagedAIG, part: &Partition
    ) {
        let afters = part.stages.iter().map(|s| {
            let mut after = Vec::with_capacity(1 << BOOMERANG_NUM_STAGES);
            after.push(usize::MAX);
            for i in (1..=BOOMERANG_NUM_STAGES).rev() {
                after.extend(s.hier[i].iter().copied());
            }
            after
        }).collect::<Vec<_>>();
        let wos = part.stages.iter().zip(afters.iter()).map(|(s, after)| {
            s.write_outs.iter().map(|&woi| {
                after[woi * 32..(woi + 1) * 32].iter().copied()
            }).flatten()
        }).flatten().collect::<Vec<_>>();

        // println!("test wos: {:?}", wos);

        self.afters = afters;
        self.parts_after_writeouts = wos;
        self.num_normal_writeouts = part.stages.iter()
            .map(|s| s.write_outs.len()).sum::<usize>() as u32;
        self.num_srams = 0;
        self.num_macros = 0;
        self.num_classc_macros = 0;
        self.classc_perm_words = 0;
        self.classc_state_words = 0;
        self.classc_kinds = Vec::new();
        self.classc_feedback_pins = Vec::new();
        self.classr_kinds = Vec::new();

        // map: output aig pin id -> ((clken, data iv) -> pos)
        let mut comb_outputs_activations =
            IndexMap::<usize, IndexMap<usize, Option<u16>>>::new();
        for &endpt_i in &part.endpoints {
            match staged.get_endpoint_group(aig, endpt_i) {
                EndpointGroup::RAMBlock(_) => {
                    self.num_srams += 1;
                },
                EndpointGroup::Macro(m) => match m.kind.class() {
                    crate::hwmacro::MacroClass::Registered => {
                        self.num_macros += 1;
                        self.classr_kinds.push(classr_kind_tag(m.kind));
                    }
                    crate::hwmacro::MacroClass::Combinational => {
                        self.num_classc_macros += 1;
                        self.classc_perm_words += m.kind.num_perm_words() as u32;
                        self.classc_state_words += m.kind.num_state_words() as u32;
                        self.classc_kinds.push(classc_kind_word(m.kind));
                        self.classc_feedback_pins.push(m.feedback_pin);
                    }
                },
                EndpointGroup::PrimaryOutput(idx) => {
                    comb_outputs_activations.entry(idx >> 1)
                        .or_default().insert(2 | (idx & 1), None);
                },
                EndpointGroup::StagedIOPin(idx) => {
                    comb_outputs_activations.entry(idx)
                        .or_default().insert(2, None);
                },
                EndpointGroup::DFF(dff) => {
                    comb_outputs_activations.entry(dff.d_iv >> 1)
                        .or_default().insert(
                            dff.en_iv << 1 | (dff.d_iv & 1),
                            None);
                },
            }
        }
        self.num_duplicate_writeouts = ((
            comb_outputs_activations.values()
                .map(|v| v.len() - 1).sum::<usize>()
                + 31) / 32) as u32;
        self.comb_outputs_activations = comb_outputs_activations;

        // I/O write-out region:  [ normal | dup | sram | macro(Class R) | classc scratch ]
        // Bases are stored explicitly (and emitted into the script metadata)
        // rather than derived by subtraction, so appending the macro sections
        // does not perturb the arithmetic for the earlier sections.
        self.wo_base_dup = self.num_normal_writeouts;
        self.wo_base_sram = self.wo_base_dup + self.num_duplicate_writeouts;
        let after_sram = self.wo_base_sram + self.num_srams;
        self.wo_base_macro = if self.num_macros > 0 {
            (after_sram + 1) & !1
        } else {
            after_sram
        };
        // Class C scratch words are bit-addressed staged-IO reads, no 64-bit
        // load, so no alignment hole.
        self.classc_scratch_base = self.wo_base_macro + 2 * self.num_macros;
        self.classc_perm_base = self.num_srams * 4 + self.num_macros * 4;
        self.num_writeouts = self.classc_scratch_base + self.classc_state_words;

        self.after_writeout_pin2pos = self.parts_after_writeouts.iter().enumerate()
            .filter_map(|(i, &pin)| {
                if pin == usize::MAX { None }
                else { Some((pin, i as u16)) }
            })
            .collect::<IndexMap<_, _>>();
    }

    /// returns permutation id, invert bit, and setzero bit
    fn query_permute_with_pin_iv(&self, pin_iv: usize) -> (u16, u8, u8) {
        if pin_iv <= 1 {
            return (0, pin_iv as u8, 1)
        }
        let pos = self.after_writeout_pin2pos.get(&(pin_iv >> 1)).unwrap();
        (*pos, (pin_iv & 1) as u8, 0)
    }

    /// places a sram_duplicate bit.
    fn place_sram_duplicate(&mut self, pos: usize, (perm, inv, set0): (u16, u8, u8)) {
        self.sram_duplicate_permute[pos] = perm;
        set_bit_in_u32(&mut self.sram_duplicate_inv[pos >> 5],
                       (pos & 31) as u32, inv);
        set_bit_in_u32(&mut self.sram_duplicate_set0[pos >> 5],
                       (pos & 31) as u32, set0);
    }

    /// places a writeout bit's clock enable and data invert.
    fn place_clken_datainv(
        &mut self, pos: usize,
        clken_iv_perm: u16, clken_iv_inv: u8, clken_iv_set0: u8, data_inv: u8
    ) {
        self.clken_permute[pos] = clken_iv_perm;
        set_bit_in_u32(&mut self.clken_inv[pos >> 5],
                       (pos & 31) as u32, clken_iv_inv);
        set_bit_in_u32(&mut self.clken_set0[pos >> 5],
                       (pos & 31) as u32, clken_iv_set0);
        set_bit_in_u32(&mut self.data_inv[pos >> 5],
                       (pos & 31) as u32, data_inv);
    }

    /// returns a final local position for a data output bit with given pin_iv and clken_iv.
    ///
    /// if is not already placed, we will place it as well as place
    /// the clock enable bit, duplication bit, and bitflags for clock and data.
    fn get_or_place_output_with_activation(&mut self, pin_iv: usize, clken_iv: usize) -> u16 {
        let (activ_idx, _, pos) = self.comb_outputs_activations
            .get(&(pin_iv >> 1)).unwrap()
            .get_full(&(clken_iv << 1 | (pin_iv & 1))).unwrap();
        if let Some(pos) = *pos {
            return pos
        }
        let (clken_iv_perm, clken_iv_inv, clken_iv_set0) = self.query_permute_with_pin_iv(clken_iv);
        let origpos = match self.after_writeout_pin2pos.get(&(pin_iv >> 1)) {
            Some(origpos) => *origpos,
            None => {
                panic!("position of pin_iv {} (clken_iv {}) not found.. buggy boomerang, check if netlist and gemparts mismatch.", pin_iv, clken_iv)
            }
        } as usize;
        let r_pos = if activ_idx == 0 {
            self.place_clken_datainv(
                origpos, clken_iv_perm, clken_iv_inv, clken_iv_set0, (pin_iv & 1) as u8
            );
            origpos as u16
        }
        else {
            self.cnt_placed_duplicate_permute += 1;
            let dup_pos = (self.wo_base_sram * 32 - self.cnt_placed_duplicate_permute) as usize;
            let dup_perm_pos = ((self.num_srams * 4 + self.num_macros * 4
                + self.classc_perm_words + self.num_duplicate_writeouts) * 32
                - self.cnt_placed_duplicate_permute) as usize;
            if dup_perm_pos >= 8192 {
                panic!("sram duplicate bit larger than expected..")
                // dup_perm_pos = 8191;
            }
            self.place_sram_duplicate(
                dup_perm_pos, (origpos as u16, 0, 0)
            );
            self.place_clken_datainv(
                dup_pos, clken_iv_perm, clken_iv_inv, clken_iv_set0, (pin_iv & 1) as u8
            );
            dup_pos as u16
        };
        *self.comb_outputs_activations.get_mut(&(pin_iv >> 1)).unwrap()
            .get_mut(&(clken_iv << 1 | (pin_iv & 1))).unwrap() = Some(r_pos);
        r_pos
    }

    fn make_inputs_outputs(
        &mut self,
        aig: &AIG,
        staged: &StagedAIG,
        part: &Partition,
        input_map: &mut IndexMap<usize, u32>,
        staged_io_map: &mut IndexMap<usize, u32>,
        output_map: &mut IndexMap<usize, u32>,
    ) {
        self.sram_duplicate_permute = vec![0; 1 << BOOMERANG_NUM_STAGES];
        self.sram_duplicate_inv = vec![0u32; NUM_THREADS_V1];
        self.sram_duplicate_set0 = vec![u32::MAX; NUM_THREADS_V1];
        self.clken_permute = vec![0; 1 << BOOMERANG_NUM_STAGES];
        self.clken_inv = vec![0u32; NUM_THREADS_V1];
        self.clken_set0 = vec![u32::MAX; NUM_THREADS_V1];
        self.data_inv = vec![0u32; NUM_THREADS_V1];
        self.cnt_placed_duplicate_permute = 0;

        let mut cur_sram_id = 0;
        let mut cur_macro_id = 0;
        let mut cur_classc_id: u32 = 0;
        let mut cur_classc_lane_off: usize = 0;
        let mut cur_classc_state_off: usize = 0;
        for &endpt_i in &part.endpoints {
            match staged.get_endpoint_group(aig, endpt_i) {
                EndpointGroup::RAMBlock(ram) => {
                    let sram_rd_data_local_offset = self.wo_base_sram as usize + cur_sram_id as usize;
                    let sram_rd_data_global_start = self.state_start + self.wo_base_sram + cur_sram_id;
                    let (perm_r_en_iv, perm_r_en_iv_inv, perm_r_en_iv_set0) = self.query_permute_with_pin_iv(ram.port_r_en_iv);
                    for k in 0..32 {
                        let d = ram.port_r_rd_data[k];
                        if d == usize::MAX { continue }
                        input_map.insert(d, sram_rd_data_global_start * 32 + k as u32);
                        // `or_insert` for the same reason the macro arm below
                        // uses it: when a `PORT_R_RD_DATA` pin is *also* a
                        // primary output, the PrimaryOutput arm's own write-out
                        // slot holds this cycle's value, while this slot holds
                        // next cycle's registered read data. The PO arm uses a
                        // plain `insert`, so it wins in either endpoint order.
                        output_map.entry(d << 1)
                            .or_insert(sram_rd_data_global_start * 32 + k as u32);
                        self.place_clken_datainv(
                            sram_rd_data_local_offset * 32 + k,
                            perm_r_en_iv, perm_r_en_iv_inv, perm_r_en_iv_set0, 0
                        );
                    }
                    let sram_input_perm_st = (cur_sram_id * 32 * 4) as usize;
                    for k in 0..13 {
                        self.place_sram_duplicate(
                            sram_input_perm_st + k,
                            self.query_permute_with_pin_iv(ram.port_r_addr_iv[k])
                        );
                        self.place_sram_duplicate(
                            sram_input_perm_st + 16 + k,
                            self.query_permute_with_pin_iv(ram.port_w_addr_iv[k])
                        );
                    }
                    for k in 0..32 {
                        self.place_sram_duplicate(
                            sram_input_perm_st + 32 + k,
                            self.query_permute_with_pin_iv(ram.port_w_wr_en_iv[k])
                        );
                        self.place_sram_duplicate(
                            sram_input_perm_st + 64 + k,
                            self.query_permute_with_pin_iv(ram.port_w_wr_data_iv[k])
                        );
                    }
                    cur_sram_id += 1;
                },
                EndpointGroup::Macro(m) if m.kind.class()
                    == crate::hwmacro::MacroClass::Registered =>
                {
                    let kind = m.kind;
                    let sw = kind.num_state_words() as u32;
                    // registered output words in the I/O write-out region,
                    // clock-enable gated by en_iv (the same construction the
                    // SRAM uses for port_r_en_iv).
                    let macro_state_local_word =
                        self.wo_base_macro as usize + cur_macro_id as usize * sw as usize;
                    let macro_state_global_word =
                        self.state_start + self.wo_base_macro + cur_macro_id * sw;
                    let (perm_en, perm_en_inv, perm_en_set0) =
                        self.query_permute_with_pin_iv(m.en_iv);
                    for k in 0..kind.num_output_bits() {
                        let d = m.outputs[k];
                        if d == usize::MAX { continue }
                        let bitpos = (macro_state_global_word + (k / 32) as u32) * 32
                            + (k % 32) as u32;
                        input_map.insert(d, bitpos);
                        // `or_insert`, not `insert`: when this macro output
                        // pin is ALSO a primary output (e.g. SRLC32E's
                        // `Q31 = state[31]` driving a port directly), the
                        // PrimaryOutput arm's own write-out slot is the
                        // correct answer — it holds this cycle's combinational
                        // value, i.e. the PRE-edge state, matching the DFF `Q`
                        // convention and the reference VCD. The macro state
                        // slot here holds the POST-edge value. The PO arm uses
                        // a plain `insert`, so the primary output wins in
                        // either endpoint order.
                        output_map.entry(d << 1).or_insert(bitpos);
                        self.place_clken_datainv(
                            (macro_state_local_word + k / 32) * 32 + (k % 32),
                            perm_en, perm_en_inv, perm_en_set0, 0
                        );
                    }
                    // packed input lanes, after the SRAM permute words.
                    let macro_perm_st = (self.num_srams as usize * 4
                        + cur_macro_id as usize * kind.num_perm_words()) * 32;
                    for i in 0..kind.num_input_bits() {
                        self.place_sram_duplicate(
                            macro_perm_st + kind.input_bit_slot(i),
                            self.query_permute_with_pin_iv(m.inputs_iv[i])
                        );
                    }
                    cur_macro_id += 1;
                },
                // Class C (combinational) macro: outputs are routed to the
                // staged-IO scratch region (visible this cycle in the next
                // major stage), committed unconditionally (en_iv is tie-high).
                EndpointGroup::Macro(m) => {
                    let kind = m.kind;
                    let scratch_local_word =
                        self.classc_scratch_base as usize + cur_classc_state_off;
                    let scratch_global_word =
                        self.state_start + self.classc_scratch_base
                        + cur_classc_state_off as u32;
                    let (perm_en, perm_en_inv, perm_en_set0) =
                        self.query_permute_with_pin_iv(m.en_iv);
                    for k in 0..kind.num_output_bits() {
                        let d = m.outputs[k];
                        if d == usize::MAX { continue }
                        let bitpos = (scratch_global_word + (k / 32) as u32) * 32
                            + (k % 32) as u32;
                        // staged IO only: consumed by a *later* major stage's
                        // global read via the current-iteration type bit.
                        staged_io_map.insert(d, bitpos);
                        self.place_clken_datainv(
                            (scratch_local_word + k / 32) * 32 + (k % 32),
                            perm_en, perm_en_inv, perm_en_set0, 0
                        );
                    }
                    let perm_st = (self.classc_perm_base as usize
                        + cur_classc_lane_off) * 32;
                    for i in 0..kind.num_input_bits() {
                        self.place_sram_duplicate(
                            perm_st + kind.input_bit_slot(i),
                            self.query_permute_with_pin_iv(m.inputs_iv[i])
                        );
                    }
                    cur_classc_lane_off += kind.num_perm_words();
                    cur_classc_state_off += kind.num_state_words();
                    cur_classc_id += 1;
                },
                EndpointGroup::PrimaryOutput(idx_iv) => {
                    if idx_iv == 0 {
                        panic!("primary output has zero..??")
                    }
                    let pos = self.state_start * 32 + self.get_or_place_output_with_activation(
                        idx_iv, 1
                    ) as u32;
                    output_map.insert(idx_iv, pos);
                },
                EndpointGroup::StagedIOPin(idx) => {
                    if idx == 0 {
                        panic!("staged IO pin has zero..??")
                    }
                    let pos = self.state_start * 32 + self.get_or_place_output_with_activation(
                        idx << 1, 1
                    ) as u32;
                    staged_io_map.insert(idx, pos);
                },
                EndpointGroup::DFF(dff) => {
                    if dff.d_iv == 0 {
                        clilog::warn!(DFF_CONST_ERR, "dff d_iv has zero, not fully optimized netlist. ignoring the error..");
                        input_map.insert(dff.q, 0);
                        continue
                    }
                    let pos = self.state_start * 32 + self.get_or_place_output_with_activation(
                        dff.d_iv, dff.en_iv
                    ) as u32;
                    output_map.insert(dff.d_iv, pos);
                    input_map.insert(dff.q, pos);
                },
            }
        }
        assert_eq!(cur_sram_id, self.num_srams);
        assert_eq!(cur_macro_id, self.num_macros);
        assert_eq!(cur_classc_id, self.num_classc_macros);
        assert_eq!(cur_classc_lane_off as u32, self.classc_perm_words);
        assert_eq!(cur_classc_state_off as u32, self.classc_state_words);
        assert_eq!((self.cnt_placed_duplicate_permute + 31) / 32, self.num_duplicate_writeouts);

        // println!("test clken_permute: {:?}, wos (w/o sram or dup): {:?}", self.clken_permute, self.parts_after_writeouts);
    }

    fn build_script(
        &self, aig: &AIG, part: &Partition,
        input_map: &IndexMap<usize, u32>,
        staged_io_map: &IndexMap<usize, u32>,
    ) -> Vec<u32> {
        let mut script = Vec::<u32>::new();

        // metadata
        script.push(part.stages.len() as u32);
        script.push(0);
        script.push(self.num_writeouts);
        script.push(self.state_start);
        script.push(self.num_srams);
        script.push(self.sram_start);
        script.push(0);   // [6]=num global read rounds, assigned later
        script.push(self.num_duplicate_writeouts);
        // [8..12]: word-level macro section. Emitted as zeros for a macro-free
        // partition (the kernel's legacy SRAM/duplicate arithmetic still
        // applies), so the script bytes — and their hash — are byte-identical
        // to before this change for every existing design. A Class-C-only
        // partition still needs the explicit bases (its num_ios now includes
        // the scratch region, so the legacy subtraction would be wrong).
        if self.num_macros > 0 || self.num_classc_macros > 0 {
            script.push(self.num_macros);       // [8]
            script.push(self.wo_base_dup);      // [9]
            script.push(self.wo_base_sram);     // [10]
            script.push(self.wo_base_macro);    // [11]
        }
        // [12..16]: Class C (combinational) macro section. Zero for a Part-A /
        // macro-free partition, so their script bytes are unchanged.
        //   [12] num_classc_macros
        //   [13] classc_perm_base   (permute-lane index, after SRAM + Class R)
        //   [14] classc_scratch_base (I/O write-out word, after Class R macros)
        //   [15] classc_perm_words  (total lanes; the dup region begins at
        //        classc_perm_base + classc_perm_words)
        // [16 + i]: tagged shape word (kind << 28 | payload) of Class C macro
        //   i (part.endpoints order), for the emulator / kernel to recompute
        //   per-macro lane/word offsets and dispatch the datapath.
        //   For `Carry4` the payload is `chain_len`; for `Srl32Read` it is the
        //   *global* state word index its zero-copy feedback load reads.
        assert!(
            16 + self.num_classc_macros as usize + self.num_macros as usize <= 128,
            "too many word-level macros in one partition for the metadata table"
        );
        if self.num_classc_macros > 0 {
            script.push(self.num_classc_macros);   // [12]
            script.push(self.classc_perm_base);    // [13]
            script.push(self.classc_scratch_base); // [14]
            script.push(self.classc_perm_words);   // [15]
            for (ci, &cw) in self.classc_kinds.iter().enumerate() {
                // Resolve the zero-copy feedback pin to a global state word.
                // `input_map` is complete by now: `build_flattened_script_v1`
                // runs `make_inputs_outputs` over every part of every major
                // stage before it calls any `build_script`, so a read half may
                // safely be flattened before its shift half.
                let fb = self.classc_feedback_pins[ci];
                let word = if fb == usize::MAX { cw } else {
                    let bitpos = *input_map.get(&fb).expect(
                        "Class C feedback pin has no input_map entry: its \
                         paired Class R macro was never realized"
                    );
                    // the 32 state bits of one Srl32Shift occupy exactly one
                    // 32-bit I/O word, so bit 0 must be word-aligned.
                    assert_eq!(
                        bitpos % 32, 0,
                        "Class C feedback state is not word-aligned"
                    );
                    let w = bitpos / 32;
                    assert!(
                        w < crate::hwmacro::CLASSC_PAYLOAD_LIMIT,
                        "global state word {w} does not fit the 28-bit Class C \
                         metadata payload"
                    );
                    cw | w
                };
                script.push(word);                 // [16 + i]
            }
        }
        // [16 + num_classc_macros + j]: Class R kind tag of macro j
        //   (0 = Dsp48e2, 1 = Srl32Shift). For an all-DSP partition these are
        //   all zero — byte-identical to the padding that used to sit here, so
        //   Part A / Part B script hashes are unchanged.
        if self.num_macros > 0 {
            while script.len() < 16 + self.num_classc_macros as usize {
                script.push(0);
            }
            for &kw in &self.classr_kinds {
                script.push(kw);
            }
        }
        // padding
        while script.len() < 128 {
            script.push(0);
        }
        // final 128: write-out locations
        // compressed 2-1
        let mut last_wo = u32::MAX;
        for (j, bs) in part.stages.iter().enumerate() {
            for &wo in &bs.write_outs {
                let cur_wo = (j as u32) << 8 | (wo as u32);
                if last_wo == u32::MAX {
                    last_wo = cur_wo;
                }
                else {
                    script.push(last_wo | (cur_wo << 16));
                    last_wo = u32::MAX;
                }
            }
        }
        if last_wo != u32::MAX {
            script.push(last_wo | (((1 << 16) - 1) << 16));
        }
        while script.len() < 256 {
            script.push(u32::MAX);
        }
        // read global (256x32)
        let mut inputs_taken = BTreeMap::<u32, u32>::new();
        for &inp in &part.stages[0].hier[0] {
            if inp == usize::MAX { continue }
            match input_map.get(&inp) {
                Some(&pos) => {
                    *inputs_taken.entry(pos >> 5).or_default() |=
                        1 << (pos & 31);
                }
                None => {
                    match staged_io_map.get(&inp) {
                        Some(&pos) => {
                            *inputs_taken.entry((pos >> 5) | (1u32 << 31))
                                .or_default() |= 1 << (pos & 31);
                        }
                        None => {
                            panic!("cannot find input pin {}, driver: {:?}, in either primary inputs or staged IOs", inp, aig.drivers[inp]);
                        }
                    }
                }
            }
        }
        // clilog::debug!(
        //     "part (?) inputs_taken len {}: {:?}",
        //     inputs_taken.len(),
        //     inputs_taken.iter().map(|(id, val)| format!("{}[{}]", id, val.count_ones())).collect::<Vec<_>>()
        // );
        let rounds_idx_masks = map_global_read_to_rounds(
            &inputs_taken
        );
        let num_global_stages = rounds_idx_masks.iter()
            .map(|v| v.len()).max().unwrap() as u32;
        script[6] = num_global_stages;
        assert_eq!(script.len(), NUM_THREADS_V1);
        let global_perm_start = script.len();
        script.extend((0..(2 * num_global_stages as usize * NUM_THREADS_V1)).map(|_| 0));
        for (i, v) in rounds_idx_masks.iter().enumerate() {
            for (round, &(idx, mask)) in v.iter().enumerate() {
                script[global_perm_start + NUM_THREADS_V1 * 2 * round + (i * 2)] = idx;
                script[global_perm_start + NUM_THREADS_V1 * 2 * round + (i * 2 + 1)] = mask;
                // println!("test: round {} i {} idx {} mask {}",
                //          round, i, idx, mask);
            }
        }

        let outputpos2localpos = rounds_idx_masks.iter().enumerate().map(|(local_i, v)| {
            let mut local_op2lp = Vec::with_capacity(32);
            let mut bit_id = 0;
            for &(idx, mask) in v.iter().rev() {
                let is_staged_io = (idx >> 31) != 0;
                for k in (0..32).rev() {
                    if (mask >> k & 1) != 0 {
                        local_op2lp.push(((is_staged_io, idx << 5 | k), (local_i * 32 + bit_id) as u16));
                        bit_id += 1;
                    }
                }
            }
            assert!(bit_id <= 32);
            local_op2lp.into_iter()
        }).flatten().collect::<IndexMap<_, _>>();
        // println!("output2localpos: {:?}", outputpos2localpos);

        let mut last_pin2localpos = IndexMap::new();
        for &inp in &part.stages[0].hier[0] {
            if inp == usize::MAX { continue }
            let pos = match input_map.get(&inp) {
                Some(&pos) => (false, pos),
                None => (true, *staged_io_map.get(&inp).unwrap())
            };
            last_pin2localpos.insert(inp, *outputpos2localpos.get(&pos).unwrap());
        }

        // boomerang sections start
        for (bs_i, bs) in part.stages.iter().enumerate() {
            let bs_perm = bs.hier[0].iter().map(|&pin| {
                if pin == usize::MAX { 0 }
                else { *last_pin2localpos.get(&pin).unwrap() }
            }).collect::<Vec<_>>();

            let mut bs_xora = vec![0u32; NUM_THREADS_V1];
            let mut bs_xorb = vec![0u32; NUM_THREADS_V1];
            let mut bs_orb = vec![0u32; NUM_THREADS_V1];
            for hi in 1..bs.hier.len() {
                let hi_len = bs.hier[hi].len();
                for j in 0..hi_len {
                    let out = bs.hier[hi][j];
                    let a = bs.hier[hi - 1][j];
                    let b = bs.hier[hi - 1][j + hi_len];
                    if out == usize::MAX {
                        continue
                    }
                    if out == a {
                        bs_orb[(hi_len + j) >> 5] |= 1 << ((hi_len + j) & 31);
                        continue
                    }
                    let (a_iv, b_iv) = match aig.drivers[out] {
                        DriverType::AndGate(a_iv, b_iv) => (a_iv, b_iv),
                        _ => unreachable!()
                    };
                    assert_eq!(a_iv >> 1, a);
                    assert_eq!(b_iv >> 1, b);
                    if (a_iv & 1) != 0 {
                        bs_xora[(hi_len + j) >> 5] |= 1 << ((hi_len + j) & 31);
                    }
                    if (b_iv & 1) != 0 {
                        bs_xorb[(hi_len + j) >> 5] |= 1 << ((hi_len + j) & 31);
                    }
                }
            }

            for k in 0..4 {
                for i in ((k * 8)..bs_perm.len()).step_by(32) {
                    script.push(((bs_perm[i] as u32)) |
                                (bs_perm[i + 1] as u32) << 16);
                    script.push(((bs_perm[i + 2] as u32)) |
                                (bs_perm[i + 3] as u32) << 16);
                    script.push(((bs_perm[i + 4] as u32)) |
                                (bs_perm[i + 5] as u32) << 16);
                    script.push(((bs_perm[i + 6] as u32)) |
                                (bs_perm[i + 7] as u32) << 16);
                }
            }
            for i in 0..NUM_THREADS_V1 {
                script.push(bs_xora[i]);
                script.push(bs_xorb[i]);
                script.push(bs_orb[i]);
                script.push(0);
            }

            last_pin2localpos = self.afters[bs_i].iter().enumerate().filter_map(|(i, &pin)| {
                if pin == usize::MAX { None }
                else { Some((pin, i as u16)) }
            }).collect::<IndexMap<_, _>>();
        }

        // sram worker
        for k in 0..4 {
            for i in ((k * 8)..self.sram_duplicate_permute.len()).step_by(32) {
                script.push(((self.sram_duplicate_permute[i] as u32)) |
                            (self.sram_duplicate_permute[i + 1] as u32) << 16);
                script.push(((self.sram_duplicate_permute[i + 2] as u32)) |
                            (self.sram_duplicate_permute[i + 3] as u32) << 16);
                script.push(((self.sram_duplicate_permute[i + 4] as u32)) |
                            (self.sram_duplicate_permute[i + 5] as u32) << 16);
                script.push(((self.sram_duplicate_permute[i + 6] as u32)) |
                            (self.sram_duplicate_permute[i + 7] as u32) << 16);
            }
        }
        for i in 0..NUM_THREADS_V1 {
            script.push(self.sram_duplicate_inv[i]);
            script.push(self.sram_duplicate_set0[i]);
            script.push(0);
            script.push(0);
        }
        // clock enable signal
        for k in 0..4 {
            for i in ((k * 8)..self.clken_permute.len()).step_by(32) {
                script.push(((self.clken_permute[i] as u32)) |
                            (self.clken_permute[i + 1] as u32) << 16);
                script.push(((self.clken_permute[i + 2] as u32)) |
                            (self.clken_permute[i + 3] as u32) << 16);
                script.push(((self.clken_permute[i + 4] as u32)) |
                            (self.clken_permute[i + 5] as u32) << 16);
                script.push(((self.clken_permute[i + 6] as u32)) |
                            (self.clken_permute[i + 7] as u32) << 16);
            }
        }
        for i in 0..NUM_THREADS_V1 {
            script.push(self.clken_inv[i]);
            script.push(self.clken_set0[i]);
            script.push(self.data_inv[i]);
            script.push(0);
        }

        script
    }
}

fn build_flattened_script_v1(
    aig: &AIG, stageds: &[&StagedAIG],
    parts_in_stages: &[&[Partition]],
    num_blocks: usize,
    input_layout: Vec<usize>
) -> FlattenedScriptV1 {
    // determine the output position.
    // this is the prerequisite for generating the read
    // permutations and more.
    // input map:
    // locate input pins and FF/SRAM Q's - for partition input
    // output map:
    // locate primary outputs - for circuit outs
    // staged io map:
    // store intermediate nodes between major stages
    let mut input_map = IndexMap::new();
    let mut output_map = IndexMap::new();
    let mut staged_io_map = IndexMap::new();
    for (i, &input) in input_layout.iter().enumerate() {
        if input == usize::MAX { continue }
        input_map.insert(input, i as u32);
    }

    let num_major_stages = parts_in_stages.len();

    let states_start = ((input_layout.len() + 31) / 32) as u32;
    let mut sum_state_start = states_start;
    let mut sum_srams_start = 0;
    // whether any partition holds a word-level macro. gates the even-alignment
    // of state offsets so a macro-free design's layout is untouched.
    let mut any_macros = false;

    // enumerate all major stages and build them one by one.

    // #[derive(Debug, Clone, Default)]
    // struct FlatteningStage {
    //     blocks_parts: Vec<Vec<usize>>,
    //     flattening_parts: Vec<FlatteningPart>,
    //     parts_data_split: Vec<Vec<u32>>,
    // }
    // let mut flattening_stages =
    //     Vec::<FlatteningStage>::with_capacity(num_major_stages);

    // assemble script per block.
    let mut blocks_data = Vec::new();
    let mut blocks_start = Vec::<usize>::with_capacity(num_blocks * num_major_stages + 1);
    let mut stages_blocks_parts = Vec::new();
    let mut stages_flattening_parts = Vec::new();

    for (i, (init_parts, &staged)) in parts_in_stages.into_iter().copied().zip(
        stageds.into_iter()
    ).enumerate() {
        // first arrange parts onto blocks.
        let mut blocks_parts = vec![vec![]; num_blocks];
        let mut tot_nstages_blocks = vec![0; num_blocks];
        // below models the fixed pre&post-cost for each executor
        let executor_fixed_cost = 3;
        // masonry layout of blocks. assume parts are sorted with
        // decreasing order of #stages.
        for i in 0..init_parts.len().min(num_blocks) {
            blocks_parts[i].push(i);
            tot_nstages_blocks[i] = init_parts[i].stages.len() + executor_fixed_cost;
        }
        for i in num_blocks..init_parts.len() {
            let put = tot_nstages_blocks.iter().enumerate()
                .min_by(|(_, a), (_, b)| a.cmp(b))
                .unwrap().0;
            blocks_parts[put].push(i);
            tot_nstages_blocks[put] += init_parts[i].stages.len() + executor_fixed_cost;
        }
        // clilog::debug!("blocks_parts: {:?}", blocks_parts);
        clilog::debug!("major stage {}: max total boomerang depth (w/ cost) {}",
                       i, tot_nstages_blocks.iter().copied().max().unwrap());

        // the intermediates for parts being flattened
        let mut flattening_parts: Vec<FlatteningPart> =
            vec![Default::default(); init_parts.len()];

        // basic index preprocessing for stages
        for i in 0..init_parts.len() {
            flattening_parts[i].init_afters_writeouts(
                aig, staged, &init_parts[i]);
        }

        // allocate output state positions for all srams,
        // in the order of block affinity.
        for block in &blocks_parts {
            for &part_id in block {
                // A partition holding Class-R macros needs an even state_start
                // so the 64-bit-aligned wo_base_macro lands on an 8-byte
                // boundary. num_writeouts is itself even for such a partition,
                // so alignment is preserved for those that follow.
                if flattening_parts[part_id].num_macros > 0 {
                    any_macros = true;
                    if sum_state_start % 2 != 0 {
                        sum_state_start += 1;
                    }
                }
                flattening_parts[part_id].state_start = sum_state_start;
                sum_state_start += flattening_parts[part_id].num_writeouts;
                flattening_parts[part_id].sram_start = sum_srams_start;
                sum_srams_start += flattening_parts[part_id].num_srams * (1 << AIGPDK_SRAM_ADDR_WIDTH);
            }
        }

        // besides input ports, we also have outputs from partitions.
        // they include original-placed comb output pins,
        // copied pins for different FF activation,
        // and SRAM read outputs.
        for part_id in 0..init_parts.len() {
            // clilog::debug!("initializing output for part {}", part_id);
            flattening_parts[part_id].make_inputs_outputs(
                aig, staged, &init_parts[part_id],
                &mut input_map, &mut staged_io_map, &mut output_map
            );
        }
        stages_blocks_parts.push(blocks_parts);
        stages_flattening_parts.push(flattening_parts);
    }

    for ((blocks_parts, flattening_parts), init_parts) in stages_blocks_parts.iter().zip(
        stages_flattening_parts.iter_mut()
    ).zip(
        parts_in_stages.into_iter().copied()
    ) {
        // build script per part. we will later assemble them to blocks.
        let mut parts_data_split = vec![vec![]; init_parts.len()];
        for part_id in 0..init_parts.len() {
            // clilog::debug!("building script for part {}", part_id);
            parts_data_split[part_id] = flattening_parts[part_id].build_script(
                aig, &init_parts[part_id], &input_map, &staged_io_map
            );
        }

        for block_id in 0..num_blocks {
            blocks_start.push(blocks_data.len());
            if blocks_parts[block_id].is_empty() {
                let mut dummy = vec![0; NUM_THREADS_V1];
                dummy[1] = 1;
                blocks_data.extend(dummy.into_iter());
            }
            else {
                let num_parts = blocks_parts[block_id].len();
                let mut last_part_st = usize::MAX;
                for (i, &part_id) in blocks_parts[block_id].iter().enumerate() {
                    if i == num_parts - 1 {
                        last_part_st = blocks_data.len();
                    }
                    blocks_data.extend(parts_data_split[part_id].iter().copied());
                }
                assert_ne!(last_part_st, usize::MAX);
                blocks_data[last_part_st + 1] = 1;
            }
        }
    }
    blocks_start.push(blocks_data.len());
    blocks_data.extend((0..NUM_THREADS_V1 * 8).map(|_| 0)); // padding

    // The per-cycle state stride must stay even so a macro's P slot keeps its
    // 8-byte alignment from one simulated cycle to the next.
    let reg_io_state_size = if any_macros {
        (sum_state_start + 1) & !1
    } else {
        sum_state_start
    };

    clilog::info!("Built script for {} blocks, reg/io state size {}, sram size {}, script size {}",
                  num_blocks, reg_io_state_size, sum_srams_start, blocks_data.len());

    FlattenedScriptV1 {
        num_blocks,
        num_major_stages,
        blocks_start: blocks_start.into(),
        blocks_data: blocks_data.into(),
        reg_io_state_size,
        sram_storage_size: sum_srams_start,
        input_layout,
        input_map,
        output_map,
        stages_blocks_parts,
    }
}

impl FlattenedScriptV1 {
    /// build a flattened script.
    ///
    /// `init_parts` give the partitions to flatten.
    /// it is better sorted in advance in descending order
    /// of #layers for better duty cycling.
    ///
    /// `num_blocks` should be set to the hardware allowances,
    /// i.e. the number of SMs in your GPU.
    /// for example, A100 should set it to 108.
    ///
    /// `input_layout` should give the expected primary input
    /// memory layout, each one is an AIG bit index.
    /// padding bits should be set to usize::MAX.
    pub fn from(
        aig: &AIG, stageds: &[&StagedAIG],
        parts_in_stages: &[&[Partition]],
        num_blocks: usize,
        input_layout: Vec<usize>
    ) -> FlattenedScriptV1 {
        build_flattened_script_v1(
            aig, stageds, parts_in_stages, num_blocks, input_layout)
    }
}
