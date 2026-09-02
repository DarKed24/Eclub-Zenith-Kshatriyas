// SPDX-FileCopyrightText: Copyright (c) 2024 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Techmap: Xilinx `DSP48E2` -> `GEM_DSP48E2`.
//
// Run this after Yosys has produced real `DSP48E2` cells (whether from
// `synth_xilinx`/`xilinx_dsp` inference or a hand-instantiated primitive):
//
//   read_verilog path/to/aigpdk/gemmacro.v
//   ...
//   techmap -map path/to/aigpdk/gemmacro_map.v
//
// Only the fixed configuration GEM evaluates natively is accepted:
//   * PREG = 1                              (registered accumulator)
//   * AREG/BREG/CREG/DREG/ADREG/MREG = 0    (everything else combinational)
//   * USE_MULT != "NONE"                    (multiplier enabled)
// Anything else raises a synthesis `$error` telling you what to change.
//
// The port/parameter list below mirrors `\`yosys-config --datdir\`/xilinx/
// cells_xtra.v` from Yosys 0.68 (git sha1 38e001a6f); it is identical to the
// 0.33 list, so this file did not change across that version bump. Validated
// against 0.68: `techmap` of a PREG=1 DSP48E2 MAC yields a `GEM_DSP48E2`
// instance with the expected constant (`OPMODE 9'h025 -> 2`, `9'h005 -> 1`) or
// dynamic `OPMODE_S`, and PREG=0 / non-zero AREG each raise the intended
// `$error`. Newer Yosys releases occasionally add DSP48E2 ports/params -
// re-check `help DSP48E2` if you bump the version.
//
// NOTE: Yosys's `xilinx_dsp` does *not* fold a plain `P <= P + A*B`
// accumulator into PREG + the post-adder - it leaves PREG=0 and the adder in
// fabric, which this map then rejects. Confirmed on both 0.33 and 0.68, for
// `p <= p + a*b`, `p <= c + a*b` and even a plain registered `p <= a*b`, via
// both the manual `map_dsp` sequence and `synth_xilinx -family {xcup,xcu,xc7}`.
// See usage.md "Step 1.5" for the flows that do produce a PREG=1 DSP48E2.

