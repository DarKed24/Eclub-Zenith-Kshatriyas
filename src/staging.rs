// SPDX-FileCopyrightText: Copyright (c) 2024 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//! Splitting deep circuit into major stages at global level indices.
//!
//! This is crucial in efficiently handling large and deep circuits
//! with a limited processing element width.
//!
//! # The boomerang scheduler extension for Class C macros (Part B)
//!
//! A Class R macro (DSP48E2) is a clean cut at the cycle boundary: its outputs
//! are registered, so nothing in the scheduler had to change. A **Class C**
//! macro (CARRY4) is combinational — its outputs must reach downstream AIG
//! logic *within the same simulated cycle* — but GEM evaluates macros in the
//! partition epilogue, after the boomerang. So a Class C macro is modelled as a
//! **forced major-stage split at the macro's level**: it is realized (its
//! inputs boomerang-computed, the macro evaluated) in stage *k*, its outputs
//! land in the staged-IO scratch region, and stage *k+1* reads them back via
//! the existing "current-iteration" global-read path.
//!
//! The levelized-DAG equations gain, for a Class C macro `M`:
//! ```text
//! macro_level[M] = max over b in in(M) of level_id[b]   # last input ready
//! level_id[o]    = macro_level[M] + 1   for o in out(M) # the native eval step
//! ```
//! [`macro_levels`] computes `macro_level` with a small fixpoint (a Class C
//! macro can feed another). The distinct `macro_level` values become forced
//! split points, unioned with any user `--level-split`.

use indexmap::{IndexMap, IndexSet};
use crate::aig::{AIG, EndpointGroup, DriverType};
use crate::hwmacro::MacroClass;

/// A struct representing the boundaries of a staged AIG.
pub struct StagedAIG {
    /// the staged primary inputs from previous levels.
    pub primary_inputs: Option<IndexSet<usize>>,
    /// the staged primary output pins for next levels.
    ///
    /// these pins are active nodes at the level split.
    pub primary_output_pins: Vec<usize>,
    /// the endpoint indices of original AIG fulfilled by current level.
    pub endpoints: Vec<usize>,
}

impl StagedAIG {
    /// Get the number of endpoint groups that should be fulfilled
    /// with this staged AIG.
    ///
    /// This mimics the interface given by a raw AIG.
    pub fn num_endpoint_groups(&self) -> usize {
        self.primary_output_pins.len() + self.endpoints.len()
    }

