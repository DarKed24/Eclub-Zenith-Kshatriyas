// SPDX-FileCopyrightText: Copyright (c) 2024 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Techmap helpers for the SRLC32E shift-register-LUT frontend.
//
//   read_verilog path/to/aigpdk/gemsrl.v
//   # ... your design, synthesised so that SRL primitives appear ...
//   techmap -map path/to/aigpdk/gemsrl_map.v
//   stat
//
// **Checked against Yosys 0.68 (git sha1 38e001a6f).** In this release
// `synth_xilinx` already infers real SRL primitives from an RTL shift
// register, so most flows need only the `SRL16E` rule below:
//
//   * depth 2..16  -> `SRL16E`   (scalar address pins A0..A3)  -> mapped here
//   * depth 18..32 -> `SRLC32E`  (5-bit bus A[4:0])            -> already GEM's
//   * depth > 32   -> a `Q31 -> D` cascade of `SRLC32E`s, which GEM evaluates
//                     with ZERO Class C macros and ZERO forced splits
//
// The `$__XILINX_SHREG_` rule is a fallback for a hand-rolled flow that runs
// `xilinx_srl` (or `shregmap`) WITHOUT Yosys's own
// `techmap -map +/xilinx/cells_map.v`. Its port/parameter list mirrors
// `techlibs/xilinx/cells_map.v`, which is unchanged between 0.33 and 0.68.
//
// As with the DSP and CARRY4 flows, pass names and the exact cells emitted
// drift between Yosys releases: pin the version and validate against the
// installed Yosys as the first frontend task.
//
// Yosys 0.68 PREREQUISITES for the inference path (both scripted in usage.md
// "Step 1.7"): run `synth_xilinx` with `-noiopad -noclkbuf`, and strip the
// cell parameters with `setparam -unset INIT -unset IS_CLK_INVERTED
// t:SRLC32E` (likewise `t:SRL16E`) before `write_verilog`. GEM's netlist
// reader has no instance-parameter support and fails the whole file on `#(`.
//
// FIXED IN 0.68: Yosys 0.33 dropped a non-constant clock enable here -
// `always @(posedge clk) if (ce) sh <= {sh[N-2:0], d};` came out as `SRLC32E`
// with `.CE(1'b1)` and `ce` left dangling. 0.68 preserves it (`.CE(ce)`), and
// also emits the addressed-read form as one `SRLC32E` with a dynamic `A` bus
// and a live `CE`. On 0.33 or earlier, instantiate `SRLC32E` explicitly
// whenever `CE` is not constant.

// --- SRL16E -> SRLC32E ----------------------------------------------------
//
// The 4-bit address is zero-extended to 5 bits. This is exact, not an
// approximation: reading stage `a` of a 16-deep register and stage `a` of a
// 32-deep one return the same bit for every `a` in 0..15 - the extra 16
// stages simply go unread. `SRL16E` has no `Q31` port, so there is no cascade
// output whose depth would change.
module SRL16E (Q, A0, A1, A2, A3, CE, CLK, D);
  output Q;
  input  A0, A1, A2, A3;
  input  CE, CLK, D;

  parameter [15:0] INIT = 16'h0000;
  parameter [0:0]  IS_CLK_INVERTED = 1'b0;

  generate if (IS_CLK_INVERTED != 1'b0)
    $error("GEM: SRL16E with an inverted clock is not supported. GEM models ",
           "one global rising edge; invert the clock outside the primitive.");
  endgenerate

  SRLC32E _TECHMAP_REPLACE_ (
    .Q(Q), .Q31(),
    .A({1'b0, A3, A2, A1, A0}),
    .CE(CE), .CLK(CLK), .D(D)
  );
endmodule

// --- Yosys's pre-techmap shift-register cell -> SRLC32E --------------------
//
// `$__XILINX_SHREG_` as defined by Yosys 0.68's xilinx/cells_map.v:
//   ports  C, D, L[31:0], E, Q, SO
//   params DEPTH, INIT, CLKPOL, ENPOL   (ENPOL: 0 = active low, 1 = active
//                                        high, 2 = no enable)
// `L` is the (possibly dynamic) tap index; for a fixed-length shift register
// Yosys drives it with the constant `DEPTH-1`, which is exactly SRLC32E's
// `A`. `SO` is the cascade shift-out, i.e. `Q31`.
//
// Only the rising-edge, DEPTH <= 32 form maps onto a single SRLC32E; anything
// else raises a synthesis `$error` telling you what to change rather than
// silently mis-modelling the delay.
module \$__XILINX_SHREG_ (C, D, L, E, Q, SO);
  parameter DEPTH = 0;
  parameter [DEPTH-1:0] INIT = 0;
  parameter CLKPOL = 1;
  parameter ENPOL = 2;

  input         C;
  input         D;
  input  [31:0] L;
  input         E;
  output        Q;
  output        SO;

  generate if (CLKPOL != 1)
    $error("GEM: $__XILINX_SHREG_ with a falling-edge clock is not ",
           "supported - GEM models one global rising edge.");
  endgenerate

  generate if (ENPOL == 0)
    $error("GEM: $__XILINX_SHREG_ with an active-low enable is not ",
           "supported - invert the enable outside the primitive.");
  endgenerate

  generate if (DEPTH > 32)
    $error("GEM: $__XILINX_SHREG_ deeper than 32 needs an explicit ",
           "Q31 -> D cascade of SRLC32Es. Re-run the SRL inference pass ",
           "with a maximum length of 32.");
  endgenerate

  wire ce = (ENPOL == 2) ? 1'b1 : E;

  // `SO` is left unconnected unless the caller cascades: GEM charges nothing
  // for `Q31`, so exposing it is free.
  SRLC32E _TECHMAP_REPLACE_ (
    .Q(Q), .Q31(SO),
    .A(L[4:0]),
    .CE(ce), .CLK(C), .D(D)
  );
endmodule
