// SPDX-FileCopyrightText: Copyright (c) 2024 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//! And-inverter graph format
//!
//! An AIG is derived from netlistdb synthesized in AIGPDK.

use netlistdb::{NetlistDB, GeneralPinName, Direction};
use indexmap::{IndexMap, IndexSet};
use crate::aigpdk::AIGPDK_SRAM_ADDR_WIDTH;
use crate::hwmacro::{
    MacroClass, MacroKind, SRL_ADDR_WIDTH, SRL_READ_ADDR_OFFSET, SRL_STATE_WIDTH,
};

/// A DFF.
#[derive(Debug, Default, Clone)]
pub struct DFF {
    /// The D input pin with invert (last bit)
    pub d_iv: usize,
    /// If the DFF is enabled, i.e., if the clock, S, or R is active.
    pub en_iv: usize,
    /// The Q pin output with invert.
    pub q: usize,
}

/// A ram block resembling the interface of `$__RAMGEM_SYNC_`.
#[derive(Debug, Default, Clone)]
pub struct RAMBlock {
    pub port_r_addr_iv: [usize; AIGPDK_SRAM_ADDR_WIDTH],

    /// controls whether r_rd_data should update. (from read clock)
    pub port_r_en_iv: usize,
    pub port_r_rd_data: [usize; 32],

    pub port_w_addr_iv: [usize; AIGPDK_SRAM_ADDR_WIDTH],
    /// controls whether memory should be updated.
    ///
    /// this is a combination of write enable and write clock.
    pub port_w_wr_en_iv: [usize; 32],
    pub port_w_wr_data_iv: [usize; 32],
}

/// A word-level hardware macro instance (Part A: DSP48E2).
///
/// Mirrors [`RAMBlock`]: the 124 canonical input bits are boomerang
/// endpoints (co-partitioned because they belong to one endpoint group),
/// and the 48 registered output bits are AIG sources on the next cycle.
/// See [`crate::hwmacro`] for the packed-word layout and the datapath.
#[derive(Debug, Clone)]
pub struct MacroBlock {
    /// the macro kind. determines shape, packing, and semantics.
    pub kind: MacroKind,
    /// canonical-order input pins with invert bit in the LSB.
    /// length is `kind.num_input_bits()`.
    pub inputs_iv: Vec<usize>,
    /// canonical-order output AIG pins, no invert.
    /// length is `kind.num_output_bits()`; `usize::MAX` marks an
    /// unconnected output bit.
    pub outputs: Vec<usize>,
    /// clock-edge & (CEP | RSTP), with invert bit. when active the
    /// registered output commits. RSTP is OR'd in here (and also zeroes
    /// the data inside the macro) because it has precedence over CEP on
    /// the real DSP48E2.
    pub en_iv: usize,
    /// For a kind that takes a **zero-copy state feedback load** instead of
    /// routing its state through the boomerang, the AIG pin of bit 0 of the
    /// paired macro's committed state; `usize::MAX` otherwise.
    ///
    /// Only [`MacroKind::Srl32Read`] uses it: the 32 state bits of its paired
    /// [`MacroKind::Srl32Shift`] land contiguously in one 32-bit I/O word, and
    /// the flattener turns this pin into that word's *global* index via
    /// `input_map`, parking it in the Class C metadata payload. The kernel then
    /// reads all 32 bits with a single `input_state[payload]`.
    pub feedback_pin: usize,
}

impl Default for MacroBlock {
    fn default() -> Self {
        MacroBlock::new(MacroKind::default())
    }
}

impl MacroBlock {
    /// A fresh macro block of the given kind, with `inputs_iv` tied low and all
    /// outputs unconnected.
    pub fn new(kind: MacroKind) -> Self {
        MacroBlock {
            inputs_iv: vec![0; kind.num_input_bits()],
            outputs: vec![usize::MAX; kind.num_output_bits()],
            en_iv: 0,
            feedback_pin: usize::MAX,
            kind,
        }
    }
}

