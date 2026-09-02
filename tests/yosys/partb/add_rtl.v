// Part B, Path B: plain `+` RTL with no vendor primitive in sight.
//
// `add_rtl` is the 16-bit registered adder quoted in usage.md Step 1.6; the
// Path B recipe must infer 4 CARRY4 slices from it and then map the rest onto
// aigpdk cells.
//
// `add_rtl8` is the same thing at 8 bits, used to show the inference scales
// down (2 CARRY4, the same shape Path A builds by hand).
module add_rtl(clk, a, b, cin, sum);
  input         clk;
  input  [15:0] a;
  input  [15:0] b;
  input         cin;
  output [15:0] sum;
  reg    [15:0] sum;

  always @(posedge clk)
    sum <= a + b + cin;
endmodule

module add_rtl8(clk, a, b, cin, sum);
  input        clk;
  input  [7:0] a;
  input  [7:0] b;
  input        cin;
  output [7:0] sum;
  reg    [7:0] sum;

  always @(posedge clk)
    sum <= a + b + cin;
endmodule
