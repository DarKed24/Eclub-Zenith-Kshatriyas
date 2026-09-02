
module carry_adders_16x64 (
  input  logic         clk, rst,
  input  logic [63:0] x_in, y_in,
  output logic [63:0] s_xor,
  output logic         c_xor
);
  logic [16-1:0][63:0] x_pipe, y_pipe;
  logic [63:0] o [16];
  logic [63:0] co [16];
  logic [63:0] s [16], di [16];
  always_ff @(posedge clk) begin
    x_pipe[0] <= x_in; y_pipe[0] <= y_in;
    for (int i = 1; i < 16; i++) begin
      x_pipe[i] <= x_pipe[i-1]; y_pipe[i] <= y_pipe[i-1];
    end
  end
  genvar g, c;
  generate for (g = 0; g < 16; g++) begin : adder
    assign s[g]  = x_pipe[g] ^ y_pipe[g];   // propagate
    assign di[g] = x_pipe[g];               // generate data
    for (c = 0; c < 16; c++) begin : slice
      if (c == 0) begin : first
        CARRY4 u (.CO(co[g][3:0]), .O(o[g][3:0]), .CI(1'b0), .CYINIT(1'b0),
                  .DI(di[g][3:0]), .S(s[g][3:0]));
      end else begin : rest
        CARRY4 u (.CO(co[g][4*c+3:4*c]), .O(o[g][4*c+3:4*c]), .CI(co[g][4*c-1]),
                  .CYINIT(1'b0), .DI(di[g][4*c+3:4*c]), .S(s[g][4*c+3:4*c]));
      end
    end
  end endgenerate
  logic [63:0] s_acc; logic c_acc;
  always_comb begin
    s_acc = '0; c_acc = 1'b0;
    for (int i = 0; i < 16; i++) begin s_acc ^= o[i]; c_acc ^= co[i][63]; end
  end
  always_ff @(posedge clk) begin s_xor <= rst ? '0 : s_acc; c_xor <= rst ? 1'b0 : c_acc; end
endmodule