/// A type of endpoint group. can be a primary output-related pin,
/// a D flip-flop, a ram block, or a word-level macro.
///
/// A group means a task for the partition to complete.
/// For primary output pins, the task is just to store.
/// For DFFs, the task is to store only when the clock is enable.
/// For RAMBlocks, the task is to simulate a sync SRAM.
/// A StagedIOPin indicates a temporary live pin between different
/// major stages but reside in the same simulated cycle.
#[derive(Debug, Copy, Clone)]
pub enum EndpointGroup<'i> {
    PrimaryOutput(usize),
    DFF(&'i DFF),
    RAMBlock(&'i RAMBlock),
    Macro(&'i MacroBlock),
    StagedIOPin(usize),
}

impl EndpointGroup<'_> {
    /// Enumerate all related aigpin inputs for this endpoint group.
    ///
    /// The enumerated inputs may have duplicates.
    pub fn for_each_input(self, mut f_nz: impl FnMut(usize)) {
        let mut f = |i| {
            if i >= 1 { f_nz(i); }
        };
        match self {
            Self::PrimaryOutput(idx) => f(idx >> 1),
            Self::DFF(dff) => {
                f(dff.en_iv >> 1);
                f(dff.d_iv >> 1);
            },
            Self::RAMBlock(ram) => {
                f(ram.port_r_en_iv >> 1);
                for i in 0..13 {
                    f(ram.port_r_addr_iv[i] >> 1);
                    f(ram.port_w_addr_iv[i] >> 1);
                }
                for i in 0..32 {
                    f(ram.port_w_wr_en_iv[i] >> 1);
                    f(ram.port_w_wr_data_iv[i] >> 1);
                }
            },
            Self::Macro(m) => {
                f(m.en_iv >> 1);
                for &inp_iv in &m.inputs_iv {
                    f(inp_iv >> 1);
                }
                // `feedback_pin` is deliberately NOT enumerated: it is read
                // directly out of global state by the kernel, so it must not
                // become a boomerang input of this endpoint group.
            },
            Self::StagedIOPin(idx) => f(idx),
        }
    }
}

/// The driver type of an AIG pin.
#[derive(Debug, Clone)]
pub enum DriverType {
    /// Driven by an and gate.
    ///
    /// The inversion bit is stored as the last bits in
    /// two input indices.
    ///
    /// Only this type has combinational fan-in.
    AndGate(usize, usize),
    /// Driven by a primary input port (with its netlistdb id).
    InputPort(usize),
    /// Driven by a clock flag (with clock port netlistdb id, and pos/negedge)
    InputClockFlag(usize, u8),
    /// Driven by a DFF (with its index)
    DFF(usize),
    /// Driven by a 13-bit by 32-bit RAM block (with its index)
    SRAM(usize),
    /// Driven by a word-level hardware macro (with its index).
    /// Output pins are AIG sources, exactly like [`DriverType::SRAM`].
    Macro(usize),
    /// Tie0: tied to zero. Only the 0-th aig pin is allowed to have this.
    Tie0
}

/// An AIG associated with a netlistdb.
#[derive(Debug, Default)]
pub struct AIG {
    /// The number of AIG pins.
    ///
    /// This number might be smaller than num_pins in netlistdb,
    /// because inverters and buffers are merged when possible.
    /// It might also be larger because we may add mux circuits.
    ///
    /// AIG pins are numbered from 1 to num_aigpins inclusive.
    /// The AIG pin id zero (0) is tied to 0.
    ///
    /// AIG pins are guaranteed to have topological order.
    pub num_aigpins: usize,
    /// The mapping from a netlistdb pin to an AIG pin.
    ///
    /// The inversion bit is stored as the last bit.
    /// E.g., `pin2aigpin_iv[pin_id] = aigpin_id << 1 | invert`.
    pub pin2aigpin_iv: Vec<usize>,
    /// The clock pins map. Every clock pin has a pair of flag pins
    /// showing if they are posedge/negedge.
    ///
    /// The flag pin can be empty which means the circuit is not
    /// active with that edge.
    pub clock_pin2aigpins: IndexMap<usize, (usize, usize)>,
    /// The driver types of AIG pins.
    pub drivers: Vec<DriverType>,
    /// A cache for identical and gates.
    pub and_gate_cache: IndexMap<(usize, usize), usize>,
    /// Unique primary output aigpin indices
    pub primary_outputs: IndexSet<usize>,
    /// The D flip-flops (DFFs), indexed by cell id
    pub dffs: IndexMap<usize, DFF>,
    /// The SRAMs, indexed by cell id
    pub srams: IndexMap<usize, RAMBlock>,
    /// The word-level hardware macros, indexed by cell id.
    pub macros: IndexMap<usize, MacroBlock>,
    /// Positions into [`AIG::macros`] giving the macro-endpoint order:
    /// Class R (registered) macros first, then Class C (combinational) ones.
    /// This keeps a design with no Class C macro at the same endpoint indices
    /// it had before Part B.
    pub macro_order: Vec<usize>,
    /// The fanout CSR start array.
    pub fanouts_start: Vec<usize>,
    /// The fanout CSR array.
    pub fanouts: Vec<usize>,
}

impl AIG {
    fn add_aigpin(&mut self, driver: DriverType) -> usize {
        self.num_aigpins += 1;
        self.drivers.push(driver);
        self.num_aigpins
    }

    fn add_and_gate(&mut self, a: usize, b: usize) -> usize {
        assert_ne!(a | 1, usize::MAX);
        assert_ne!(b | 1, usize::MAX);
        if a == 0 || b == 0 {
            return 0
        }
        if a == 1 {
            return b
        }
        if b == 1 {
            return a
        }
        let (a, b) = if a < b { (a, b) } else { (b, a) };
        if let Some(o) = self.and_gate_cache.get(&(a, b)) {
            return o << 1;
        }
        let aigpin = self.add_aigpin(DriverType::AndGate(a, b));
        self.and_gate_cache.insert((a, b), aigpin);
        aigpin << 1
    }

    /// given a clock pin, trace back to clock root and return its
    /// enable signal (with invert bit).
    ///
    /// if result is 0, that means the pin is dangled.
    /// if an error occurs because of a undecipherable multi-input cell,
    /// we will return in error the last output pin index of that cell.
    fn trace_clock_pin(
        &mut self,
        netlistdb: &NetlistDB,
        pinid: usize, is_negedge: bool,
        // should we ignore cklnqd in this tracing.
        // if set to true, we will treat cklnqd as a simple buffer.
        // otherwise, we assert that cklnqd/en is already built in
        // our aig mapping (pin2aigpin_iv).
        ignore_cklnqd: bool,
    ) -> Result<usize, usize> {
        if netlistdb.pindirect[pinid] == Direction::I {
            let netid = netlistdb.pin2net[pinid];
            if Some(netid) == netlistdb.net_zero || Some(netid) == netlistdb.net_one {
                return Ok(0)
            }
            let root = netlistdb.net2pin.items[
                netlistdb.net2pin.start[netid]
            ];
            return self.trace_clock_pin(
                netlistdb, root, is_negedge,
                ignore_cklnqd
            )
        }
        let cellid = netlistdb.pin2cell[pinid];
        if cellid == 0 {
            let clkentry = self.clock_pin2aigpins.entry(pinid)
                .or_insert((usize::MAX, usize::MAX));
            let clksignal = match is_negedge {
                false => clkentry.0,
                true => clkentry.1
            };
            if clksignal != usize::MAX {
                return Ok(clksignal << 1)
            }
            let aigpin = self.add_aigpin(DriverType::InputClockFlag(pinid, is_negedge as u8));
            let clkentry = self.clock_pin2aigpins.get_mut(&pinid).unwrap();
            let clksignal = match is_negedge {
                false => &mut clkentry.0,
                true => &mut clkentry.1
            };
            *clksignal = aigpin;
            return Ok(aigpin << 1)
        }
        let mut pin_a = usize::MAX;
        let mut pin_cp = usize::MAX;
        let mut pin_en = usize::MAX;
        let celltype = netlistdb.celltypes[cellid].as_str();
        if !matches!(celltype, "INV" | "BUF" | "CKLNQD") {
            clilog::error!("cell type {} supported on clock path. expecting only INV, BUF, or CKLNQD", celltype);
            return Err(pinid)
        }
        for ipin in netlistdb.cell2pin.iter_set(cellid) {
            if netlistdb.pindirect[ipin] == Direction::I {
                match netlistdb.pinnames[ipin].1.as_str() {
                    "A" => pin_a = ipin,
                    "CP" => pin_cp = ipin,
                    "E" => pin_en = ipin,
                    i @ _ => {
                        clilog::error!("input pin {} unexpected for ck element {}", i, celltype);
                        return Err(ipin)
                    }
                }
            }
        }
        match celltype {
            "INV" => {
                assert_ne!(pin_a, usize::MAX);
                self.trace_clock_pin(
                    netlistdb, pin_a, !is_negedge,
                    ignore_cklnqd
                )
            },
            "BUF" => {
                assert_ne!(pin_a, usize::MAX);
                self.trace_clock_pin(
                    netlistdb, pin_a, is_negedge,
                    ignore_cklnqd
                )
            },
            "CKLNQD" => {
                assert_ne!(pin_cp, usize::MAX);
                assert_ne!(pin_en, usize::MAX);
                let ck_iv = self.trace_clock_pin(
                    netlistdb, pin_cp, is_negedge,
                    ignore_cklnqd
                )?;
                if ignore_cklnqd {
                    return Ok(ck_iv)
                }
                let en_iv = self.pin2aigpin_iv[pin_en];
                assert_ne!(en_iv, usize::MAX, "clken not built");
                Ok(self.add_and_gate(ck_iv, en_iv))
            },
            _ => unreachable!()
        }
    }

    /// recursively add aig pins for netlistdb pins
    ///
    /// for sequential logics like DFF and RAM,
    /// 1. their netlist pin inputs are not patched,
    /// 2. their aig pin inputs (in dffs and srams arrays) will be
    ///    patched to include mux -- but not inside this function.
    /// 3. their netlist/aig outputs are directly built here,
    ///    with possible patches for asynchronous DFFSR polyfill.
    fn dfs_netlistdb_build_aig(
        &mut self,
        netlistdb: &NetlistDB,
        topo_vis: &mut Vec<bool>,
        topo_instack: &mut Vec<bool>,
        srl_read_cid: &IndexMap<usize, usize>,
        pinid: usize
    ) {
        if topo_instack[pinid] {
            panic!("circuit has a loop around pin {}",
                   netlistdb.pinnames[pinid].dbg_fmt_pin());
        }
        if topo_vis[pinid] {
            return
        }
        topo_vis[pinid] = true;
        topo_instack[pinid] = true;
        let netid = netlistdb.pin2net[pinid];
        let cellid = netlistdb.pin2cell[pinid];
        let celltype = netlistdb.celltypes[cellid].as_str();
        if netlistdb.pindirect[pinid] == Direction::I {
            if Some(netid) == netlistdb.net_zero {
                self.pin2aigpin_iv[pinid] = 0;
            }
            else if Some(netid) == netlistdb.net_one {
                self.pin2aigpin_iv[pinid] = 1;
            }
            else {
                let root = netlistdb.net2pin.items[
                    netlistdb.net2pin.start[netid]
                ];
                self.dfs_netlistdb_build_aig(
                    netlistdb, topo_vis, topo_instack, srl_read_cid,
                    root
                );
                self.pin2aigpin_iv[pinid] = self.pin2aigpin_iv[root];
                if cellid == 0 {
                    self.primary_outputs.insert(self.pin2aigpin_iv[pinid]);
                }
            }
        }
        else if cellid == 0 {
            let aigpin = self.add_aigpin(
                DriverType::InputPort(pinid)
            );
            self.pin2aigpin_iv[pinid] = aigpin << 1;
        }
        else if matches!(celltype, "DFF" | "DFFSR") {
            let q = self.add_aigpin(DriverType::DFF(cellid));
            let dff = self.dffs.entry(cellid).or_default();
            dff.q = q;
            let mut ap_s_iv = 1;
            let mut ap_r_iv = 1;
            let mut q_out = q << 1;
            for pinid in netlistdb.cell2pin.iter_set(cellid) {
                if !matches!(netlistdb.pinnames[pinid].1.as_str(), "S" | "R") {
                    continue
                }
                self.dfs_netlistdb_build_aig(
                    netlistdb, topo_vis, topo_instack, srl_read_cid, pinid
                );
                let prev = self.pin2aigpin_iv[pinid];
                match netlistdb.pinnames[pinid].1.as_str() {
                    "S" => ap_s_iv = prev,
                    "R" => ap_r_iv = prev,
                    _ => unreachable!()
                }
            }
            q_out = self.add_and_gate(q_out ^ 1, ap_s_iv) ^ 1;
            q_out = self.add_and_gate(q_out, ap_r_iv);
            self.pin2aigpin_iv[pinid] = q_out;
        }
        else if celltype == "LATCH" {
            panic!("latches are intentionally UNSUPPORTED by GEM, \
                    except in identified gated clocks. \n\
                    you can link a FF&MUX-based LATCH module, \
                    but most likely that is NOT the right solution. \n\
                    check all your assignments inside always@(*) block \
                    to make sure they cover all scenarios.");
        }
        else if celltype == "$__RAMGEM_SYNC_" {
            let o = self.add_aigpin(DriverType::SRAM(cellid));
            self.pin2aigpin_iv[pinid] = o << 1;
            assert_eq!(netlistdb.pinnames[pinid].1.as_str(),
                       "PORT_R_RD_DATA");
            let sram = self.srams.entry(cellid).or_default();
            sram.port_r_rd_data[netlistdb.pinnames[pinid].2.unwrap() as usize] = o;
        }
        else if celltype == "GEM_DSP48E2" {
            let o = self.add_aigpin(DriverType::Macro(cellid));
            self.pin2aigpin_iv[pinid] = o << 1;
            assert_eq!(netlistdb.pinnames[pinid].1.as_str(), "P");
            let m = self.macros.entry(cellid).or_default();
            m.outputs[netlistdb.pinnames[pinid].2.unwrap() as usize] = o;
        }
        else if celltype == "CARRY4" {
            // one CARRY4 slice = a Class C macro with chain_len 1; a maximal
            // CO[3]->CI cascade is fused into one longer block after parsing.
            let o = self.add_aigpin(DriverType::Macro(cellid));
            self.pin2aigpin_iv[pinid] = o << 1;
            let bit = netlistdb.pinnames[pinid].2.unwrap() as usize;
            let idx = match netlistdb.pinnames[pinid].1.as_str() {
                "O" => crate::hwmacro::carry4_o_index(1, bit),
                "CO" => crate::hwmacro::carry4_co_index(1, bit),
                other => panic!("unexpected CARRY4 output pin {other}"),
            };
            let m = self.macros.entry(cellid)
                .or_insert_with(|| MacroBlock::new(MacroKind::Carry4 { chain_len: 1 }));
            m.outputs[idx] = o;
        }
        else if celltype == "SRLC32E" {
            // One SRLC32E cell becomes two macro blocks joined by a zero-copy
            // state feedback (see `MacroBlock::feedback_pin`):
            //   * `Srl32Shift` (Class R), keyed on the real cell id, owning the
            //     32-bit state. `Q31` *is* `state[31]` — no macro, no split.
            //   * `Srl32Read`  (Class C), keyed on a synthetic cell id above
            //     `netlistdb.num_cells`, created only when `Q` is connected.
            // Both are allocated on the first visit to *any* output pin of the
            // cell, so the aigpins stay in topological order (the invariant
            // `macro_levels` and `from_split` rely on). Every one of the 32
            // state pins is created eagerly, connected or not: an uncommitted
            // state bit would silently break the shift.
            if !self.macros.contains_key(&cellid) {
                let mut sb = MacroBlock::new(MacroKind::Srl32Shift);
                for k in 0..SRL_STATE_WIDTH {
                    sb.outputs[k] = self.add_aigpin(DriverType::Macro(cellid));
                }
                self.macros.insert(cellid, sb);
                if let Some(&read_cid) = srl_read_cid.get(&cellid) {
                    let mut rb = MacroBlock::new(MacroKind::Srl32Read);
                    rb.outputs[0] = self.add_aigpin(DriverType::Macro(read_cid));
                    // Class C: the scratch slot is committed every cycle.
                    rb.en_iv = 1;
                    self.macros.insert(read_cid, rb);
                }
            }
            match netlistdb.pinnames[pinid].1.as_str() {
                "Q31" => {
                    let st31 = self.macros[&cellid].outputs[SRL_STATE_WIDTH - 1];
                    self.pin2aigpin_iv[pinid] = st31 << 1;
                },
                "Q" => {
                    self.pin2aigpin_iv[pinid] = match srl_read_cid.get(&cellid) {
                        // `Q` is unconnected: no Class C macro exists, so this
                        // pin drives nothing. Tie it low.
                        None => 0,
                        Some(&read_cid) => self.macros[&read_cid].outputs[0] << 1,
                    };
                },
                other => panic!("unexpected SRLC32E output pin {other}"),
            }
        }
        else if celltype == "CKLNQD" {
            let mut prev_cp = usize::MAX;
            let mut prev_en = usize::MAX;
            for pinid in netlistdb.cell2pin.iter_set(cellid) {
                match netlistdb.pinnames[pinid].1.as_str() {
                    "CP" => prev_cp = pinid,
                    "E" => prev_en = pinid,
                    _ => {}
                }
            }
            assert_ne!(prev_cp, usize::MAX);
            assert_ne!(prev_en, usize::MAX);
            for prev in [prev_cp, prev_en] {
                self.dfs_netlistdb_build_aig(
                    netlistdb, topo_vis, topo_instack, srl_read_cid,
                    prev
                );
            }
            // do not define pin2aigpin_iv[pinid] which is CKLNQD/Q and unused in logic.
        }
        else {
            let mut prev_a = usize::MAX;
            let mut prev_b = usize::MAX;
            for pinid in netlistdb.cell2pin.iter_set(cellid) {
                match netlistdb.pinnames[pinid].1.as_str() {
                    "A" => prev_a = pinid,
                    "B" => prev_b = pinid,
                    _ => {}
                }
            }
            for prev in [prev_a, prev_b] {
                if prev != usize::MAX {
                    self.dfs_netlistdb_build_aig(
                        netlistdb, topo_vis, topo_instack, srl_read_cid,
                        prev
                    );
                }
            }
            match celltype {
                "AND2_00_0" | "AND2_01_0" | "AND2_10_0" | "AND2_11_0" | "AND2_11_1" => {
                    assert_ne!(prev_a, usize::MAX);
                    assert_ne!(prev_b, usize::MAX);
                    let name = netlistdb.celltypes[cellid].as_bytes();
                    let iv_a = name[5] - b'0';
                    let iv_b = name[6] - b'0';
                    let iv_y = name[8] - b'0';
                    let apid = self.add_and_gate(
                        self.pin2aigpin_iv[prev_a] ^ (iv_a as usize),
                        self.pin2aigpin_iv[prev_b] ^ (iv_b as usize),
                    ) ^ (iv_y as usize);
                    self.pin2aigpin_iv[pinid] = apid;
                },
                "INV" => {
                    assert_ne!(prev_a, usize::MAX);
                    self.pin2aigpin_iv[pinid] = self.pin2aigpin_iv[prev_a] ^ 1;
                },
                "BUF" => {
                    assert_ne!(prev_a, usize::MAX);
                    self.pin2aigpin_iv[pinid] = self.pin2aigpin_iv[prev_a];
                },
                _ => unreachable!()
            }
        }
        topo_instack[pinid] = false;
    }

    pub fn from_netlistdb(netlistdb: &NetlistDB) -> AIG {
        let mut aig = AIG {
            num_aigpins: 0,
            pin2aigpin_iv: vec![usize::MAX; netlistdb.num_pins],
            drivers: vec![DriverType::Tie0],
            ..Default::default()
        };

        // Synthetic cell ids for the Class C read half of every SRLC32E whose
        // `Q` is actually used, allocated as `netlistdb.num_cells + j` in cell
        // order (deterministic, and known before the DFS so the read macro's
        // output pin can be created at first visit). An SRLC32E with `Q`
        // unconnected — a pure `Q31 -> D` cascade / delay line — gets no entry
        // here and therefore no Class C macro, and so costs zero forced
        // major-stage splits.
        let mut srl_read_cid: IndexMap<usize, usize> = IndexMap::new();
        for cellid in 1..netlistdb.num_cells {
            if netlistdb.celltypes[cellid].as_str() != "SRLC32E" {
                continue
            }
            for pinid in netlistdb.cell2pin.iter_set(cellid) {
                if netlistdb.pinnames[pinid].1.as_str() != "Q" {
                    continue
                }
                let netid = netlistdb.pin2net[pinid];
                // net2pin holds the driver plus every reader; a length of one
                // means nothing reads `Q`.
                if netlistdb.net2pin.len(netid) > 1 {
                    let next = netlistdb.num_cells + srl_read_cid.len();
                    srl_read_cid.insert(cellid, next);
                }
            }
        }

        for cellid in 1..netlistdb.num_cells {
            if !matches!(netlistdb.celltypes[cellid].as_str(),
                         "DFF" | "DFFSR" | "$__RAMGEM_SYNC_" | "GEM_DSP48E2"
                         | "SRLC32E") {
                continue
            }
            for pinid in netlistdb.cell2pin.iter_set(cellid) {
                if !matches!(netlistdb.pinnames[pinid].1.as_str(),
                            "CLK" | "PORT_R_CLK" | "PORT_W_CLK") {
                    continue
                }
                if let Err(pinid) = aig.trace_clock_pin(
                    netlistdb, pinid, false,
                    true
                ) {
                    use netlistdb::GeneralHierName;
                    panic!("Tracing clock pin of cell {} error: \
                            there is a multi-input cell driving {} \
                            that clocks this sequential element. \
                            Clock gating need to be manually patched atm.",
                           netlistdb.cellnames[cellid].dbg_fmt_hier(),
                           netlistdb.pinnames[pinid].dbg_fmt_pin());
                }
            }
        }
        for (&clk, &(flagr, flagf)) in &aig.clock_pin2aigpins {
            clilog::info!(
                "inferred clock port {} ({})",
                netlistdb.pinnames[clk].dbg_fmt_pin(),
                match (flagr, flagf) {
                    (_, usize::MAX) => "posedge",
                    (usize::MAX, _) => "negedge",
                    _ => "posedge & negedge"
                }
            );
        }

        let mut topo_vis = vec![false; netlistdb.num_pins];
        let mut topo_instack = vec![false; netlistdb.num_pins];

        for pinid in 0..netlistdb.num_pins {
            aig.dfs_netlistdb_build_aig(
                netlistdb, &mut topo_vis, &mut topo_instack, &srl_read_cid,
                pinid
            );
        }

        for cellid in 0..netlistdb.num_cells {
            if matches!(netlistdb.celltypes[cellid].as_str(), "DFF" | "DFFSR") {
                let mut ap_s_iv = 1;
                let mut ap_r_iv = 1;
                let mut ap_d_iv = 0;
                let mut ap_clken_iv = 0;
                for pinid in netlistdb.cell2pin.iter_set(cellid) {
                    let pin_iv = aig.pin2aigpin_iv[pinid];
                    match netlistdb.pinnames[pinid].1.as_str() {
                        "D" => ap_d_iv = pin_iv,
                        "S" => ap_s_iv = pin_iv,
                        "R" => ap_r_iv = pin_iv,
                        "CLK" => ap_clken_iv = aig.trace_clock_pin(
                            netlistdb, pinid, false,
                            false
                        ).unwrap(),
                        _ => {}
                    }
                }
                let mut d_in = ap_d_iv;

                d_in = aig.add_and_gate(d_in ^ 1, ap_s_iv) ^ 1;
                ap_clken_iv = aig.add_and_gate(ap_clken_iv ^ 1, ap_s_iv) ^ 1;
                d_in = aig.add_and_gate(d_in, ap_r_iv);
                ap_clken_iv = aig.add_and_gate(ap_clken_iv ^ 1, ap_r_iv) ^ 1;
                let dff = aig.dffs.entry(cellid).or_default();
                dff.en_iv = ap_clken_iv;
                dff.d_iv = d_in;
                assert_ne!(dff.q, 0);
            }
            else if netlistdb.celltypes[cellid].as_str() == "$__RAMGEM_SYNC_" {
                let mut sram = aig.srams.entry(cellid).or_default().clone();
                let mut write_clken_iv = 0;
                for pinid in netlistdb.cell2pin.iter_set(cellid) {
                    let bit = netlistdb.pinnames[pinid].2.map(|i| i as usize);
                    let pin_iv = aig.pin2aigpin_iv[pinid];
                    match netlistdb.pinnames[pinid].1.as_str() {
                        "PORT_R_ADDR" => {
                            sram.port_r_addr_iv[bit.unwrap()] = pin_iv;
                        },
                        "PORT_R_CLK" => {
                            sram.port_r_en_iv = aig.trace_clock_pin(
                                netlistdb, pinid, false,
                                false
                            ).unwrap();
                        },
                        "PORT_W_ADDR" => {
                            sram.port_w_addr_iv[bit.unwrap()] = pin_iv;
                        }
                        "PORT_W_CLK" => {
                            write_clken_iv = aig.trace_clock_pin(
                                netlistdb, pinid, false,
                                false
                            ).unwrap();
                        },
                        "PORT_W_WR_DATA" => {
                            sram.port_w_wr_data_iv[bit.unwrap()] = pin_iv;
                        },
                        "PORT_W_WR_EN" => {
                            sram.port_w_wr_en_iv[bit.unwrap()] = pin_iv;
                        },
                        _ => {}
                    }
                }
                for i in 0..32 {
                    let or_en = sram.port_w_wr_en_iv[i];
                    let or_en = aig.add_and_gate(
                        or_en, write_clken_iv
                    );
                    sram.port_w_wr_en_iv[i] = or_en;
                }
                *aig.srams.get_mut(&cellid).unwrap() = sram;
            }
            else if netlistdb.celltypes[cellid].as_str() == "GEM_DSP48E2" {
                use crate::hwmacro::{
                    DSP_A_OFFSET, DSP_D_OFFSET, DSP_B_OFFSET, DSP_C_OFFSET,
                    DSP_OPMODE_S_OFFSET, DSP_USE_D_INDEX, DSP_RSTP_INDEX,
                };
                let mut m = aig.macros.entry(cellid).or_default().clone();
                let mut clk_iv = 0;
                let mut cep_iv = 0;
                let mut rstp_iv = 0;
                for pinid in netlistdb.cell2pin.iter_set(cellid) {
                    let bit = netlistdb.pinnames[pinid].2.map(|i| i as usize);
                    let pin_iv = aig.pin2aigpin_iv[pinid];
                    match netlistdb.pinnames[pinid].1.as_str() {
                        "A" => m.inputs_iv[DSP_A_OFFSET + bit.unwrap()] = pin_iv,
                        "D" => m.inputs_iv[DSP_D_OFFSET + bit.unwrap()] = pin_iv,
                        "B" => m.inputs_iv[DSP_B_OFFSET + bit.unwrap()] = pin_iv,
                        "C" => m.inputs_iv[DSP_C_OFFSET + bit.unwrap()] = pin_iv,
                        "OPMODE_S" => {
                            m.inputs_iv[DSP_OPMODE_S_OFFSET + bit.unwrap()] = pin_iv
                        },
                        "USE_D" => m.inputs_iv[DSP_USE_D_INDEX] = pin_iv,
                        "RSTP" => {
                            m.inputs_iv[DSP_RSTP_INDEX] = pin_iv;
                            rstp_iv = pin_iv;
                        },
                        "CEP" => cep_iv = pin_iv,
                        "CLK" => clk_iv = aig.trace_clock_pin(
                            netlistdb, pinid, false, false
                        ).unwrap(),
                        _ => {}
                    }
                }
                // en_iv = clk & (CEP | RSTP). the OR is built with the
                // De Morgan idiom  add_and_gate(x^1, y^1) ^ 1  ==  x | y,
                // matching how port_w_wr_en_iv is assembled above.
                let cep_or_rstp = aig.add_and_gate(cep_iv ^ 1, rstp_iv ^ 1) ^ 1;
                m.en_iv = aig.add_and_gate(clk_iv, cep_or_rstp);
                *aig.macros.get_mut(&cellid).unwrap() = m;
            }
            else if netlistdb.celltypes[cellid].as_str() == "CARRY4" {
                use crate::hwmacro::{
                    carry4_s_index, carry4_di_index, carry4_cyinit_index, carry4_cin_index,
                };
                let mut m = aig.macros.entry(cellid)
                    .or_insert_with(|| MacroBlock::new(MacroKind::Carry4 { chain_len: 1 }))
                    .clone();
                for pinid in netlistdb.cell2pin.iter_set(cellid) {
                    let bit = netlistdb.pinnames[pinid].2.map(|i| i as usize);
                    let pin_iv = aig.pin2aigpin_iv[pinid];
                    match netlistdb.pinnames[pinid].1.as_str() {
                        "S" => m.inputs_iv[carry4_s_index(1, bit.unwrap())] = pin_iv,
                        "DI" => m.inputs_iv[carry4_di_index(1, bit.unwrap())] = pin_iv,
                        "CYINIT" => m.inputs_iv[carry4_cyinit_index(1)] = pin_iv,
                        "CI" => m.inputs_iv[carry4_cin_index(1)] = pin_iv,
                        _ => {}
                    }
                }
                // Class C macro: the registered-commit gate is tied high, so
                // the staged-IO scratch slot is written unconditionally.
                m.en_iv = 1;
                *aig.macros.get_mut(&cellid).unwrap() = m;
            }
            else if netlistdb.celltypes[cellid].as_str() == "SRLC32E" {
                let mut clk_iv = 0;
                let mut ce_iv = 0;
                let mut d_iv = 0;
                let mut a_iv = [0usize; SRL_ADDR_WIDTH];
                for pinid in netlistdb.cell2pin.iter_set(cellid) {
                    let bit = netlistdb.pinnames[pinid].2.map(|i| i as usize);
                    let pin_iv = aig.pin2aigpin_iv[pinid];
                    match netlistdb.pinnames[pinid].1.as_str() {
                        "D" => d_iv = pin_iv,
                        "CE" => ce_iv = pin_iv,
                        "A" => a_iv[bit.unwrap()] = pin_iv,
                        "CLK" => clk_iv = aig.trace_clock_pin(
                            netlistdb, pinid, false, false
                        ).unwrap(),
                        _ => {}
                    }
                }
                if ce_iv == 0 {
                    use netlistdb::GeneralHierName;
                    clilog::warn!(
                        SRL_CE_TIE0,
                        "SRLC32E {} has CE tied low: its state can never shift.",
                        netlistdb.cellnames[cellid].dbg_fmt_hier()
                    );
                }
                // Class R half: en_iv = CLK & CE. Mirrors the DSP's
                // en_iv = clk & (CEP | RSTP) minus the reset term — SRLC32E
                // has no reset port.
                let en_iv = aig.add_and_gate(clk_iv, ce_iv);
                {
                    let sm = aig.macros.get_mut(&cellid).unwrap();
                    sm.inputs_iv[0] = d_iv;
                    sm.en_iv = en_iv;
                }
                // Class C half (only when `Q` is used). Its only boomerang
                // inputs are A[4:0]; the 32 state bits arrive by the zero-copy
                // feedback load, so `feedback_pin` records state bit 0 and the
                // flattener resolves it to that word's global index. Keeping
                // the state out of `inputs_iv` also keeps it out of
                // `for_each_input`, so a state pin nothing else reads stays
                // dead: no boomerang slot, no staged-IO copy across a split.
                if let Some(&read_cid) = srl_read_cid.get(&cellid) {
                    let state0 = aig.macros[&cellid].outputs[0];
                    let rm = aig.macros.get_mut(&read_cid).unwrap();
                    for k in 0..SRL_ADDR_WIDTH {
                        rm.inputs_iv[SRL_READ_ADDR_OFFSET + k] = a_iv[k];
                    }
                    rm.feedback_pin = state0;
                    rm.en_iv = 1;
                }
            }
        }

        // The two synthetic-cell-id spaces must not collide: the CARRY4 fusion
        // starts allocating above every `Srl32Read` id.
        aig.fuse_carry4_chains(netlistdb.num_cells + srl_read_cid.len());

        aig.fanouts_start = vec![0; aig.num_aigpins + 2];
        for (_i, driver) in aig.drivers.iter().enumerate() {
            if let DriverType::AndGate(a, b) = *driver {
                if (a >> 1) != 0 {
                    aig.fanouts_start[a >> 1] += 1;
                }
                if (b >> 1) != 0 {
                    aig.fanouts_start[b >> 1] += 1;
                }
            }
        }
        for i in 1..aig.num_aigpins + 2 {
            aig.fanouts_start[i] += aig.fanouts_start[i - 1];
        }
        aig.fanouts = vec![0; aig.fanouts_start[aig.num_aigpins + 1]];
        for (i, driver) in aig.drivers.iter().enumerate() {
            if let DriverType::AndGate(a, b) = *driver {
                if (a >> 1) != 0 {
                    let st = aig.fanouts_start[a >> 1] - 1;
                    aig.fanouts_start[a >> 1] = st;
                    aig.fanouts[st] = i;
                }
                if (b >> 1) != 0 {
                    let st = aig.fanouts_start[b >> 1] - 1;
                    aig.fanouts_start[b >> 1] = st;
                    aig.fanouts[st] = i;
                }
            }
        }

        // macro endpoint order: Class R first, then Class C.
        let mut macro_order = Vec::with_capacity(aig.macros.len());
        for class in [MacroClass::Registered, MacroClass::Combinational] {
            for (pos, (_, m)) in aig.macros.iter().enumerate() {
                if m.kind.class() == class {
                    macro_order.push(pos);
                }
            }
        }
        aig.macro_order = macro_order;

        aig
    }

    /// Fuse maximal `CO[3] -> CI` CARRY4 cascades into single
    /// `Carry4 { chain_len: k }` macro blocks (`k <= MAX_CARRY4_CHAIN`; longer
    /// cascades split into that many-slice segments joined by one macro level).
    /// Runs at the end of AIG construction, before the fanout CSR is built.
    /// `synth_cid_base` must be above every real netlist cell id.
    fn fuse_carry4_chains(&mut self, synth_cid_base: usize) {
        use crate::hwmacro::{
            carry4_s_index, carry4_di_index, carry4_cyinit_index, carry4_cin_index,
            carry4_o_index, carry4_co_index, MAX_CARRY4_CHAIN,
        };

        let slice_cids: Vec<usize> = self.macros.iter()
            .filter(|(_, m)| matches!(m.kind, MacroKind::Carry4 { chain_len: 1 }))
            .map(|(&c, _)| c)
            .collect();
        if slice_cids.is_empty() {
            return;
        }

        // consumer count per aigpin: decides which CO bits are exposed.
        let mut cc = vec![0usize; self.num_aigpins + 1];
        let bump = |cc: &mut Vec<usize>, iv: usize| {
            if iv >> 1 != 0 {
                cc[iv >> 1] += 1;
            }
        };
        for d in &self.drivers {
            if let DriverType::AndGate(a, b) = *d {
                bump(&mut cc, a);
                bump(&mut cc, b);
            }
        }
        for dff in self.dffs.values() {
            bump(&mut cc, dff.d_iv);
            bump(&mut cc, dff.en_iv);
        }
        for s in self.srams.values() {
            bump(&mut cc, s.port_r_en_iv);
            for i in 0..AIGPDK_SRAM_ADDR_WIDTH {
                bump(&mut cc, s.port_r_addr_iv[i]);
                bump(&mut cc, s.port_w_addr_iv[i]);
            }
            for i in 0..32 {
                bump(&mut cc, s.port_w_wr_en_iv[i]);
                bump(&mut cc, s.port_w_wr_data_iv[i]);
            }
        }
        for m in self.macros.values() {
            bump(&mut cc, m.en_iv);
            for &iv in &m.inputs_iv {
                bump(&mut cc, iv);
            }
        }
        for &po in &self.primary_outputs {
            if po >> 1 != 0 {
                cc[po >> 1] += 1;
            }
        }

        // CO[3] aigpin -> producing slice; successor along CO[3] -> CI.
        let mut co3_producer: IndexMap<usize, usize> = IndexMap::new();
        for &c in &slice_cids {
            let co3 = self.macros[&c].outputs[carry4_co_index(1, 3)];
            if co3 != usize::MAX {
                co3_producer.insert(co3, c);
            }
        }
        let mut succ: IndexMap<usize, usize> = IndexMap::new();
        let mut has_pred: IndexSet<usize> = IndexSet::new();
        for &b in &slice_cids {
            let cin_iv = self.macros[&b].inputs_iv[carry4_cin_index(1)];
            if cin_iv & 1 != 0 || cin_iv >> 1 == 0 {
                continue; // inverted or tied-low carry-in: not a fusable link
            }
            if let Some(&a) = co3_producer.get(&(cin_iv >> 1)) {
                if a != b && !succ.contains_key(&a) && !has_pred.contains(&b) {
                    succ.insert(a, b);
                    has_pred.insert(b);
                }
            }
        }

        let mut fused_index = 0usize;
        let mut to_remove: Vec<usize> = Vec::new();
        let mut to_insert: Vec<(usize, MacroBlock)> = Vec::new();
        for &head in &slice_cids {
            if has_pred.contains(&head) {
                continue;
            }
            let mut chain = vec![head];
            let mut seen: IndexSet<usize> = IndexSet::new();
            seen.insert(head);
            let mut cur = head;
            while let Some(&nx) = succ.get(&cur) {
                if !seen.insert(nx) {
                    break; // guard against a pathological carry loop
                }
                chain.push(nx);
                cur = nx;
            }
            if chain.len() == 1 {
                continue; // a lone CARRY4 stays chain_len 1
            }
            for seg in chain.chunks(MAX_CARRY4_CHAIN) {
                let k = seg.len();
                let synth_cid = synth_cid_base + fused_index;
                fused_index += 1;
                let mut fb = MacroBlock::new(MacroKind::Carry4 { chain_len: k as u16 });
                for (j, &sid) in seg.iter().enumerate() {
                    let sm = &self.macros[&sid];
                    for l in 0..4 {
                        fb.inputs_iv[carry4_s_index(k, 4 * j + l)] =
                            sm.inputs_iv[carry4_s_index(1, l)];
                        fb.inputs_iv[carry4_di_index(k, 4 * j + l)] =
                            sm.inputs_iv[carry4_di_index(1, l)];
                    }
                }
                let head_sm = &self.macros[&seg[0]];
                fb.inputs_iv[carry4_cyinit_index(k)] = head_sm.inputs_iv[carry4_cyinit_index(1)];
                fb.inputs_iv[carry4_cin_index(k)] = head_sm.inputs_iv[carry4_cin_index(1)];
                fb.en_iv = 1;
                for (j, &sid) in seg.iter().enumerate() {
                    let sm = self.macros[&sid].clone();
                    for l in 0..4 {
                        let o_pin = sm.outputs[carry4_o_index(1, l)];
                        let co_pin = sm.outputs[carry4_co_index(1, l)];
                        if o_pin != usize::MAX && cc[o_pin] > 0 {
                            fb.outputs[carry4_o_index(k, 4 * j + l)] = o_pin;
                            self.drivers[o_pin] = DriverType::Macro(synth_cid);
                        }
                        // a CO[3] linking two slices inside this segment carries
                        // one internal consumer (the next CI): expose it only if
                        // something else reads it too.
                        let internal_link = l == 3 && j + 1 < k;
                        if co_pin != usize::MAX && cc[co_pin] > internal_link as usize {
                            fb.outputs[carry4_co_index(k, 4 * j + l)] = co_pin;
                            self.drivers[co_pin] = DriverType::Macro(synth_cid);
                        }
                    }
                }
                to_insert.push((synth_cid, fb));
                to_remove.extend_from_slice(seg);
            }
        }
        for c in to_remove {
            self.macros.shift_remove(&c);
        }
        for (c, b) in to_insert {
            self.macros.insert(c, b);
        }
    }

    pub fn topo_traverse_generic(
        &self,
        endpoints: Option<&Vec<usize>>,
        is_primary_input: Option<&IndexSet<usize>>,
    ) -> Vec<usize> {
        let mut vis = IndexSet::new();
        let mut ret = Vec::new();
        fn dfs_topo(aig: &AIG, vis: &mut IndexSet<usize>, ret: &mut Vec<usize>, is_primary_input: Option<&IndexSet<usize>>, u: usize) {
            if vis.contains(&u) {
                return
            }
            vis.insert(u);
            if let DriverType::AndGate(a, b) = aig.drivers[u] {
                if is_primary_input.map(|s| s.contains(&u)) != Some(true) {
                    if (a >> 1) != 0 {
                        dfs_topo(aig, vis, ret, is_primary_input, a >> 1);
                    }
                    if (b >> 1) != 0 {
                        dfs_topo(aig, vis, ret, is_primary_input, b >> 1);
                    }
                }
            }
            ret.push(u);
        }
        if let Some(endpoints) = endpoints {
            for &endpoint in endpoints {
                dfs_topo(self, &mut vis, &mut ret, is_primary_input, endpoint);
            }
        }
        else {
            for i in 1..self.num_aigpins + 1 {
                dfs_topo(self, &mut vis, &mut ret, is_primary_input, i);
            }
        }
        ret
    }

    pub fn num_endpoint_groups(&self) -> usize {
        self.primary_outputs.len() + self.dffs.len() + self.srams.len()
            + self.macros.len()
    }

    /// Endpoint groups are ordered: primary outputs, DFFs, SRAMs, then
    /// macros (Class R before Class C, via [`AIG::macro_order`]). Macros are
    /// appended last so `.gemparts` files for designs with zero macros keep
    /// the same endpoint indices as before; the Class-R-before-Class-C split
    /// likewise keeps a Part-A design's indices stable.
    pub fn get_endpoint_group(&self, endpt_id: usize) -> EndpointGroup<'_> {
        let n_po = self.primary_outputs.len();
        let n_dff = self.dffs.len();
        let n_sram = self.srams.len();
        if endpt_id < n_po {
            EndpointGroup::PrimaryOutput(*self.primary_outputs.get_index(endpt_id).unwrap())
        }
        else if endpt_id < n_po + n_dff {
            EndpointGroup::DFF(&self.dffs[endpt_id - n_po])
        }
        else if endpt_id < n_po + n_dff + n_sram {
            EndpointGroup::RAMBlock(&self.srams[endpt_id - n_po - n_dff])
        }
        else {
            let local = endpt_id - n_po - n_dff - n_sram;
            EndpointGroup::Macro(self.macros.get_index(self.macro_order[local]).unwrap().1)
        }
    }
}
