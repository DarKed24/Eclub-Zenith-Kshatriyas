// Part A / Path B: plain accumulator RTL. The question is whether Yosys's
// `xilinx_dsp` folds `p <= p + a*b` into a DSP48E2 with PREG=1 + the post-adder
// (which gemmacro_map.v accepts) or leaves PREG=0 with the adder in fabric.
module dsp_rtl (
    input                 clk,
    input  signed [26:0]  a,
    input  signed [17:0]  b,
    output reg signed [47:0] p
);
  always @(posedge clk)
    p <= p + a * b;
endmodule

// The C-feedback MACC form, which some Yosys releases prefer.
module dsp_rtl_c (
    input                 clk,
    input  signed [26:0]  a,
    input  signed [17:0]  b,
    input  signed [47:0]  c,
    output reg signed [47:0] p
);
  always @(posedge clk)
    p <= c + a * b;
endmodule

// Plain registered multiply - no accumulator, so PREG should be foldable.
module dsp_rtl_mult (
    input                 clk,
    input  signed [26:0]  a,
    input  signed [17:0]  b,
    output reg signed [44:0] p
);
  always @(posedge clk)
    p <= a * b;
endmodule
