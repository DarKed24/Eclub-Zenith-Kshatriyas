
module mac_array_256 (
  input  logic        clk, rst,
  input  logic [26:0] a_in,
  input  logic [17:0] b_in,
  output logic [47:0] p_xor
);
  logic [256-1:0][26:0] a_pipe;
  logic [256-1:0][17:0] b_pipe;
  logic [47:0] p [256];
  always_ff @(posedge clk) begin
    a_pipe[0] <= a_in; b_pipe[0] <= b_in;
    for (int i = 1; i < 256; i++) begin
      a_pipe[i] <= a_pipe[i-1]; b_pipe[i] <= b_pipe[i-1];
    end
  end
  genvar g;
  generate for (g = 0; g < 256; g++) begin : lane
    DSP48E2 #(.AREG(0), .BREG(0), .CREG(0), .DREG(0), .ADREG(0), .MREG(0), .PREG(1),
             .USE_MULT("MULTIPLY"), .USE_SIMD("ONE48")) u_dsp (
      .CLK(clk), .A(a_pipe[g]), .B(b_pipe[g]), .C(48'd0), .D(27'd0),
      .OPMODE(9'b000100101), .INMODE(5'b00000), .ALUMODE(4'b0000),
      .CARRYIN(1'b0), .CARRYINSEL(3'b000),
      .CEA1(1'b0), .CEA2(1'b0), .CEB1(1'b0), .CEB2(1'b0), .CEC(1'b0), .CED(1'b0),
      .CEAD(1'b0), .CEM(1'b0), .CEP(1'b1), .CEALUMODE(1'b0), .CECARRYIN(1'b0),
      .CECTRL(1'b0), .CEINMODE(1'b0),
      .RSTA(1'b0), .RSTB(1'b0), .RSTC(1'b0), .RSTD(1'b0), .RSTM(1'b0), .RSTP(rst),
      .RSTALLCARRYIN(1'b0), .RSTALUMODE(1'b0), .RSTCTRL(1'b0), .RSTINMODE(1'b0),
      .ACIN(30'd0), .BCIN(18'd0), .PCIN(48'd0), .CARRYCASCIN(1'b0), .MULTSIGNIN(1'b0),
      .P(p[g])
    );
  end endgenerate
  always_comb begin
    p_xor = '0;
    for (int i = 0; i < 256; i++) p_xor ^= p[i];
  end
endmodule