module DSP48E2 (
  input  [29:0] A,
  input  [29:0] ACIN,
  input  [3:0]  ALUMODE,
  input  [17:0] B,
  input  [17:0] BCIN,
  input  [47:0] C,
  input         CARRYCASCIN,
  input         CARRYIN,
  input  [2:0]  CARRYINSEL,
  input         CEA1, CEA2, CEAD, CEALUMODE, CEB1, CEB2, CEC, CECARRYIN,
  input         CECTRL, CED, CEINMODE, CEM, CEP,
  input         CLK,
  input  [26:0] D,
  input  [4:0]  INMODE,
  input         MULTSIGNIN,
  input  [8:0]  OPMODE,
  input  [47:0] PCIN,
  input         RSTA, RSTALLCARRYIN, RSTALUMODE, RSTB, RSTC, RSTCTRL, RSTD,
  input         RSTINMODE, RSTM, RSTP,
  output [29:0] ACOUT,
  output [17:0] BCOUT,
  output        CARRYCASCOUT,
  output [3:0]  CARRYOUT,
  output        MULTSIGNOUT,
  output        OVERFLOW,
  output [47:0] P,
  output        PATTERNBDETECT,
  output        PATTERNDETECT,
  output [47:0] PCOUT,
  output        UNDERFLOW,
  output [7:0]  XOROUT
);
  // Register-control parameters. Defaults are the *accepted* configuration so
  // that reading this map file (which elaborates it once with defaults) does
  // not trip the `$error`s below; techmap re-elaborates with the real instance
  // parameters and the checks then fire for a genuinely unsupported cell.
  parameter integer ACASCREG      = 0;
  parameter integer ADREG         = 0;
  parameter integer ALUMODEREG    = 0;
  parameter integer AREG          = 0;
  parameter integer BCASCREG      = 0;
  parameter integer BREG          = 0;
  parameter integer CARRYINREG    = 0;
  parameter integer CARRYINSELREG = 0;
  parameter integer CREG          = 0;
  parameter integer DREG          = 0;
  parameter integer INMODEREG     = 0;
  parameter integer MREG          = 0;
  parameter integer OPMODEREG     = 0;
  parameter integer PREG          = 1;

  parameter AMULTSEL   = "A";
  parameter BMULTSEL   = "B";
  parameter PREADDINSEL = "A";
  parameter A_INPUT    = "DIRECT";
  parameter B_INPUT    = "DIRECT";
  parameter USE_MULT   = "MULTIPLY";
  parameter USE_SIMD   = "ONE48";
  parameter USE_WIDEXOR = "FALSE";
  parameter USE_PATTERN_DETECT = "NO_PATDET";
  parameter XORSIMD    = "XOR24_48_96";
  parameter AUTORESET_PATDET  = "NO_RESET";
  parameter AUTORESET_PRIORITY = "RESET";
  parameter SEL_MASK    = "MASK";
  parameter SEL_PATTERN = "PATTERN";
  parameter [47:0] MASK    = 48'h3FFFFFFFFFFF;
  parameter [47:0] PATTERN = 48'h000000000000;
  parameter [47:0] RND     = 48'h000000000000;
  parameter [3:0]  IS_ALUMODE_INVERTED = 4'b0000;
  parameter [0:0]  IS_CARRYIN_INVERTED = 1'b0;
  parameter [0:0]  IS_CLK_INVERTED     = 1'b0;
  parameter [4:0]  IS_INMODE_INVERTED  = 5'b00000;
  parameter [8:0]  IS_OPMODE_INVERTED  = 9'b000000000;
  parameter [0:0]  IS_RSTALLCARRYIN_INVERTED = 1'b0;
  parameter [0:0]  IS_RSTALUMODE_INVERTED    = 1'b0;
  parameter [0:0]  IS_RSTA_INVERTED    = 1'b0;
  parameter [0:0]  IS_RSTB_INVERTED    = 1'b0;
  parameter [0:0]  IS_RSTCTRL_INVERTED = 1'b0;
  parameter [0:0]  IS_RSTC_INVERTED    = 1'b0;
  parameter [0:0]  IS_RSTD_INVERTED    = 1'b0;
  parameter [0:0]  IS_RSTINMODE_INVERTED = 1'b0;
  parameter [0:0]  IS_RSTM_INVERTED    = 1'b0;
  parameter [0:0]  IS_RSTP_INVERTED    = 1'b0;

  // --- mandated configuration checks ------------------------------------
  // PREG must be 1: a registered accumulator. With PREG=0 the multiply is
  // purely combinational and the macro would be Class C, out of scope here.
  generate if (PREG != 1)
    $error("GEM: DSP48E2 has PREG=0 - GEM only maps a *registered* MAC. ",
           "Add a pipeline register on the accumulator (P) in your RTL, or ",
           "check that xilinx_dsp folded the output flop into the DSP.");
  endgenerate

  // Input / pipeline registers are not modelled inside the macro (a later
  // revision could unpack them into explicit aigpdk DFFs in front of it).
  generate if (AREG != 0 || BREG != 0 || DREG != 0 || ADREG != 0 ||
               CREG != 0 || MREG != 0)
    $error("GEM: DSP48E2 has a non-zero AREG/BREG/CREG/DREG/ADREG/MREG. ",
           "Part A needs every register except PREG to be 0 (combinational ",
           "inputs). Move the pipelining outside the MAC.");
  endgenerate

  generate if (USE_MULT != "MULTIPLY" && USE_MULT != "DYNAMIC")
    $error("GEM: DSP48E2 USE_MULT must keep the multiplier enabled.");
  endgenerate

  // --- OPMODE -> 2-bit OPMODE_S ----------------------------------------
  // Written as plain combinational logic: a *static* OPMODE folds to a
  // constant OPMODE_S, a *dynamic* one synthesises into AIG gates that feed
  // the macro's OPMODE_S input.
  //   X mux = OPMODE[1:0], Y mux = OPMODE[3:2], Z mux = OPMODE[6:4]
  //   multiplier product M is selected when X=01 and Y=01
  wire [8:0] opm  = OPMODE ^ IS_OPMODE_INVERTED;
  wire [2:0] zmux = opm[6:4];
  wire       mmux = (opm[1:0] == 2'b01) && (opm[3:2] == 2'b01);
  wire [1:0] opmode_s =
       (mmux && zmux == 3'b010) ? 2'd2      // Z=P  -> P <= P + A*B
     : (mmux && zmux == 3'b000) ? 2'd1      // Z=0  -> P <= A*B
     :                            2'd0;     // else -> P <= C (bypass/load)

  // Pre-adder is engaged when the A-path multiplier operand is the A+D sum.
  wire use_d = (AMULTSEL == "AD") || (BMULTSEL == "AD") ||
               (PREADDINSEL == "AD");

  GEM_DSP48E2 _TECHMAP_REPLACE_ (
    .CLK      (CLK),
    .CEP      (CEP),
    .RSTP     (RSTP ^ IS_RSTP_INVERTED),
    .USE_D    (use_d),
    .A        (A[26:0]),
    .D        (D[26:0]),
    .B        (B[17:0]),
    .C        (C[47:0]),
    .OPMODE_S (opmode_s),
    .P        (P)
  );
endmodule
