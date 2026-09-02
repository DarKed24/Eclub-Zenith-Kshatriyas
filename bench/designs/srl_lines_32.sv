
module srl_lines_32 (
  input  logic       clk, rst, d_in, ce,
  input  logic [4:0] addr,
  output logic       q_xor, q31_xor
);
  logic [32-1:0] d_pipe;
  logic q [32], q31 [32], qc [32];
  always_ff @(posedge clk) begin
    d_pipe[0] <= d_in;
    for (int i = 1; i < 32; i++) d_pipe[i] <= d_pipe[i-1];
  end
  genvar g;
  generate for (g = 0; g < 32; g++) begin : line
    // even lanes: dynamic address; odd lanes: static per-lane tap + Q31 cascade
    if (g % 2 == 0) begin : dyn
      SRLC32E u (.Q(q[g]), .Q31(q31[g]), .A(addr), .CE(ce), .CLK(clk), .D(d_pipe[g]));
      assign qc[g] = 1'b0;
    end else begin : casc
      SRLC32E u0 (.Q(q[g]),  .Q31(q31[g]), .A(5'(g % 32)), .CE(ce), .CLK(clk), .D(d_pipe[g]));
      SRLC32E u1 (.Q(qc[g]), .Q31(),       .A(5'(31 - g % 32)), .CE(ce), .CLK(clk), .D(q31[g]));
    end
  end endgenerate
  logic qa, q31a;
  always_comb begin
    qa = 1'b0; q31a = 1'b0;
    for (int i = 0; i < 32; i++) begin qa ^= q[i] ^ qc[i]; q31a ^= q31[i]; end
  end
  always_ff @(posedge clk) begin q_xor <= rst ? 1'b0 : qa; q31_xor <= rst ? 1'b0 : q31a; end
endmodule
