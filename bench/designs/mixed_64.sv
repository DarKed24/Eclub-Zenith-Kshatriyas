
module mixed_64 (
  input  logic        clk, rst, ce,
  input  logic [26:0] a_in,
  input  logic [17:0] b_in,
  input  logic [31:0] x_in, y_in,
  input  logic [4:0]  addr,
  output logic [47:0] p_xor,
  output logic [31:0] s_xor,
  output logic        q_xor
);
  logic [64-1:0][26:0] a_pipe;  logic [64-1:0][17:0] b_pipe;
  logic [64-1:0][31:0] x_pipe, y_pipe;
  logic [47:0] p [64];
  logic [31:0] o [64], co [64], s [64];
  logic q [64];
  always_ff @(posedge clk) begin
    a_pipe[0] <= a_in; b_pipe[0] <= b_in; x_pipe[0] <= x_in; y_pipe[0] <= y_in;
    for (int i = 1; i < 64; i++) begin
      a_pipe[i] <= a_pipe[i-1]; b_pipe[i] <= b_pipe[i-1];
      x_pipe[i] <= x_pipe[i-1]; y_pipe[i] <= y_pipe[i-1];
    end
  end
  genvar g, c;
  generate for (g = 0; g < 64; g++) begin : lane
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
    assign s[g] = x_pipe[g] ^ y_pipe[g];
    for (c = 0; c < 8; c++) begin : slice
      if (c == 0) begin : first
        CARRY4 u (.CO(co[g][3:0]), .O(o[g][3:0]), .CI(1'b0), .CYINIT(1'b0),
                  .DI(x_pipe[g][3:0]), .S(s[g][3:0]));
      end else begin : rest
        CARRY4 u (.CO(co[g][4*c+3:4*c]), .O(o[g][4*c+3:4*c]), .CI(co[g][4*c-1]),
                  .CYINIT(1'b0), .DI(x_pipe[g][4*c+3:4*c]), .S(s[g][4*c+3:4*c]));
      end
    end
    // the SRL delays the adder's carry-out; its read tap is data dependent
    SRLC32E u_srl (.Q(q[g]), .Q31(), .A(addr ^ o[g][4:0]), .CE(ce), .CLK(clk), .D(co[g][31]));
  end endgenerate
  logic [47:0] pa; logic [31:0] sa; logic qa;
  always_comb begin
    pa = '0; sa = '0; qa = 1'b0;
    for (int i = 0; i < 64; i++) begin pa ^= p[i]; sa ^= o[i]; qa ^= q[i]; end
  end
  always_ff @(posedge clk) begin
    p_xor <= rst ? '0 : pa; s_xor <= rst ? '0 : sa; q_xor <= rst ? 1'b0 : qa;
  end
endmodule
