// Part C: configurations `gemsrl_map.v` must refuse rather than mis-model.
// Each is a single SRL16E whose parameters put it outside GEM's one supported
// shape (one global rising edge, active-high enable).
module srl_inv_clk (input clk, input ce, input d, output q);
  SRL16E #(.IS_CLK_INVERTED(1'b1)) u (
    .Q(q), .A0(1'b1), .A1(1'b1), .A2(1'b1), .A3(1'b0),
    .CE(ce), .CLK(clk), .D(d)
  );
endmodule
