// Part A: data-dependent OPMODE - `load` picks P<=A*B vs P<=P+A*B.
// gemmacro_map.v must synthesise this into a couple of gates driving OPMODE_S.
module dsp_dyn (
    input                 clk,
    input                 load,
    input  signed [26:0]  a,
    input  signed [17:0]  b,
    output signed [47:0]  p
);
  wire [8:0] opmode = load ? 9'h005 : 9'h025;

  DSP48E2 #(
    .ACASCREG(0), .ADREG(0), .ALUMODEREG(0), .AREG(0),
    .BCASCREG(0), .BREG(0), .CARRYINREG(0), .CARRYINSELREG(0),
    .CREG(0), .DREG(0), .INMODEREG(0), .MREG(0), .OPMODEREG(0),
    .PREG(1)
  ) u (
    .CLK(clk), .CEP(1'b1), .RSTP(1'b0),
    .A({3'b000, a}), .B(b), .C(48'b0), .D(27'b0),
    .OPMODE(opmode), .ALUMODE(4'b0000), .INMODE(5'b00000),
    .CARRYIN(1'b0), .CARRYINSEL(3'b000),
    .P(p)
  );
endmodule

// Rejected configurations: PREG=0 and a non-zero AREG must each raise a
// clean $error out of gemmacro_map.v rather than mapping silently.
module dsp_preg0 (
    input clk, input signed [26:0] a, input signed [17:0] b,
    output signed [47:0] p
);
  DSP48E2 #(
    .ACASCREG(0), .ADREG(0), .ALUMODEREG(0), .AREG(0),
    .BCASCREG(0), .BREG(0), .CARRYINREG(0), .CARRYINSELREG(0),
    .CREG(0), .DREG(0), .INMODEREG(0), .MREG(0), .OPMODEREG(0),
    .PREG(0)
  ) u (
    .CLK(clk), .CEP(1'b1), .RSTP(1'b0),
    .A({3'b000, a}), .B(b), .C(48'b0), .D(27'b0),
    .OPMODE(9'h005), .ALUMODE(4'b0000), .INMODE(5'b00000),
    .CARRYIN(1'b0), .CARRYINSEL(3'b000),
    .P(p)
  );
endmodule

module dsp_areg1 (
    input clk, input signed [26:0] a, input signed [17:0] b,
    output signed [47:0] p
);
  DSP48E2 #(
    .ACASCREG(1), .ADREG(0), .ALUMODEREG(0), .AREG(1),
    .BCASCREG(0), .BREG(0), .CARRYINREG(0), .CARRYINSELREG(0),
    .CREG(0), .DREG(0), .INMODEREG(0), .MREG(0), .OPMODEREG(0),
    .PREG(1)
  ) u (
    .CLK(clk), .CEP(1'b1), .RSTP(1'b0), .CEA2(1'b1),
    .A({3'b000, a}), .B(b), .C(48'b0), .D(27'b0),
    .OPMODE(9'h005), .ALUMODE(4'b0000), .INMODE(5'b00000),
    .CARRYIN(1'b0), .CARRYINSEL(3'b000),
    .P(p)
  );
endmodule
