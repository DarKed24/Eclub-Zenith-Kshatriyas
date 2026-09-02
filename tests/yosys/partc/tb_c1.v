// Part C Step 4: source RTL (with the SRLC32E behavioural model) vs the mapped
// aigpdk gate netlist, 512 random cycles. Both instances own an independent
// 32-bit shift state, so this also checks that synthesis did not perturb the
// address glue in front of the macro.
`timescale 1ns/1ps
module tb_c1;
  reg clk = 0, d_in = 0, ce_in = 0;
  reg [4:0] a_in = 0;
  wire qr, q31r, qg, q31g;
  integer i, errors;

  ysrl      u_ref  (.clk(clk), .d_in(d_in), .a_in(a_in), .ce_in(ce_in),
                    .q_out(qr), .q31_out(q31r));
  ysrl_gate u_gate (.clk(clk), .d_in(d_in), .a_in(a_in), .ce_in(ce_in),
                    .q_out(qg), .q31_out(q31g));

  always #5 clk = ~clk;

  initial begin
    errors = 0;
    @(negedge clk);
    for (i = 0; i < 512; i = i + 1) begin
      @(negedge clk);
      d_in = $random; a_in = $random; ce_in = ($random % 4) != 0;
      @(posedge clk);
      #2;
      if ({qr, q31r} !== {qg, q31g}) begin
        errors = errors + 1;
        if (errors < 5)
          $display("MISMATCH cycle %0d: rtl=%b/%b gate=%b/%b", i, qr, q31r, qg, q31g);
      end
    end
    if (errors == 0)
      $display("PASS: 512 cycles, RTL and aigpdk gate netlist agree bit-exactly");
    else
      $display("FAIL: %0d mismatching cycles", errors);
    $finish;
  end
endmodule
