// SPDX-FileCopyrightText: Copyright (c) 2024 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//! End-to-end differential test for the word-level macro substrate
//! (Part A: DSP48E2, Part B: CARRY4 + the Class C boomerang scheduler
//! extension, Part C: SRLC32E — Class R *and* Class C at once).
//!
//! For a small hand-written gate-level netlist (`tests/data/*.gv`) this:
//!   1. runs the full compile pipeline
//!      `AIG::from_netlistdb` -> `build_staged_aigs` -> `Partition::build_one`
//!      -> `FlattenedScriptV1::from`  (one partition per major stage, one
//!      block), asserting the expected number of major stages,
//!   2. drives random input vectors through `emulate::simulate_block_v1`
//!      (major stage by major stage, the same scan order the CUDA kernel
//!      uses) and through an independent behavioural gate-level evaluator, and
//!   3. asserts bit-exact agreement on every primary output, every DSP `P`
//!      bit and every SRL state bit, every cycle.
//!
//! `mac_accumulator.gv` exercises the Class R path; `ripple_adder.gv`,
//! `carry_then_logic_then_carry.gv` and `mixed_macros.gv` exercise the Class C
//! path; the `srl_*.gv` cases exercise Part C (static / dynamic address, the
//! zero-split `Q31` cascade, and `CE` gating); `heterogeneous.gv` runs all
//! three primitives in one design; `simple_dff.gv` has no macro and pins the
//! macro-free script ABI.
//!
//! Build/run with **no** `cuda` feature: `cargo run --bin macro_test`.

use std::collections::HashMap;
use std::path::Path;

use gem::aig::{DriverType, AIG};
use gem::aigpdk::AIGPDKLeafPins;
use gem::emulate::simulate_block_v1;
use gem::flatten::FlattenedScriptV1;
use gem::hwmacro::{
    eval_dsp48e2, MacroKind, DSP_A_OFFSET, DSP_B_OFFSET, DSP_C_OFFSET, DSP_D_OFFSET,
    DSP_OPMODE_S_OFFSET, DSP_RSTP_INDEX, DSP_USE_D_INDEX,
};
use gem::pe::Partition;
use gem::staging::build_staged_aigs;
use netlistdb::{Direction, NetlistDB};
use rand::prelude::*;
use rand_chacha::ChaCha20Rng;

fn eval_gate(celltype: &str, a: u8, b: u8) -> u8 {
    match celltype {
        "AND2_00_0" => a & b,
        "AND2_01_0" => a & (b ^ 1),
        "AND2_10_0" => (a ^ 1) & b,
        "AND2_11_0" => (a | b) ^ 1,
        "AND2_11_1" => a | b,
        "INV" => a ^ 1,
        "BUF" => a,
        other => panic!("unexpected combinational cell {other}"),
    }
}

/// One CARRY4 slice, the spec recurrence: `C[0] = CI | CYINIT`,
/// `C[i+1] = (S[i] & C[i]) | (~S[i] & DI[i])`, `O[i] = S[i] ^ C[i]`,
/// `CO[i] = C[i+1]`.
fn eval_carry4_slice(s: [u8; 4], di: [u8; 4], ci: u8, cyinit: u8) -> ([u8; 4], [u8; 4]) {
    let mut c = (ci | cyinit) & 1;
    let mut o = [0u8; 4];
    let mut co = [0u8; 4];
    for i in 0..4 {
        o[i] = s[i] ^ c;
        let cn = (s[i] & c) | ((s[i] ^ 1) & di[i]);
        co[i] = cn;
        c = cn;
    }
    (o, co)
}

/// canonical DSP48E2 input index for a given pin name + bit, or `None` for
/// pins that are not packed inputs (CLK, CEP, P).
fn dsp_canon(name: &str, bit: Option<isize>) -> Option<usize> {
    Some(match name {
        "A" => DSP_A_OFFSET + bit.unwrap() as usize,
        "D" => DSP_D_OFFSET + bit.unwrap() as usize,
        "B" => DSP_B_OFFSET + bit.unwrap() as usize,
        "C" => DSP_C_OFFSET + bit.unwrap() as usize,
        "OPMODE_S" => DSP_OPMODE_S_OFFSET + bit.unwrap() as usize,
        "USE_D" => DSP_USE_D_INDEX,
        "RSTP" => DSP_RSTP_INDEX,
        _ => return None,
    })
}

/// Independent behavioural gate-level evaluator, mirroring `naive_sim`'s
/// {settle, record, latch} model. State is carried between cycles; each call
/// applies one rising clock edge. CARRY4 is combinational and is settled in
/// the fixpoint alongside the AIG gates (fusion-independent).
struct Behavioural<'d> {
    db: &'d NetlistDB,
    dff: Vec<(usize, usize)>,      // (d pin, q pin)
    dsp: Vec<usize>,              // cell ids
    srl: Vec<usize>,              // cell ids
    q_state: HashMap<usize, u8>,  // q pin -> value
    p_state: HashMap<usize, u64>, // dsp cell -> P register
    srl_state: HashMap<usize, u32>, // srl cell -> 32-bit shift state
    netval: Vec<u8>,
}

