// Part C / Path A source for tests/data/yosys_srl.gv.
// Explicit SRLC32E with registered data and address. The `^ 5'b10101` on the
// address keeps a little AIG glue (INV cells) in front of the register; the
// SRLC32E itself must survive synth + abc as a blackbox instance with `A`
// connected as a bus.
module ysrl (
    input        clk,
    input        d_in,
    input  [4:0] a_in,
    input        ce_in,
    output       q_out,
    output       q31_out
);
  reg       d;
  reg [4:0] a;
  always @(posedge clk) begin
    d <= d_in;
    a <= a_in ^ 5'b10101;
  end

  SRLC32E sr (
    .Q(q_out), .Q31(q31_out),
    .A(a), .CE(ce_in), .CLK(clk), .D(d)
  );
endmodule
