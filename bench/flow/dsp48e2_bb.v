// DSP48E2 blackbox declaration for the slang (SystemVerilog) frontend.
// Port/parameter list extracted verbatim from Yosys's xilinx/cells_xtra.v;
// the (* blackbox *) attribute is what keeps sv-elab from flattening the
// (empty) module away. Mapped to GEM_DSP48E2 by gemmacro_map.v.
(* blackbox *)
module DSP48E2(ACOUT, BCOUT, CARRYCASCOUT, CARRYOUT, MULTSIGNOUT, OVERFLOW, P, PATTERNBDETECT, PATTERNDETECT, PCOUT, UNDERFLOW, XOROUT, A, ACIN, ALUMODE, B, BCIN, C, CARRYCASCIN, CARRYIN, CARRYINSEL
, CEA1, CEA2, CEAD, CEALUMODE, CEB1, CEB2, CEC, CECARRYIN, CECTRL, CED, CEINMODE, CEM, CEP, CLK, D, INMODE, MULTSIGNIN, OPMODE, PCIN, RSTA, RSTALLCARRYIN
, RSTALUMODE, RSTB, RSTC, RSTCTRL, RSTD, RSTINMODE, RSTM, RSTP);
    parameter integer ACASCREG = 1;
    parameter integer ADREG = 1;
    parameter integer ALUMODEREG = 1;
    parameter AMULTSEL = "A";
    parameter integer AREG = 1;
    parameter AUTORESET_PATDET = "NO_RESET";
    parameter AUTORESET_PRIORITY = "RESET";
    parameter A_INPUT = "DIRECT";
    parameter integer BCASCREG = 1;
    parameter BMULTSEL = "B";
    parameter integer BREG = 1;
    parameter B_INPUT = "DIRECT";
    parameter integer CARRYINREG = 1;
    parameter integer CARRYINSELREG = 1;
    parameter integer CREG = 1;
    parameter integer DREG = 1;
    parameter integer INMODEREG = 1;
    parameter [3:0] IS_ALUMODE_INVERTED = 4'b0000;
    parameter [0:0] IS_CARRYIN_INVERTED = 1'b0;
    parameter [0:0] IS_CLK_INVERTED = 1'b0;
    parameter [4:0] IS_INMODE_INVERTED = 5'b00000;
    parameter [8:0] IS_OPMODE_INVERTED = 9'b000000000;
    parameter [0:0] IS_RSTALLCARRYIN_INVERTED = 1'b0;
    parameter [0:0] IS_RSTALUMODE_INVERTED = 1'b0;
    parameter [0:0] IS_RSTA_INVERTED = 1'b0;
    parameter [0:0] IS_RSTB_INVERTED = 1'b0;
    parameter [0:0] IS_RSTCTRL_INVERTED = 1'b0;
    parameter [0:0] IS_RSTC_INVERTED = 1'b0;
    parameter [0:0] IS_RSTD_INVERTED = 1'b0;
    parameter [0:0] IS_RSTINMODE_INVERTED = 1'b0;
    parameter [0:0] IS_RSTM_INVERTED = 1'b0;
    parameter [0:0] IS_RSTP_INVERTED = 1'b0;
    parameter [47:0] MASK = 48'h3FFFFFFFFFFF;
    parameter integer MREG = 1;
    parameter integer OPMODEREG = 1;
    parameter [47:0] PATTERN = 48'h000000000000;
    parameter PREADDINSEL = "A";
    parameter integer PREG = 1;
    parameter [47:0] RND = 48'h000000000000;
    parameter SEL_MASK = "MASK";
    parameter SEL_PATTERN = "PATTERN";
    parameter USE_MULT = "MULTIPLY";
    parameter USE_PATTERN_DETECT = "NO_PATDET";
    parameter USE_SIMD = "ONE48";
    parameter USE_WIDEXOR = "FALSE";
    parameter XORSIMD = "XOR24_48_96";
    output [29:0] ACOUT;
    output [17:0] BCOUT;
    output CARRYCASCOUT;
    output [3:0] CARRYOUT;
    output MULTSIGNOUT;
    output OVERFLOW;
    output [47:0] P;
    output PATTERNBDETECT;
    output PATTERNDETECT;
    output [47:0] PCOUT;
    output UNDERFLOW;
    output [7:0] XOROUT;
    input [29:0] A;
    input [29:0] ACIN;
    (* invertible_pin = "IS_ALUMODE_INVERTED" *)
    input [3:0] ALUMODE;
    input [17:0] B;
    input [17:0] BCIN;
    input [47:0] C;
    input CARRYCASCIN;
    (* invertible_pin = "IS_CARRYIN_INVERTED" *)
    input CARRYIN;
    input [2:0] CARRYINSEL;
    input CEA1;
    input CEA2;
    input CEAD;
    input CEALUMODE;
    input CEB1;
    input CEB2;
    input CEC;
    input CECARRYIN;
    input CECTRL;
    input CED;
    input CEINMODE;
    input CEM;
    input CEP;
    (* clkbuf_sink *)
    (* invertible_pin = "IS_CLK_INVERTED" *)
    input CLK;
    input [26:0] D;
    (* invertible_pin = "IS_INMODE_INVERTED" *)
    input [4:0] INMODE;
    input MULTSIGNIN;
    (* invertible_pin = "IS_OPMODE_INVERTED" *)
    input [8:0] OPMODE;
    input [47:0] PCIN;
    (* invertible_pin = "IS_RSTA_INVERTED" *)
    input RSTA;
    (* invertible_pin = "IS_RSTALLCARRYIN_INVERTED" *)
    input RSTALLCARRYIN;
    (* invertible_pin = "IS_RSTALUMODE_INVERTED" *)
    input RSTALUMODE;
    (* invertible_pin = "IS_RSTB_INVERTED" *)
    input RSTB;
    (* invertible_pin = "IS_RSTC_INVERTED" *)
    input RSTC;
    (* invertible_pin = "IS_RSTCTRL_INVERTED" *)
    input RSTCTRL;
    (* invertible_pin = "IS_RSTD_INVERTED" *)
    input RSTD;
    (* invertible_pin = "IS_RSTINMODE_INVERTED" *)
    input RSTINMODE;
    (* invertible_pin = "IS_RSTM_INVERTED" *)
    input RSTM;
    (* invertible_pin = "IS_RSTP_INVERTED" *)
    input RSTP;
endmodule