impl<'d> Behavioural<'d> {
    fn new(db: &'d NetlistDB) -> Self {
        let mut dff = Vec::new();
        let mut dsp = Vec::new();
        let mut srl = Vec::new();
        for cellid in 1..db.num_cells {
            match db.celltypes[cellid].as_str() {
                "DFF" | "DFFSR" => {
                    let mut d = usize::MAX;
                    let mut q = usize::MAX;
                    for p in db.cell2pin.iter_set(cellid) {
                        match db.pinnames[p].1.as_str() {
                            "D" => d = p,
                            "Q" => q = p,
                            _ => {}
                        }
                    }
                    dff.push((d, q));
                }
                "GEM_DSP48E2" => dsp.push(cellid),
                "SRLC32E" => srl.push(cellid),
                _ => {}
            }
        }
        let mut netval = vec![0u8; db.num_nets];
        if let Some(n1) = db.net_one {
            netval[n1] = 1;
        }
        Behavioural {
            db,
            dff,
            dsp,
            srl,
            q_state: HashMap::new(),
            p_state: HashMap::new(),
            srl_state: HashMap::new(),
            netval,
        }
    }

    /// Fixpoint-evaluate every combinational net given the current registered
    /// state and the driven input values.
    fn settle(&mut self, inputs: &HashMap<usize, u8>) {
        let db = self.db;
        loop {
            let mut changed = false;
            for netid in 0..db.num_nets {
                if Some(netid) == db.net_zero || Some(netid) == db.net_one {
                    continue;
                }
                if db.net2pin.len(netid) == 0 {
                    continue;
                }
                let root = db.net2pin.items[db.net2pin.start[netid]];
                if db.pindirect[root] != Direction::O {
                    // an undriven net: leave it at its default value
                    continue;
                }
                let rcell = db.pin2cell[root];
                let newv = if rcell == 0 {
                    inputs.get(&root).copied().unwrap_or(0)
                } else {
                    match db.celltypes[rcell].as_str() {
                        "DFF" | "DFFSR" => self.q_state.get(&root).copied().unwrap_or(0),
                        "GEM_DSP48E2" => {
                            let bit = db.pinnames[root].2.unwrap() as usize;
                            ((self.p_state.get(&rcell).copied().unwrap_or(0) >> bit) & 1) as u8
                        }
                        "CARRY4" => {
                            let bit = db.pinnames[root].2.unwrap() as usize;
                            let name = db.pinnames[root].1.as_str();
                            let (mut s, mut di) = ([0u8; 4], [0u8; 4]);
                            let (mut ci, mut cyinit) = (0u8, 0u8);
                            for p in db.cell2pin.iter_set(rcell) {
                                let v = self.netval[db.pin2net[p]];
                                match (db.pinnames[p].1.as_str(), db.pinnames[p].2) {
                                    ("S", Some(i)) => s[i as usize] = v,
                                    ("DI", Some(i)) => di[i as usize] = v,
                                    ("CI", _) => ci = v,
                                    ("CYINIT", _) => cyinit = v,
                                    _ => {}
                                }
                            }
                            let (o, co) = eval_carry4_slice(s, di, ci, cyinit);
                            match name {
                                "O" => o[bit],
                                "CO" => co[bit],
                                _ => 0,
                            }
                        }
                        "SRLC32E" => {
                            // Both read ports see the PRE-edge state — the
                            // same snapshot GEM's cycle N reads out of
                            // `input_state`. Q is combinational on A and so
                            // is settled inside this fixpoint; Q31 is a plain
                            // state bit.
                            let st = self.srl_state.get(&rcell).copied().unwrap_or(0);
                            match db.pinnames[root].1.as_str() {
                                "Q31" => ((st >> 31) & 1) as u8,
                                "Q" => {
                                    let mut addr = 0u32;
                                    for p in db.cell2pin.iter_set(rcell) {
                                        if db.pinnames[p].1.as_str() == "A" {
                                            let i = db.pinnames[p].2.unwrap() as u32;
                                            addr |= (self.netval[db.pin2net[p]] as u32) << i;
                                        }
                                    }
                                    ((st >> (addr & 31)) & 1) as u8
                                }
                                _ => 0,
                            }
                        }
                        ct => {
                            let (mut a, mut b) = (0u8, 0u8);
                            for p in db.cell2pin.iter_set(rcell) {
                                match db.pinnames[p].1.as_str() {
                                    "A" => a = self.netval[db.pin2net[p]],
                                    "B" => b = self.netval[db.pin2net[p]],
                                    _ => {}
                                }
                            }
                            eval_gate(ct, a, b)
                        }
                    }
                };
                if self.netval[netid] != newv {
                    self.netval[netid] = newv;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Latch DFFs, DSP `P` registers and SRL shift states from the
    /// freshly-settled nets.
    fn latch(&mut self) {
        let db = self.db;
        let mut q_next = self.q_state.clone();
        for &(d, q) in &self.dff {
            q_next.insert(q, self.netval[db.pin2net[d]]);
        }
        let mut p_next = self.p_state.clone();
        for &cellid in &self.dsp {
            let mut w = [0u32; 4];
            let (mut cep, mut rstp) = (0u8, 0u8);
            for p in db.cell2pin.iter_set(cellid) {
                let name = db.pinnames[p].1.as_str();
                let v = self.netval[db.pin2net[p]];
                if name == "CEP" {
                    cep = v;
                    continue;
                }
                let Some(canon) = dsp_canon(name, db.pinnames[p].2) else {
                    continue;
                };
                if name == "RSTP" && v != 0 {
                    rstp = 1;
                }
                if v != 0 {
                    let slot = MacroKind::Dsp48e2.input_bit_slot(canon);
                    w[slot / 32] |= 1 << (slot % 32);
                }
            }
            if cep != 0 || rstp != 0 {
                let p_cur = self.p_state.get(&cellid).copied().unwrap_or(0);
                p_next.insert(cellid, eval_dsp48e2(w, p_cur));
            }
        }
        let mut srl_next = self.srl_state.clone();
        for &cellid in &self.srl {
            let (mut d, mut ce) = (0u8, 0u8);
            for p in db.cell2pin.iter_set(cellid) {
                match db.pinnames[p].1.as_str() {
                    "D" => d = self.netval[db.pin2net[p]],
                    "CE" => ce = self.netval[db.pin2net[p]],
                    _ => {}
                }
            }
            // on the rising edge, if CE == 1 the state shifts left (LSB->MSB)
            // and D loads into index 0. CE == 0 holds.
            if ce != 0 {
                let st = self.srl_state.get(&cellid).copied().unwrap_or(0);
                srl_next.insert(cellid, (st << 1) | (d as u32));
            }
        }
        self.q_state = q_next;
        self.p_state = p_next;
        self.srl_state = srl_next;
    }
}

struct Harness {
    netlistdb: NetlistDB,
    aig: AIG,
    script: FlattenedScriptV1,
    num_stages: usize,
    n_endpoints: usize,
    /// design input port pins
    input_ports: Vec<usize>,
    clock_port: usize,
    /// primary output port pins that are not tied constants
    output_ports: Vec<usize>,
    /// (dsp cell id, P output aigpins in bit order) — Class R macros only
    dsps: Vec<(usize, Vec<usize>)>,
    /// (srl cell id, state[0..32] output aigpins in bit order) — the Class R
    /// half of each SRLC32E. Keyed on the real netlist cell id, so it lines up
    /// with the behavioural model's `srl_state`.
    srls: Vec<(usize, Vec<usize>)>,
}

impl Harness {
    fn build(path: &Path, top: &str, expect_stages: usize) -> Harness {
        let netlistdb = NetlistDB::from_sverilog_file(path, Some(top), &AIGPDKLeafPins())
            .expect("cannot build netlist");
        let aig = AIG::from_netlistdb(&netlistdb);

        let stageds = build_staged_aigs(&aig, &[]);
        assert_eq!(
            stageds.len(),
            expect_stages,
            "[{top}] expected {expect_stages} major stage(s), got {}",
            stageds.len()
        );

        let mut input_layout = Vec::new();
        for (i, driv) in aig.drivers.iter().enumerate() {
            if let DriverType::InputPort(_) | DriverType::InputClockFlag(_, _) = driv {
                input_layout.push(i);
            }
        }

        // one partition per major stage: the whole staged sub-design.
        let staged_refs: Vec<_> = stageds.iter().map(|(_, _, s)| s).collect();
        let mut parts_per_stage: Vec<Vec<Partition>> = Vec::new();
        for staged in &staged_refs {
            let all: Vec<usize> = (0..staged.num_endpoint_groups()).collect();
            let part = Partition::build_one(&aig, staged, &all)
                .unwrap_or_else(|| panic!("[{top}] a staged sub-design does not fit one partition"));
            parts_per_stage.push(vec![part]);
        }
        let n_endpoints = staged_refs.last().unwrap().num_endpoint_groups();

        let parts_slices: Vec<&[Partition]> =
            parts_per_stage.iter().map(|p| p.as_slice()).collect();
        let script = FlattenedScriptV1::from(
            &aig,
            &staged_refs,
            &parts_slices,
            1,
            input_layout,
        );

        // classify top-level port pins
        let mut clock_ports = Vec::new();
        for cellid in 1..netlistdb.num_cells {
            if !matches!(
                netlistdb.celltypes[cellid].as_str(),
                "DFF" | "DFFSR" | "$__RAMGEM_SYNC_" | "GEM_DSP48E2" | "SRLC32E"
            ) {
                continue;
            }
            for p in netlistdb.cell2pin.iter_set(cellid) {
                if !matches!(
                    netlistdb.pinnames[p].1.as_str(),
                    "CLK" | "PORT_R_CLK" | "PORT_W_CLK"
                ) {
                    continue;
                }
                let netid = netlistdb.pin2net[p];
                let root = netlistdb.net2pin.items[netlistdb.net2pin.start[netid]];
                assert_eq!(netlistdb.pin2cell[root], 0, "clock must be a top-level port");
                clock_ports.push(root);
            }
        }
        clock_ports.sort_unstable();
        clock_ports.dedup();
        assert_eq!(clock_ports.len(), 1, "test netlists have exactly one clock");
        let clock_port = clock_ports[0];

        let mut input_ports = Vec::new();
        let mut output_ports = Vec::new();
        for p in netlistdb.cell2pin.iter_set(0) {
            match netlistdb.pindirect[p] {
                // outputs of the top macro == design inputs
                Direction::O if p != clock_port => input_ports.push(p),
                Direction::I => {
                    let aigpin_iv = aig.pin2aigpin_iv[p];
                    if aigpin_iv > 1 {
                        output_ports.push(p);
                    }
                }
                _ => {}
            }
        }

        let mut dsps = Vec::new();
        let mut srls = Vec::new();
        for (&cellid, mb) in &aig.macros {
            match mb.kind {
                MacroKind::Dsp48e2 => dsps.push((cellid, mb.outputs.clone())),
                // the shift half keys on the real netlist cell id; the read
                // half keys on a synthetic id above netlistdb.num_cells.
                MacroKind::Srl32Shift => srls.push((cellid, mb.outputs.clone())),
                _ => {}
            }
        }

        Harness {
            netlistdb,
            aig,
            script,
            num_stages: stageds.len(),
            n_endpoints,
            input_ports,
            clock_port,
            output_ports,
            dsps,
            srls,
        }
    }

    fn stage_script(&self, stage: usize) -> &[u32] {
        let s = self.script.blocks_start[stage];
        let e = self.script.blocks_start[stage + 1];
        &self.script.blocks_data[s..e]
    }

    fn state_bit(state: &[u32], pos: u32) -> u8 {
        ((state[(pos >> 5) as usize] >> (pos & 31)) & 1) as u8
    }

    fn run(&self, num_cycles: usize, seed: u64) {
        let mut rng = ChaCha20Rng::seed_from_u64(seed);
        let size = self.script.reg_io_state_size as usize;

        // clock posedge flag bit position
        let (pe_flag, _ne) = self.aig.clock_pin2aigpins[&self.clock_port];
        let pe_pos = *self.script.input_map.get(&pe_flag).expect("posedge flag mapped");

        let mut behav = Behavioural::new(&self.netlistdb);
        let mut cur = vec![0u32; size];
        let mut sram = vec![0u32; self.script.sram_storage_size as usize];

        for cycle in 0..num_cycles {
            // fresh random input vector
            let mut inputs: HashMap<usize, u8> = HashMap::new();
            for &p in &self.input_ports {
                let name = self.netlistdb.pinnames[p].1.as_str();
                // keep rst rare so an accumulator actually accumulates
                let bit = if name == "rst" {
                    (rng.gen_range(0u32..8) == 0) as u8
                } else {
                    rng.gen::<bool>() as u8
                };
                inputs.insert(p, bit);
            }

            // ---- behavioural: settle, record, latch ----
            behav.settle(&inputs);
            let mut behav_outputs = Vec::new();
            for &p in &self.output_ports {
                behav_outputs.push(behav.netval[self.netlistdb.pin2net[p]]);
            }
            behav.latch();

            // ---- emulator: one full cycle == every major stage, in order ----
            for word in cur.iter_mut().take(self.script_states_start()) {
                *word = 0;
            }
            for (&p, &v) in &inputs {
                let aigpin = self.aig.pin2aigpin_iv[p] >> 1;
                if let Some(&pos) = self.script.input_map.get(&aigpin) {
                    if v != 0 {
                        cur[(pos >> 5) as usize] |= 1 << (pos & 31);
                    }
                }
            }
            cur[(pe_pos >> 5) as usize] |= 1 << (pe_pos & 31);

            let mut next = cur.clone();
            for stage in 0..self.num_stages {
                simulate_block_v1(self.stage_script(stage), &cur, &mut next, &mut sram, false);
            }

            // compare primary outputs (pre-latch combinational reads)
            for (i, &p) in self.output_ports.iter().enumerate() {
                let aigpin_iv = self.aig.pin2aigpin_iv[p];
                let pos = *self
                    .script
                    .output_map
                    .get(&aigpin_iv)
                    .unwrap_or_else(|| panic!("output {:?} not mapped", self.netlistdb.pinnames[p]));
                let got = Self::state_bit(&next, pos);
                assert_eq!(
                    got, behav_outputs[i],
                    "cycle {cycle}: primary output {:?} mismatch (emu {got} != beh {})",
                    self.netlistdb.pinnames[p], behav_outputs[i]
                );
            }

            // compare each DSP P register against the behavioural post-latch state
            for (cellid, outs) in &self.dsps {
                let want = behav.p_state.get(cellid).copied().unwrap_or(0);
                let mut got: u64 = 0;
                for (k, &o_k) in outs.iter().enumerate() {
                    if o_k == usize::MAX {
                        continue;
                    }
                    let pos = *self
                        .script
                        .input_map
                        .get(&o_k)
                        .expect("macro output bit mapped");
                    got |= (Self::state_bit(&next, pos) as u64) << k;
                }
                assert_eq!(
                    got, want,
                    "cycle {cycle}: DSP {cellid} P mismatch (emu {got:#014x} != beh {want:#014x})"
                );
            }

            // compare every SRL's full 32-bit shift state, post-latch. All 32
            // bits are committed unconditionally (decision 6): an uncommitted
            // state bit would silently break the shift, so none may be skipped.
            for (cellid, outs) in &self.srls {
                let want = behav.srl_state.get(cellid).copied().unwrap_or(0);
                let mut got: u32 = 0;
                for (k, &o_k) in outs.iter().enumerate() {
                    assert_ne!(
                        o_k,
                        usize::MAX,
                        "SRL {cellid} state bit {k} was never created"
                    );
                    let pos = *self
                        .script
                        .input_map
                        .get(&o_k)
                        .expect("SRL state bit mapped");
                    got |= (Self::state_bit(&next, pos) as u32) << k;
                }
                assert_eq!(
                    got, want,
                    "cycle {cycle}: SRL {cellid} state mismatch (emu {got:#010x} != beh {want:#010x})"
                );
            }

            cur = next;
        }
    }

    fn script_states_start(&self) -> usize {
        // words [0, states_start) are the primary-input region the emulator
        // never writes; rebuild it from scratch every cycle.
        ((self.script.input_layout.len() + 31) / 32).max(1)
    }
}

fn script_hash(script: &FlattenedScriptV1) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut s = DefaultHasher::new();
    script.blocks_data.hash(&mut s);
    s.finish()
}

/// `expect_hash`: when `Some(h)` with `h != 0`, assert the `blocks_data` hash
/// matches. Used as an ABI-stability regression guard — the values are pinned
/// to what the flattener produced before the change under test. `Some(0)`
/// means "record this one, not yet pinned".
fn run_case(
    rel_path: &str,
    top: &str,
    expect_stages: usize,
    num_cycles: usize,
    seed: u64,
    expect_hash: Option<u64>,
) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel_path);
    let h = Harness::build(&path, top, expect_stages);
    let hash = script_hash(&h.script);
    println!(
        "[{top}] {} aigpins, {} endpoint groups, {} macro(s), {} major stage(s), \
         reg/io state {} words, blocks_data hash {hash}",
        h.aig.num_aigpins,
        h.n_endpoints,
        h.aig.macros.len(),
        h.num_stages,
        h.script.reg_io_state_size
    );
    if let Some(want) = expect_hash {
        if want != 0 {
            assert_eq!(hash, want, "[{top}] blocks_data hash changed - script ABI perturbed");
        }
    }
    h.run(num_cycles, seed);
    println!("[{top}] differential check passed over {num_cycles} cycles");
}

// Pinned macro-free script hash (see run_case). Verified byte-identical to a
// clean pre-Part-A `HEAD` build of the flattener. If a legitimate, intended
// change to the macro-free script format lands, update this.
const SIMPLE_DFF_HASH: u64 = 4367312310778971198;

// Pinned Part A / Part B `blocks_data` hashes. Part C's metadata additions are
// designed to be ABI-neutral for every pre-existing design: the Class C kind
// tag is `0 << 28 | chain_len` (numerically what CARRY4 already emitted) and
// the Class R kind table lands on the metadata padding zeros an all-DSP
// partition already had. These pins turn that from a claim into an assertion.
const MAC_ACCUMULATOR_HASH: u64 = 1388458641175822285;
const RIPPLE_ADDER_HASH: u64 = 2711449478675204136;
const CARRY_LOGIC_CARRY_HASH: u64 = 17676642068319116734;
const MIXED_MACROS_HASH: u64 = 9165969952107539029;
// `yadder`'s netlist is regenerated whenever the pinned Yosys version changes
// (it *is* Yosys output), so this hash tracks the frontend, not the ABI: it was
// 299385802866267780 for Yosys 0.33's netlist and is the value below for
// Yosys 0.68's. The other five pins above must never move.
const YADDER_HASH: u64 = 2719296911916927336;

/// Structural checks on the Class C boomerang scheduler extension: the number
/// of major stages, and that a `macro -> logic -> macro` design routes the
/// first CARRY4's outputs through staged IO into the stage that evaluates the
/// second.
fn structural_checks() {
    use gem::aig::EndpointGroup;
    use gem::hwmacro::{MacroClass, MacroKind};

    // ripple_adder: one fused Carry4 { chain_len: 2 }, two major stages.
    {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/ripple_adder.gv");
        let db = NetlistDB::from_sverilog_file(&path, Some("ripple_adder"), &AIGPDKLeafPins()).unwrap();
        let aig = AIG::from_netlistdb(&db);
        let chain_lens: Vec<u16> = aig
            .macros
            .values()
            .filter_map(|m| match m.kind {
                MacroKind::Carry4 { chain_len } => Some(chain_len),
                _ => None,
            })
            .collect();
        assert_eq!(chain_lens, vec![2], "the two CARRY4 slices must fuse into chain_len 2");
        let stageds = build_staged_aigs(&aig, &[]);
        assert_eq!(stageds.len(), 2, "ripple_adder must produce exactly 2 major stages");
    }

    // carry_then_logic_then_carry: two un-fused CARRY4s, three major stages,
    // and CARRY4 #1's output pins present in stage 1's primary_inputs / staged
    // IO of stage 1 (0-indexed).
    {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/carry_then_logic_then_carry.gv");
        let db = NetlistDB::from_sverilog_file(
            &path,
            Some("carry_then_logic_then_carry"),
            &AIGPDKLeafPins(),
        )
        .unwrap();
        let aig = AIG::from_netlistdb(&db);
        let n_classc = aig
            .macros
            .values()
            .filter(|m| m.kind.class() == MacroClass::Combinational)
            .count();
        assert_eq!(n_classc, 2, "the two CARRY4s must NOT fuse (CI tied low on #2)");

        let stageds = build_staged_aigs(&aig, &[]);
        assert_eq!(stageds.len(), 3, "must produce exactly 3 major stages");

        // first CARRY4 realized in stage 0 -> its outputs seed stage 1's inputs
        let (_, _, staged0) = &stageds[0];
        let mut c1_outputs: Vec<usize> = Vec::new();
        for endpt in &staged0.endpoints {
            if let EndpointGroup::Macro(m) = aig.get_endpoint_group(*endpt) {
                if m.kind.class() == MacroClass::Combinational {
                    c1_outputs.extend(m.outputs.iter().copied().filter(|&o| o != usize::MAX));
                }
            }
        }
        assert!(!c1_outputs.is_empty(), "stage 0 must realize the first CARRY4");
        let pi1 = stageds[1].2.primary_inputs.as_ref().expect("stage 1 has staged inputs");
        for o in &c1_outputs {
            assert!(
                pi1.contains(o),
                "CARRY4 #1 output pin {o} must be a staged input of stage 1"
            );
        }
    }
    println!("[structural] Class C staging checks passed");
}

/// Structural checks specific to Part C's two-endpoint SRLC32E decomposition.
fn srl_structural_checks() {
    use gem::aig::{DriverType, EndpointGroup};
    use gem::hwmacro::{MacroClass, MacroKind, SRL_ADDR_WIDTH, SRL_STATE_WIDTH};
    use std::collections::HashSet;

    // srl_static_addr: one SRLC32E yields exactly one Srl32Shift (Class R) and
    // one Srl32Read (Class C); Q31's aigpin *is* state[31]; the read macro's Q
    // reaches the next stage as a staged input.
    {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/srl_static_addr.gv");
        let db = NetlistDB::from_sverilog_file(&path, Some("srl_static_addr"), &AIGPDKLeafPins())
            .unwrap();
        let aig = AIG::from_netlistdb(&db);

        let shifts: Vec<_> = aig
            .macros
            .iter()
            .filter(|(_, m)| m.kind == MacroKind::Srl32Shift)
            .collect();
        let reads: Vec<_> = aig
            .macros
            .iter()
            .filter(|(_, m)| m.kind == MacroKind::Srl32Read)
            .collect();
        assert_eq!(shifts.len(), 1, "one SRLC32E must yield one Srl32Shift");
        assert_eq!(reads.len(), 1, "one SRLC32E with Q used must yield one Srl32Read");
        let (&shift_cid, shift) = shifts[0];
        let (&read_cid, read) = reads[0];
        assert!(shift_cid < db.num_cells, "the shift half keys on the real cell id");
        assert!(
            read_cid >= db.num_cells,
            "the read half must key on a synthetic cell id above netlistdb.num_cells"
        );
        assert_eq!(shift.kind.class(), MacroClass::Registered);
        assert_eq!(read.kind.class(), MacroClass::Combinational);
        // every one of the 32 state bits exists (decision 6).
        for k in 0..SRL_STATE_WIDTH {
            assert_ne!(shift.outputs[k], usize::MAX, "state bit {k} missing");
            assert!(matches!(aig.drivers[shift.outputs[k]], DriverType::Macro(c) if c == shift_cid));
        }
        // The read macro's ONLY boomerang inputs are A[4:0]; the 32 state bits
        // come from the zero-copy feedback load, so they must not appear as
        // macro inputs at all (that is what keeps them off the boomerang and
        // out of the staged-IO write-outs).
        assert_eq!(
            read.inputs_iv.len(),
            SRL_ADDR_WIDTH,
            "the read half's only boomerang inputs must be A[4:0]"
        );
        assert_eq!(
            read.feedback_pin, shift.outputs[0],
            "the read half's feedback pin must be the shift half's state[0]"
        );
        let state_pins: HashSet<usize> = shift.outputs.iter().copied().collect();
        for &inp_iv in &read.inputs_iv {
            assert!(
                !state_pins.contains(&(inp_iv >> 1)),
                "no state bit may ride the boomerang as a read-macro input"
            );
        }
        // `for_each_input` is what makes a pin live across a split, so the
        // feedback pin must be invisible to it too.
        EndpointGroup::Macro(read).for_each_input(|p| {
            assert!(
                !state_pins.contains(&p),
                "a state bit reached for_each_input: it would be copied as \
                 staged IO across every split, which is the cost this \
                 zero-copy feedback exists to remove"
            );
        });
        // Q31 is state[31] — no macro, no split, no cost.
        let mut q31_pin = usize::MAX;
        for p in db.cell2pin.iter_set(shift_cid) {
            if db.pinnames[p].1.as_str() == "Q31" {
                q31_pin = p;
            }
        }
        assert_ne!(q31_pin, usize::MAX);
        assert_eq!(
            aig.pin2aigpin_iv[q31_pin],
            shift.outputs[SRL_STATE_WIDTH - 1] << 1,
            "Q31's aigpin must BE state[31]"
        );

        let stageds = build_staged_aigs(&aig, &[]);
        assert_eq!(stageds.len(), 2, "srl_static_addr must produce 2 major stages");
        // the read macro is realized in stage 0 and its Q becomes a staged
        // input of stage 1.
        let realized_read = stageds[0].2.endpoints.iter().any(|&e| {
            matches!(aig.get_endpoint_group(e),
                     EndpointGroup::Macro(m) if m.kind == MacroKind::Srl32Read)
        });
        assert!(realized_read, "stage 0 must realize the Srl32Read macro");
        let pi1 = stageds[1].2.primary_inputs.as_ref().expect("stage 1 has staged inputs");
        assert!(
            pi1.contains(&read.outputs[0]),
            "the read macro's Q must be a staged input of stage 1"
        );
    }

    // srl_cascade_q31 (decision 7): Q unconnected on both instances -> zero
    // Class C macros, zero forced splits, ONE major stage.
    {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/srl_cascade_q31.gv");
        let db = NetlistDB::from_sverilog_file(&path, Some("srl_cascade_q31"), &AIGPDKLeafPins())
            .unwrap();
        let aig = AIG::from_netlistdb(&db);
        let n_classc = aig
            .macros
            .values()
            .filter(|m| m.kind.class() == MacroClass::Combinational)
            .count();
        let n_shift = aig
            .macros
            .values()
            .filter(|m| m.kind == MacroKind::Srl32Shift)
            .count();
        assert_eq!(n_shift, 2, "both SRLC32Es must yield a Class R shift half");
        assert_eq!(
            n_classc, 0,
            "a pure Q31 cascade must create NO Class C macro (decision 7)"
        );
        let stageds = build_staged_aigs(&aig, &[]);
        assert_eq!(
            stageds.len(),
            1,
            "a pure Q31 cascade must cost zero forced major-stage splits"
        );
    }

    // heterogeneous: all three primitives; the SRL read's address comes off the
    // CARRY4, so there are two DISTINCT Class C macro levels -> 3 stages.
    {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/heterogeneous.gv");
        let db =
            NetlistDB::from_sverilog_file(&path, Some("heterogeneous"), &AIGPDKLeafPins()).unwrap();
        let aig = AIG::from_netlistdb(&db);
        let kinds: Vec<MacroKind> = aig.macros.values().map(|m| m.kind).collect();
        assert!(kinds.contains(&MacroKind::Dsp48e2), "DSP48E2 must survive");
        assert!(
            kinds.iter().any(|k| matches!(k, MacroKind::Carry4 { .. })),
            "CARRY4 must survive"
        );
        assert!(kinds.contains(&MacroKind::Srl32Shift), "SRLC32E shift half must exist");
        assert!(kinds.contains(&MacroKind::Srl32Read), "SRLC32E read half must exist");
        let (levels, _) = gem::staging::macro_levels(&aig);
        let mut distinct: Vec<usize> = levels.values().copied().collect();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            2,
            "CARRY4 and the SRL read must sit at two distinct macro levels, got {distinct:?}"
        );
        // Class R endpoints come before Class C ones, so a Part-A/B design's
        // endpoint indices are unperturbed by the new kinds.
        let n_reg = aig.primary_outputs.len() + aig.dffs.len() + aig.srams.len();
        let mut seen_classc = false;
        for endpt in n_reg..aig.num_endpoint_groups() {
            let EndpointGroup::Macro(m) = aig.get_endpoint_group(endpt) else {
                unreachable!()
            };
            match m.kind.class() {
                MacroClass::Registered => {
                    assert!(!seen_classc, "Class R macro endpoints must all precede Class C")
                }
                MacroClass::Combinational => seen_classc = true,
            }
        }

        // The zero-copy feedback's headline effect, measured on the design that
        // exercises every kind at once: not one of the 32 state bits of any SRL
        // is gathered into the boomerang, in ANY endpoint group of ANY stage.
        // Routing them as ordinary macro inputs (the previous design) cost 32
        // gathered bits and a second packed lane per SRL whose `Q` is read;
        // here the whole state arrives as one `input_state` word instead.
        let state_pins: HashSet<usize> = aig
            .macros
            .values()
            .filter(|m| m.kind == MacroKind::Srl32Shift)
            .flat_map(|m| m.outputs.iter().copied())
            .collect();
        let stageds = build_staged_aigs(&aig, &[]);
        assert_eq!(stageds.len(), 3, "heterogeneous must produce 3 major stages");
        let mut gathered_state_bits = 0usize;
        for (_, _, staged) in &stageds {
            for endpt in 0..staged.num_endpoint_groups() {
                staged.get_endpoint_group(&aig, endpt).for_each_input(|p| {
                    if state_pins.contains(&p) {
                        gathered_state_bits += 1;
                    }
                });
            }
        }
        // Exactly one bit is legitimately gathered: `Q31 = state[31]` drives
        // `q31_out`, so state[31] is a real AIG source with real fanout. The
        // other 31 — and, crucially, every bit the *read port* consumes — never
        // enter the boomerang at all. Routing the read port's state as macro
        // inputs (the previous design) gathered all 32 on top of this one.
        assert_eq!(
            gathered_state_bits, 1,
            "only state[31] (via Q31 -> q31_out) may reach the boomerang; the \
             read port's 32 state bits must arrive by the zero-copy feedback \
             load instead"
        );
        // and the read half's packed footprint shrank with them.
        assert_eq!(MacroKind::Srl32Read.num_perm_words(), 1);
    }
    println!("[structural] Part C SRLC32E decomposition checks passed");
}

