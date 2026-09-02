// Part C / Path B: plain RTL shift registers at several depths, with and
// without a clock enable. What matters per depth is which primitive Yosys
// infers (SRL16E / SRLC32E / a Q31->D cascade) and whether a non-constant CE
// survives inference.
module srl8 (input clk, input d, output q);
  reg [7:0] sh;
  always @(posedge clk) sh <= {sh[6:0], d};
  assign q = sh[7];
endmodule

module srl20 (input clk, input d, output q);
  reg [19:0] sh;
  always @(posedge clk) sh <= {sh[18:0], d};
  assign q = sh[19];
endmodule

module srl32 (input clk, input d, output q);
  reg [31:0] sh;
  always @(posedge clk) sh <= {sh[30:0], d};
  assign q = sh[31];
endmodule

module srl48 (input clk, input d, output q);
  reg [47:0] sh;
  always @(posedge clk) sh <= {sh[46:0], d};
  assign q = sh[47];
endmodule

// The clock-enable case: Yosys 0.33 dropped `ce` here and tied SRLC32E.CE to 1.
module srl32_ce (input clk, input ce, input d, output q);
  reg [31:0] sh;
  always @(posedge clk) if (ce) sh <= {sh[30:0], d};
  assign q = sh[31];
endmodule

// Dynamic tap: the addressed-read form that maps to SRLC32E's A port.
module srl32_addr (input clk, input ce, input d, input [4:0] a, output q);
  reg [31:0] sh;
  always @(posedge clk) if (ce) sh <= {sh[30:0], d};
  assign q = sh[a];
endmodule
