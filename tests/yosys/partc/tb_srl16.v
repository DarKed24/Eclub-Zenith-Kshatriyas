// Part C Step 8: the SRL16E -> SRLC32E techmap must preserve the delay, not
// just the cell name. Compares the techmapped `srl16_top` (A = 5'h07 into a
// 32-deep SRLC32E) against a plain 16-deep RTL shift register read at stage 7,
// over 1000 random cycles with a randomly gated clock enable.
`timescale 1ns/1ps
module tb_srl16;
  reg clk = 0, ce = 0, d = 0;
  wire q_map, q_gold;
  integer i, errors;

  srl16_top  dut  (.clk(clk), .ce(ce), .d(d), .q(q_map));
  srl16_gold gold (.clk(clk), .ce(ce), .d(d), .q(q_gold));

  always #5 clk = ~clk;

  initial begin
    errors = 0;
    @(negedge clk);
    for (i = 0; i < 1000; i = i + 1) begin
      @(negedge clk);
      d = $random; ce = ($random % 4) != 0;
      @(posedge clk);
      #2;
      if (q_map !== q_gold) begin
        errors = errors + 1;
        if (errors < 5)
          $display("MISMATCH cycle %0d: mapped=%b golden=%b", i, q_map, q_gold);
      end
    end
    if (errors == 0)
      $display("PASS: 1000 cycles, SRL16E -> SRLC32E == a 16-deep shift register");
    else
      $display("FAIL: %0d mismatching cycles", errors);
    $finish;
  end
endmodule
