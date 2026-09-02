// RTL (with the GEM_DSP48E2 behavioural model) vs the aigpdk gate netlist.
`timescale 1ns/1ps
module tb_a1;
  reg clk = 0, rst = 1, load = 0;
  reg [7:0] a_in = 0, b_in = 0;
  wire ar, orr, ir, gr;
  wire ag, og, ig, gg;
  integer i, errors;

  mac_top      u_ref  (.clk(clk), .rst(rst), .load(load), .a_in(a_in), .b_in(b_in),
                       .and_out(ar), .or_out(orr), .inv_out(ir), .areg_out(gr));
  mac_top_gate u_gate (.clk(clk), .rst(rst), .load(load), .a_in(a_in), .b_in(b_in),
                       .and_out(ag), .or_out(og), .inv_out(ig), .areg_out(gg));

  always #5 clk = ~clk;

  initial begin
    errors = 0;
    @(negedge clk); @(negedge clk);          // two reset cycles
    rst = 0;
    for (i = 0; i < 512; i = i + 1) begin
      @(negedge clk);
      a_in = $random; b_in = $random;
      load = ($random % 4 == 0);
      rst  = ($random % 16 == 0);
      @(posedge clk);
      #2;
      if ({ar,orr,ir,gr} !== {ag,og,ig,gg}) begin
        errors = errors + 1;
        if (errors < 5)
          $display("MISMATCH cycle %0d: rtl=%b%b%b%b gate=%b%b%b%b",
                   i, ar, orr, ir, gr, ag, og, ig, gg);
      end
    end
    if (errors == 0)
      $display("PASS: 512 cycles, RTL and aigpdk gate netlist agree bit-exactly");
    else
      $display("FAIL: %0d mismatching cycles", errors);
    $finish;
  end
endmodule
