// SPDX-FileCopyrightText: Copyright (c) 2024 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//
// GEM_DSP48E2 - the simplified DSP48E2 MAC that GEM evaluates natively as a
// word-level macro (see src/hwmacro.rs / csrc/macro_eval.cuh).
//
// Fixed configuration (matches the challenge spec):
//   * PREG = 1                    - only the P accumulator is registered
//   * AREG/BREG/CREG/DREG/ADREG/MREG = 0  - everything else is combinational
//   * OPMODE is pre-decoded by the frontend into a 2-bit OPMODE_S:
//       0 : P <= C            (bypass)
//       1 : P <= A*B          (multiply-only)
//       2 : P <= P + A*B      (multiply-accumulate)
//   * 27-bit signed pre-adder (AD = USE_D ? A + D : A, wraps at 27 bits)
//   * 45-bit signed product   (M = AD * B)
//   * 48-bit signed accumulator; OVERFLOW / UNDERFLOW are not modelled
//   * RSTP is a synchronous reset of P and has precedence over CEP
//
// This module carries a behavioural body for reference simulation. It is also
// mapping (like $__RAMGEM_SYNC_). `gemmacro_map.v` rewrites Yosys's inferred
// `DSP48E2` cells into this one.

module GEM_DSP48E2 (CLK, CEP, RSTP, USE_D, A, D, B, C, OPMODE_S, P);
  input                 CLK;
  input                 CEP;
  input                 RSTP;
  input                 USE_D;
  input  signed [26:0]  A;
  input  signed [26:0]  D;
  input  signed [17:0]  B;
  input  signed [47:0]  C;
  input         [1:0]   OPMODE_S;
  output reg signed [47:0] P;

`ifndef GEM_DSP48E2_NO_BEHAVIOUR
  wire signed [26:0] ad = USE_D ? (A + D) : A;   // 27-bit wrap, as the real pre-adder
  wire signed [44:0] m  = ad * B;                // 45-bit product
  reg  signed [47:0] p_next;

  always @* begin
    case (OPMODE_S)
      2'd1:    p_next = m;
      2'd2:    p_next = P + m;
      default: p_next = C;
    endcase
    if (RSTP)
      p_next = 48'sd0;
  end

  always @(posedge CLK)
    if (CEP | RSTP)
      P <= p_next;
`endif
endmodule