fn main() {
    clilog::init_stderr_color_debug();
    clilog::set_max_print_count(clilog::Level::Warn, "NL_SV_LIT", 1);
    let expect = if SIMPLE_DFF_HASH != 0 { Some(SIMPLE_DFF_HASH) } else { None };
    run_case("tests/data/simple_dff.gv", "simple_dff", 1, 256, 0xA11CE, expect);
    run_case(
        "tests/data/mac_accumulator.gv",
        "mac_accumulator",
        1,
        512,
        0xD5948E2,
        Some(MAC_ACCUMULATOR_HASH),
    );
    run_case(
        "tests/data/ripple_adder.gv",
        "ripple_adder",
        2,
        400,
        0xCA224,
        Some(RIPPLE_ADDER_HASH),
    );
    run_case(
        "tests/data/carry_then_logic_then_carry.gv",
        "carry_then_logic_then_carry",
        3,
        400,
        0xC112C,
        Some(CARRY_LOGIC_CARRY_HASH),
    );
    run_case(
        "tests/data/mixed_macros.gv",
        "mixed_macros",
        2,
        400,
        0x717ED,
        Some(MIXED_MACROS_HASH),
    );
    // Yosys 0.68 frontend output: `yadder.v` (explicit CARRY4 + registered
    // inputs) run through `synth` + `dfflibmap`/`abc -liberty aigpdk_nomem.lib`.
    // Exercises GEM's parse of Yosys's bus-connected CARRY4 ports.
    run_case("tests/data/yosys_carry4.gv", "yadder", 2, 400, 0x70595, Some(YADDER_HASH));
    // ---- Part C: SRLC32E ----
    run_case("tests/data/srl_static_addr.gv", "srl_static_addr", 2, 400, 0x5121A, None);
    run_case("tests/data/srl_dynamic_addr.gv", "srl_dynamic_addr", 2, 400, 0x5D4DD, None);
    run_case("tests/data/srl_cascade_q31.gv", "srl_cascade_q31", 1, 400, 0x5CA5C, None);
    run_case("tests/data/srl_ce_gating.gv", "srl_ce_gating", 2, 400, 0x5CE04, None);
    run_case("tests/data/heterogeneous.gv", "heterogeneous", 3, 400, 0x4E7E30, None);
    // Yosys 0.68 frontend output: `ysrl.v` (explicit SRLC32E, registered data
    // and address) run through `synth -flatten` + `dfflibmap`/`abc -liberty
    // aigpdk_nomem.lib`. The blackbox survives intact with a bus-connected `A`
    // port; this exercises GEM's parse of that form.
    run_case("tests/data/yosys_srl.gv", "ysrl", 2, 400, 0x45521, None);
    structural_checks();
    srl_structural_checks();
    println!("macro_test: all differential checks passed");
}

