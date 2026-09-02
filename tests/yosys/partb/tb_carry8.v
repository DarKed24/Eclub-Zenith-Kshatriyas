// Part B Step 7: the CARRY8 -> 2x CARRY4 techmap must preserve the arithmetic.
// Compares the techmapped `carry8_top` against a plain 8-bit `a + b + cin`
// over every one of the 512 (a,b) corner-ish vectors plus 1000 random ones.
`timescale 1ns/1ps
module tb_carry8;
  reg  [7:0] a, b;
  reg        cin;
  wire [7:0] sum;
  wire       cout;
  wire [8:0] golden = a + b + cin;
  integer i, errors;

  carry8_top dut (.a(a), .b(b), .cin(cin), .sum(sum), .cout(cout));

  initial begin
    errors = 0;
    for (i = 0; i < 1000; i = i + 1) begin
      a = $random; b = $random; cin = $random;
      #1;
      if ({cout, sum} !== golden) begin
        errors = errors + 1;
        if (errors < 5)
          $display("MISMATCH %0d: %0d + %0d + %0d -> %b_%b, want %b",
                   i, a, b, cin, cout, sum, golden);
      end
    end
    if (errors == 0)
      $display("PASS: 1000 random vectors, CARRY8 -> 2x CARRY4 == a + b + cin");
    else
      $display("FAIL: %0d mismatching vectors", errors);
    $finish;
  end
endmodule
