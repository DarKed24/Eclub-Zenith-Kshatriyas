// Part B: an explicitly instantiated UltraScale CARRY8, used to exercise the
// `aigpdk/gemcarry_map.v` split rule (CARRY8 -> two chained CARRY4).
//
// Yosys 0.68 never *emits* a CARRY8 on its own — `synth_xilinx -family xcup`
// produces CARRY4 — so the only way to reach that techmap rule is to write the
// cell by hand, which is what this fixture is for.
module carry8_top(a, b, cin, sum, cout);
  input  [7:0] a;
  input  [7:0] b;
  input        cin;
  output [7:0] sum;
  output       cout;

  wire [7:0] s = a ^ b;
  wire [7:0] co;

  CARRY8 u (
    .CO(co), .O(sum),
    .CI(cin), .CI_TOP(1'b0),
    .DI(a), .S(s)
  );

  assign cout = co[7];
endmodule
