// Does `techmap -map gemmacro_map.v` preserve the DSP48E2's arithmetic?
// Compares the mapped `dsp_static` (OPMODE 9'h025, decoded to OPMODE_S=2)
// against a plain RTL multiply-accumulate with the same reset/enable rules
// (RSTP beats CEP), over random signed vectors.
`timescale 1ns/1ps

module mac_golden (
    input                     clk,
    input                     ce,
    input                     rst,
    input  signed [26:0]      a,
    input  signed [17:0]      b,
    output reg signed [47:0]  p
);
  always @(posedge clk)
    if (rst)      p <= 48'sd0;
    else if (ce)  p <= p + a * b;
endmodule

module tb_opmode;
  reg clk = 0, ce = 1, rst = 1;
  reg signed [26:0] a = 0;
  reg signed [17:0] b = 0;
  reg signed [47:0] c = 0;
  wire signed [47:0] p_map, p_ref;
  integer i, errors;

  dsp_static  u_map (.clk(clk), .ce(ce), .rst(rst), .a(a), .b(b), .c(c), .p(p_map));
  mac_golden  u_ref (.clk(clk), .ce(ce), .rst(rst), .a(a), .b(b), .p(p_ref));

  always #5 clk = ~clk;

  initial begin
    errors = 0;
    @(negedge clk); @(negedge clk);
    for (i = 0; i < 1000; i = i + 1) begin
      @(negedge clk);
      a   = {$random, $random};
      b   = $random;
      rst = ($random % 32 == 0);
      ce  = ($random % 8 != 0);
      @(posedge clk);
      #2;
      if (p_map !== p_ref) begin
        errors = errors + 1;
        if (errors < 5)
          $display("MISMATCH i=%0d rst=%b ce=%b a=%0d b=%0d mapped=%0d golden=%0d",
                   i, rst, ce, a, b, p_map, p_ref);
      end
    end
    if (errors == 0)
      $display("PASS: 1000 random vectors, techmapped DSP48E2 == golden MAC");
    else
      $display("FAIL: %0d mismatches", errors);
    $finish;
  end
endmodule
