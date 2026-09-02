// Part B Step 4: source RTL (with the CARRY4 behavioural model) vs the mapped
// aigpdk gate netlist, 512 random cycles.
`timescale 1ns/1ps
module tb_b1;
  reg clk = 0, cin = 0;
  reg [7:0] a_in = 0, b_in = 0;
  wire [7:0] sr, sg;
  wire cr, cg;
  integer i, errors;

  yadder      u_ref  (.clk(clk), .a_in(a_in), .b_in(b_in), .cin(cin),
                      .sum_out(sr), .cout(cr));
  yadder_gate u_gate (.clk(clk), .a_in(a_in), .b_in(b_in), .cin(cin),
                      .sum_out(sg), .cout(cg));

  always #5 clk = ~clk;

  initial begin
    errors = 0;
    @(negedge clk);
    for (i = 0; i < 512; i = i + 1) begin
      @(negedge clk);
      a_in = $random; b_in = $random; cin = $random;
      @(posedge clk);
      #2;
      if ({sr, cr} !== {sg, cg}) begin
        errors = errors + 1;
        if (errors < 5)
          $display("MISMATCH cycle %0d: rtl=%b/%b gate=%b/%b", i, sr, cr, sg, cg);
      end
    end
    if (errors == 0)
      $display("PASS: 512 cycles, RTL and aigpdk gate netlist agree bit-exactly");
    else
      $display("FAIL: %0d mismatching cycles", errors);
    $finish;
  end
endmodule