    /// Get the virtual endpoint group with an index.
    ///
    /// This mimics the interface given by a raw AIG.
    pub fn get_endpoint_group<'aig>(&self, aig: &'aig AIG, endpt_id: usize) -> EndpointGroup<'aig> {
        if endpt_id < self.primary_output_pins.len() {
            EndpointGroup::StagedIOPin(self.primary_output_pins[endpt_id])
        }
        else {
            aig.get_endpoint_group(self.endpoints[endpt_id - self.primary_output_pins.len()])
        }
    }

    /// build a staged AIG that consists of all levels.
    pub fn from_full_aig(aig: &AIG) -> Self {
        StagedAIG {
            primary_inputs: None,
            primary_output_pins: vec![],
            endpoints: (0..aig.num_endpoint_groups()).collect()
        }
    }

    /// build a staged AIG by horizontal splitting given a subset
    /// of endpoints.
    ///
    /// return built StagedAIG.
    /// the endpoints are given as a slice of endpoint group indices,
    /// that must have all staged primary output groups at the front
    /// and original endpoints following. otherwise we panic.
    ///
    /// the result guarantees that the endpoint `i` corresponds to
    /// the original staged's endpoint `endpoint_subset[i]`.
    pub fn to_endpoint_subset(
        &self,
        endpoint_subset: &[usize]
    ) -> StagedAIG {
        let mut staged_sub = StagedAIG {
            primary_inputs: self.primary_inputs.clone(),
            primary_output_pins: vec![],
            endpoints: vec![],
        };
        for &endpt_i in endpoint_subset {
            if endpt_i < self.primary_output_pins.len() {
                staged_sub.primary_output_pins.push(
                    self.primary_output_pins[endpt_i]
                );
                assert!(staged_sub.endpoints.is_empty(),
                        "endpoint subset must be in order!");
            }
            else {
                staged_sub.endpoints.push(
                    self.endpoints[endpt_i - self.primary_output_pins.len()]
                );
            }
        }
        staged_sub
    }

    /// build a staged AIG by vertical splitting at the given level id.
    ///
    /// return built StagedAIG.
    /// the active middle nodes at split can be obtained from the
    /// StagedAIG::primary_output_pins.
    /// if this is empty, it means all endpoints are already satisfied
    /// after this stage.
    ///
    /// `classc_output_pins` is the set of every Class C macro output aigpin;
    /// `classc_levels` maps a Class C macro endpoint index to its absolute
    /// `macro_level`; `cur_split_abs` is this stage's absolute split. When
    /// there are no Class C macros these are empty / `usize::MAX` and the
    /// behaviour is identical to before Part B.
    pub fn from_split(
        aig: &AIG,
        unrealized_orig_endpoints: &IndexSet<usize>,
        primary_inputs: Option<&IndexSet<usize>>,
        split_at_level: usize,
        classc_output_pins: &IndexSet<usize>,
        classc_levels: &IndexMap<usize, usize>,
        cur_split_abs: usize,
    ) -> Self {
        let mut unrealized_endpoint_nodes = Vec::new();
        for &endpt in unrealized_orig_endpoints {
            aig.get_endpoint_group(endpt).for_each_input(|i| {
                unrealized_endpoint_nodes.push(i);
            });
        }
        assert!(!unrealized_endpoint_nodes.is_empty());
        let order = aig.topo_traverse_generic(
            Some(&unrealized_endpoint_nodes),
            primary_inputs
        );
        let mut num_fanouts = vec![0; aig.num_aigpins + 1];
        let mut level_id = vec![0; aig.num_aigpins + 1];

        // Class C macro outputs whose macro is not realized in an earlier
        // stage (i.e. not already a primary input) are seeded above this
        // stage's split so every downstream consumer is deferred past it — the
        // macro is evaluated in the epilogue and its result is only visible in
        // the next major stage.
        let is_pi = |i: &usize| matches!(primary_inputs, Some(pi) if pi.contains(i));
        let defer_seed = if split_at_level == usize::MAX {
            aig.num_aigpins + 1
        } else {
            split_at_level + 1
        };
        for &o in classc_output_pins {
            if o <= aig.num_aigpins && !is_pi(&o) {
                level_id[o] = defer_seed;
            }
        }

        for &i in &order {
            if is_pi(&i) {
                continue
            }
            if let DriverType::AndGate(a, b) = aig.drivers[i] {
                if a >= 2 {
                    num_fanouts[a >> 1] += 1;
                    level_id[i] = level_id[i].max(level_id[a >> 1] + 1);
                }
                if b >= 2 {
                    num_fanouts[b >> 1] += 1;
                    level_id[i] = level_id[i].max(level_id[b >> 1] + 1);
                }
            }
        }
        let mut endpt_level_id = vec![0; aig.num_endpoint_groups()];
        for &endpt in unrealized_orig_endpoints {
            aig.get_endpoint_group(endpt).for_each_input(|i| {
                num_fanouts[i] += 1;
                endpt_level_id[endpt] = endpt_level_id[endpt].max(level_id[i]);
            });
        }
        let mut nodes_at_split = IndexSet::new();
        for &i in &order {
            if level_id[i] > split_at_level { continue }
            nodes_at_split.insert(i);
            if is_pi(&i) {
                continue
            }
            if let DriverType::AndGate(a, b) = aig.drivers[i] {
                if a >= 2 {
                    num_fanouts[a >> 1] -= 1;
                    if num_fanouts[a >> 1] == 0 {
                        assert!(nodes_at_split.swap_remove(&(a >> 1)));
                    }
                }
                if b >= 2 {
                    num_fanouts[b >> 1] -= 1;
                    if num_fanouts[b >> 1] == 0 {
                        assert!(nodes_at_split.swap_remove(&(b >> 1)));
                    }
                }
            }
        }
        let mut endpoints_before_split = Vec::new();
        let mut realized: IndexSet<usize> = IndexSet::new();
        let realize_endpt = |endpt: usize,
                                 endpoints_before_split: &mut Vec<usize>,
                                 realized: &mut IndexSet<usize>,
                                 num_fanouts: &mut Vec<usize>,
                                 nodes_at_split: &mut IndexSet<usize>| {
            if !realized.insert(endpt) { return }
            endpoints_before_split.push(endpt);
            aig.get_endpoint_group(endpt).for_each_input(|i| {
                if num_fanouts[i] > 0 {
                    num_fanouts[i] -= 1;
                    if num_fanouts[i] == 0 {
                        nodes_at_split.swap_remove(&i);
                    }
                }
            });
        };
        for &endpt in unrealized_orig_endpoints {
            if endpt_level_id[endpt] > split_at_level { continue }
            realize_endpt(endpt, &mut endpoints_before_split, &mut realized,
                          &mut num_fanouts, &mut nodes_at_split);
        }
        // Force-realize any Class C macro whose absolute macro_level is due by
        // this split but whose stage-relative level (which can be compressed
        // relative to the global one across earlier stages) left it behind.
        // Its inputs are guaranteed present: an unrealized endpoint keeps its
        // input nodes alive at every split until it is realized.
        for &endpt in unrealized_orig_endpoints {
            if let Some(&abs) = classc_levels.get(&endpt) {
                if abs <= cur_split_abs {
                    realize_endpt(endpt, &mut endpoints_before_split, &mut realized,
                                  &mut num_fanouts, &mut nodes_at_split);
                }
            }
        }

        StagedAIG {
            primary_inputs: primary_inputs.cloned(),
            primary_output_pins: nodes_at_split.iter().copied()
                .filter(|po| !is_pi(po) && !classc_output_pins.contains(po))
                .collect(),
            endpoints: endpoints_before_split
        }
    }
}

