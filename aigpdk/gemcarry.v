// SPDX-FileCopyrightText: Copyright (c) 2024 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//
// CARRY4 - the Xilinx 7-series carry-chain primitive that GEM evaluates
// natively as a Class C (combinational) word-level macro. See
// src/hwmacro.rs (`eval_carry4`) / csrc/macro_eval.cuh (`gem_eval_carry4`).
//
// The port list is identical to Xilinx's `CARRY4` (unisim), so a design that
// instantiates the vendor primitive - or one where Yosys's `arith_map.v`
// infers `CARRY4` from `$alu` - flows straight through. GEM takes the pin
// directions / widths from src/aigpdk.rs.
//
// Semantics (matches the challenge spec, question.md primitive B):
//   C[0]   = CYINIT | CI        (exactly one is driven in valid RTL)
//   C[i+1] = S[i] ? C[i] : DI[i]
//   O[i]   = S[i] ^ C[i]
//   CO[i]  = C[i+1]
//
// A maximal `CO[3] -> CI` cascade of these slices is fused by GEM's AIG
// frontend (src/aig.rs `fuse_carry4_chains`) into one
// `MacroKind::Carry4 { chain_len }` endpoint whose native datapath ripples
// all 4*chain_len carries in a fixed parallel prefix.
//
// Marked (* blackbox *) so Yosys / abc keep it as an opaque instance during
// logic mapping (like $__RAMGEM_SYNC_ and GEM_DSP48E2). The behavioural body
// is for reference simulation only.

(* blackbox *)
module CARRY4 (CO, O, CI, CYINIT, DI, S);
  output [3:0] CO;
  output [3:0] O;
  input        CI;
  input        CYINIT;
  input  [3:0] DI;
  input  [3:0] S;

`ifndef GEM_CARRY4_NO_BEHAVIOUR
  wire c0 = CI | CYINIT;
  wire c1 = S[0] ? c0 : DI[0];
  wire c2 = S[1] ? c1 : DI[1];
  wire c3 = S[2] ? c2 : DI[2];
  wire c4 = S[3] ? c3 : DI[3];

  assign O  = S ^ {c3, c2, c1, c0};
  assign CO = {c4, c3, c2, c1};
`endif
endmodule