#[cfg(test)]
mod tests {
    #[test]
    fn simple_dff_regression() {
        let expect = if super::SIMPLE_DFF_HASH != 0 { Some(super::SIMPLE_DFF_HASH) } else { None };
        super::run_case("tests/data/simple_dff.gv", "simple_dff", 1, 256, 0xA11CE, expect);
    }

    #[test]
    fn mac_accumulator_differential() {
        super::run_case(
            "tests/data/mac_accumulator.gv",
            "mac_accumulator",
            1,
            512,
            0xD5948E2,
            Some(super::MAC_ACCUMULATOR_HASH),
        );
    }

    #[test]
    fn ripple_adder_fused_carry4() {
        super::run_case(
            "tests/data/ripple_adder.gv",
            "ripple_adder",
            2,
            400,
            0xCA224,
            Some(super::RIPPLE_ADDER_HASH),
        );
    }

    #[test]
    fn carry_logic_carry_three_stages() {
        super::run_case(
            "tests/data/carry_then_logic_then_carry.gv",
            "carry_then_logic_then_carry",
            3,
            400,
            0xC112C,
            Some(super::CARRY_LOGIC_CARRY_HASH),
        );
    }

    #[test]
    fn mixed_class_r_and_class_c() {
        super::run_case(
            "tests/data/mixed_macros.gv",
            "mixed_macros",
            2,
            400,
            0x717ED,
            Some(super::MIXED_MACROS_HASH),
        );
    }

