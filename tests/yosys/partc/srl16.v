// Part C: exercise gemsrl_map.v's SRL16E -> SRLC32E rule with A3..A0 = 4'b0111
// (a 8-deep tap). `srl16_gold` is the plain RTL the mapped cell must match.
module srl16_top (input clk, input ce, input d, output q);
  SRL16E u (
    .Q(q), .A0(1'b1), .A1(1'b1), .A2(1'b1), .A3(1'b0),
    .CE(ce), .CLK(clk), .D(d)
  );
endmodule

// Same delay, written the obvious way: Q is stage 7 of the shift register.
module srl16_gold (input clk, input ce, input d, output q);
  reg [15:0] sh = 16'h0000;
  always @(posedge clk) if (ce) sh <= {sh[14:0], d};
  assign q = sh[7];
endmodule
