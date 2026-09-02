// SPDX-FileCopyrightText: Copyright (c) 2024 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Techmap helpers for the CARRY4 carry-chain frontend.
//
//   read_verilog path/to/aigpdk/gemcarry.v
//   ...
//   synth -flatten -run begin:fine
//   # $alu / $add / $sub -> Xilinx carry chains (produces CARRY4 directly):
//   techmap -map +/xilinx/arith_map.v -D LUT_SIZE=6
//   # UltraScale CARRY8 -> two CARRY4 (if any survived):
//   techmap -map path/to/aigpdk/gemcarry_map.v
//   synth -run fine:
//
// Yosys's `arith_map.v` emits `CARRY4` with exactly the port list of
// `aigpdk/gemcarry.v` for any `LUT_SIZE != 4` target, so such a flow needs no
// mapping here - this file only splits an UltraScale `CARRY8` into two chained
// `CARRY4`.
//
// **Checked against Yosys 0.68 (git sha1 38e001a6f).** The recipe above takes
// a 16-bit `sum <= a + b + cin` to 4 `CARRY4` plus aigpdk glue, end to end.
// Note that 0.68 never actually emits `CARRY8` - `synth_xilinx -family xcup`
// also produces `CARRY4` - so the `CARRY8` rule below elaborates but has not
// been exercised against a real design.
//
// As with the DSP flow, pass / map-file names drift between Yosys releases:
// pin the version and validate that `arith_map.v` produces `CARRY4` (not a
// `$lut` carry) against the installed Yosys as the first frontend task. Get the
// `synth -run` labels right (`begin:fine`, then `fine:`) - a wrong label pair
// is not an error, it just runs the generic flow and shreds the carry chain
// into AIG gates, so always `stat` after the `arith_map` techmap.

module CARRY8 (CO, O, CI, CI_TOP, DI, S);
  output [7:0] CO;
  output [7:0] O;
  input        CI;
  input        CI_TOP;   // tied to CI for a single 8-bit chain
  input  [7:0] DI;
  input  [7:0] S;

  wire co3;

  CARRY4 lo (
    .CO(CO[3:0]), .O(O[3:0]),
    .CI(CI), .CYINIT(1'b0),
    .DI(DI[3:0]), .S(S[3:0])
  );
  assign co3 = CO[3];
  CARRY4 hi (
    .CO(CO[7:4]), .O(O[7:4]),
    .CI(co3), .CYINIT(1'b0),
    .DI(DI[7:4]), .S(S[7:4])
  );
endmodule
