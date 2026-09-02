// SPDX-FileCopyrightText: Copyright (c) 2024 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//
// SRLC32E - the Xilinx 32-bit shift-register-LUT primitive that GEM evaluates
// natively as a word-level macro (Part C). See src/hwmacro.rs
// (`eval_srl32_shift` / `eval_srl32_read`) and csrc/macro_eval.cuh
// (`gem_eval_srl32_shift` / `gem_eval_srl32_read`).
//
// The port list is identical to Xilinx's `SRLC32E` (unisim), so a design that
// instantiates the vendor primitive - or one where Yosys's `shregmap`
// infers one - flows straight through. GEM takes the pin directions / widths
// from src/aigpdk.rs.
//
// Semantics (matches the challenge spec, question.md primitive C):
//   on posedge CLK, if CE:  sh <= {sh[30:0], D}    // shift LSB -> MSB
//   Q   = sh[A]                                    // combinational, dynamic A
//   Q31 = sh[31]                                   // combinational cascade
//
// GEM decomposes one SRLC32E into TWO macro endpoints joined by 32 ordinary
// AIG source pins (see src/aig.rs, the `SRLC32E` parse arm):
//
//   * `MacroKind::Srl32Shift` - Class R (registered). Owns the 32-bit state;
//     `en_iv = CLK & CE` gates the commit, so CE == 0 holds exactly like a
//     disabled DFF. `Q31` *is* `state[31]`, so the cascade port costs nothing:
//     no macro, no forced split, no extra grid sync.
//   * `MacroKind::Srl32Read` - Class C (combinational). Created ONLY when `Q`
//     is actually connected. Its inputs are the 32 state source pins plus
//     `A[4:0]`, so the Part B levelization equation
//     `macro_level[M] = max input level` reduces to "the level at which the
//     address is ready".
//
// A pure `Q31 -> D` cascade / delay line (the common FPGA usage) therefore
// creates NO Class C macro and costs ZERO forced major-stage splits.
//
// Marked (* blackbox *) so Yosys / abc keep it as an opaque instance during
// logic mapping (like $__RAMGEM_SYNC_, GEM_DSP48E2 and CARRY4). The
// behavioural body is for reference simulation only.

(* blackbox *)
module SRLC32E (Q, Q31, A, CE, CLK, D);
  output       Q;
  output       Q31;
  input  [4:0] A;
  input        CE;
  input        CLK;
  input        D;

`ifndef GEM_SRLC32E_NO_BEHAVIOUR
  parameter [31:0] INIT = 32'h00000000;
  parameter [0:0]  IS_CLK_INVERTED = 1'b0;

  reg [31:0] sh = INIT;

  always @(posedge CLK)
    if (CE) sh <= {sh[30:0], D};

  assign Q   = sh[A];
  assign Q31 = sh[31];
`endif
endmodule
