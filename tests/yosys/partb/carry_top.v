// Part B, Path A: an 8-bit registered adder built from two explicitly
// instantiated CARRY4 slices chained CO[3] -> CI.
//
// This is the RTL that produces the checked-in frontend fixture
// tests/data/yosys_carry4.gv (the module name `yadder` and the net/instance
// names below are chosen so the Yosys output matches that file byte for byte).
// Running it through `synth -flatten` + `dfflibmap`/`abc -liberty
// aigpdk_nomem.lib` must leave both CARRY4 instances intact and map only the
// XOR glue and the input registers onto aigpdk cells.
//
// The two slices are the fusion target: GEM's `fuse_carry4_chains` collapses
// the CO[3] -> CI cascade into one `MacroKind::Carry4 { chain_len: 2 }`.
module yadder(clk, a_in, b_in, cin, sum_out, cout);
  input        clk;
  input  [7:0] a_in;
  input  [7:0] b_in;
  input        cin;
  output [7:0] sum_out;
  output       cout;

  reg [7:0] a, b;
  always @(posedge clk) begin
    a <= a_in;
    b <= b_in;
  end

  wire [7:0] s = a ^ b;           // propagate
  wire [3:0] co_lo, co_hi;

  CARRY4 c_lo (
    .CO(co_lo), .O(sum_out[3:0]),
    .CI(cin), .CYINIT(1'b0),
    .DI(a[3:0]), .S(s[3:0])
  );

  CARRY4 c_hi (
    .CO(co_hi), .O(sum_out[7:4]),
    .CI(co_lo[3]), .CYINIT(1'b0),
    .DI(a[7:4]), .S(s[7:4])
  );

  assign cout = co_hi[3];
endmodule
