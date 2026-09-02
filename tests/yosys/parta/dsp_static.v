// Part A / Path A: explicitly instantiated PREG=1 DSP48E2 MAC, static OPMODE.
// OPMODE 9'h025 -> X=01, Y=01, Z=010  =>  P <= P + A*B  (OPMODE_S == 2)
module dsp_static (
    input                 clk,
    input                 ce,
    input                 rst,
    input  signed [26:0]  a,
    input  signed [17:0]  b,
    input  signed [47:0]  c,
    output signed [47:0]  p
);
  DSP48E2 #(
    .ACASCREG(0), .ADREG(0), .ALUMODEREG(0), .AREG(0),
    .BCASCREG(0), .BREG(0), .CARRYINREG(0), .CARRYINSELREG(0),
    .CREG(0), .DREG(0), .INMODEREG(0), .MREG(0), .OPMODEREG(0),
    .PREG(1)
  ) u (
    .CLK(clk), .CEP(ce), .RSTP(rst),
    .A({3'b000, a}), .B(b), .C(c), .D(27'b0),
    .OPMODE(9'h025), .ALUMODE(4'b0000), .INMODE(5'b00000),
    .CARRYIN(1'b0), .CARRYINSEL(3'b000),
    .P(p)
  );
endmodule

// Same, but OPMODE 9'h005 -> Z=000 => P <= A*B (OPMODE_S == 1)
module dsp_static_mult (
    input                 clk,
    input  signed [26:0]  a,
    input  signed [17:0]  b,
    output signed [47:0]  p
);
  DSP48E2 #(
    .ACASCREG(0), .ADREG(0), .ALUMODEREG(0), .AREG(0),
    .BCASCREG(0), .BREG(0), .CARRYINREG(0), .CARRYINSELREG(0),
    .CREG(0), .DREG(0), .INMODEREG(0), .MREG(0), .OPMODEREG(0),
    .PREG(1)
  ) u (
    .CLK(clk), .CEP(1'b1), .RSTP(1'b0),
    .A({3'b000, a}), .B(b), .C(48'b0), .D(27'b0),
    .OPMODE(9'h005), .ALUMODE(4'b0000), .INMODE(5'b00000),
    .CARRYIN(1'b0), .CARRYINSEL(3'b000),
    .P(p)
  );
endmodule