/// Absolute levelized-DAG level of every Class C macro (by endpoint index),
/// plus the set of every Class C macro output aigpin.
///
/// `level_id[o] = macro_level[M] + 1` for a Class C macro output feeds back
/// into `macro_level` of any macro downstream, so this is a small bounded
/// fixpoint (one iteration when every carry chain is fused, the common case).
pub fn macro_levels(aig: &AIG) -> (IndexMap<usize, usize>, IndexSet<usize>) {
    let n_reg = aig.primary_outputs.len() + aig.dffs.len() + aig.srams.len();
    // (endpoint index, input aigpins, output aigpins) for each Class C macro.
    let mut classc: Vec<(usize, Vec<usize>, Vec<usize>)> = Vec::new();
    let mut output_pins: IndexSet<usize> = IndexSet::new();
    for endpt in n_reg..aig.num_endpoint_groups() {
        if let EndpointGroup::Macro(m) = aig.get_endpoint_group(endpt) {
            if m.kind.class() != MacroClass::Combinational { continue }
            let ins: Vec<usize> = m.inputs_iv.iter()
                .map(|&iv| iv >> 1).filter(|&i| i != 0).collect();
            let outs: Vec<usize> = m.outputs.iter().copied()
                .filter(|&o| o != usize::MAX).collect();
            for &o in &outs { output_pins.insert(o); }
            classc.push((endpt, ins, outs));
        }
    }
    let mut levels = IndexMap::new();
    if classc.is_empty() {
        return (levels, output_pins);
    }

    let mut macro_out_level = vec![0usize; aig.num_aigpins + 1];
    for _iter in 0..(classc.len() + 2) {
        let mut level_id = vec![0usize; aig.num_aigpins + 1];
        for &o in &output_pins {
            level_id[o] = macro_out_level[o];
        }
        // AIG pins are in topological order by construction.
        for i in 1..=aig.num_aigpins {
            if let DriverType::AndGate(a, b) = aig.drivers[i] {
                let mut lv = 0;
                if a >= 2 { lv = lv.max(level_id[a >> 1] + 1); }
                if b >= 2 { lv = lv.max(level_id[b >> 1] + 1); }
                level_id[i] = level_id[i].max(lv);
            }
        }
        let mut changed = false;
        for (endpt, ins, outs) in &classc {
            let ml = ins.iter().map(|&i| level_id[i]).max().unwrap_or(0);
            levels.insert(*endpt, ml);
            for &o in outs {
                if macro_out_level[o] != ml + 1 {
                    macro_out_level[o] = ml + 1;
                    changed = true;
                }
            }
        }
        if !changed { break }
    }
    (levels, output_pins)
}