    #[test]
    fn classc_staging_structure() {
        super::structural_checks();
    }

    #[test]
    fn yosys_frontend_carry4() {
        super::run_case(
            "tests/data/yosys_carry4.gv",
            "yadder",
            2,
            400,
            0x70595,
            Some(super::YADDER_HASH),
        );
    }

    // ---- Part C: SRLC32E ----

    #[test]
    fn srl_static_address_differential() {
        super::run_case("tests/data/srl_static_addr.gv", "srl_static_addr", 2, 400, 0x5121A, None);
    }

    #[test]
    fn srl_dynamic_address_differential() {
        super::run_case("tests/data/srl_dynamic_addr.gv", "srl_dynamic_addr", 2, 400, 0x5D4DD, None);
    }

    #[test]
    fn srl_q31_cascade_costs_no_split() {
        super::run_case("tests/data/srl_cascade_q31.gv", "srl_cascade_q31", 1, 400, 0x5CA5C, None);
    }

    #[test]
    fn srl_clock_enable_gating() {
        super::run_case("tests/data/srl_ce_gating.gv", "srl_ce_gating", 2, 400, 0x5CE04, None);
    }

    #[test]
    fn heterogeneous_all_three_primitives() {
        super::run_case("tests/data/heterogeneous.gv", "heterogeneous", 3, 400, 0x4E7E30, None);
    }

    #[test]
    fn yosys_frontend_srlc32e() {
        super::run_case("tests/data/yosys_srl.gv", "ysrl", 2, 400, 0x45521, None);
    }

    #[test]
    fn srl_decomposition_structure() {
        super::srl_structural_checks();
    }
}