/// Given the level split points, return a list of split stages.
///
/// For example, given [10, 20], will return a list like this:
/// [(0, 10, stage0_10), (10, 20, stage10_20), (20, MAX, stage20_MAX)]
///
/// If the netlist ends early before all split points, the length might be
/// shorter than expected.
///
/// The distinct `macro_level` of every Class C macro is auto-unioned into the
/// split set (a forced major-stage split per distinct carry-chain depth), so a
/// design with Class C macros is split even when `user_level_split` is empty.
/// A design with none is unaffected.
pub fn build_staged_aigs(
    aig: &AIG, user_level_split: &[usize]
) -> Vec<(usize, usize, StagedAIG)> {
    let (classc_levels, classc_output_pins) = macro_levels(aig);

    let mut level_split: Vec<usize> = classc_levels.values().copied().collect();
    level_split.extend_from_slice(user_level_split);
    level_split.sort_unstable();
    level_split.dedup();
    // A split at absolute level 0 is meaningful for a Class C macro whose
    // inputs are all primary: it defers every consumer to the next stage.
    // `usize::MAX` is the implicit final stage.
    level_split.retain(|&s| s != usize::MAX);

    let mut ret = Vec::new();
    let mut unrealized_orig_endpoints = (0..aig.num_endpoint_groups()).collect::<IndexSet<_>>();
    let mut primary_inputs: Option<IndexSet<usize>> = None;

    // add the output pins of the Class C macros a stage realized to the
    // primary-input set carried forward.
    let carry_classc_outputs =
        |staged: &StagedAIG, primary_inputs: &mut IndexSet<usize>| {
            for &endpt in &staged.endpoints {
                if let EndpointGroup::Macro(m) = aig.get_endpoint_group(endpt) {
                    if m.kind.class() == MacroClass::Combinational {
                        for &o in &m.outputs {
                            if o != usize::MAX {
                                primary_inputs.insert(o);
                            }
                        }
                    }
                }
            }
        };

    for i in 0..level_split.len() {
        let cur_split = level_split[i];
        let last_split = if i == 0 { 0 } else { level_split[i - 1] };
        let staged = StagedAIG::from_split(
            aig, &unrealized_orig_endpoints, primary_inputs.as_ref(),
            cur_split - last_split,
            &classc_output_pins, &classc_levels, cur_split,
        );
        for &endpt in &staged.endpoints {
            assert!(unrealized_orig_endpoints.swap_remove(&endpt));
        }
        let pi = primary_inputs.get_or_insert_with(Default::default);
        for &inp in &staged.primary_output_pins {
            pi.insert(inp);
        }
        carry_classc_outputs(&staged, pi);

        if unrealized_orig_endpoints.is_empty() {
            ret.push((last_split, usize::MAX, staged));
            return ret
        }
        ret.push((last_split, cur_split, staged));
    }

    let last_split = level_split.last().copied().unwrap_or(0);
    ret.push((last_split, usize::MAX, StagedAIG::from_split(
        aig, &unrealized_orig_endpoints, primary_inputs.as_ref(),
        usize::MAX,
        &classc_output_pins, &classc_levels, usize::MAX,
    )));

    ret
}
