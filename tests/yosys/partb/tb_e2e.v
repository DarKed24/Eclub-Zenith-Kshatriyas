// Drives the aigpdk gate netlist with the same vectors GEM gets from stim.vcd
// and writes the two primary outputs to iv_out.txt, one line per cycle.
//
// Sampling convention: the line for cycle i is taken *just before* the rising
// edge at t = 10*i+5, i.e. with the registers holding what they settled into at
// the previous edge but with cycle i's `cin` already applied. That is exactly
// what GEM's naive_sim timestamps at that edge (the same pre-edge `Q`
// convention its DFF outputs follow), so the two files line up cycle for cycle
// with no shift.
`timescale 1ns/1ps
module tb_e2e;
  parameter CYCLES = 256;

  reg  [16:0] vecs [0:CYCLES-1];
  reg  clk = 0;
  reg  cin;
  reg  [7:0] a_in, b_in;
  wire [7:0] sum_out;
  wire cout;
  integer i, fh;

  yadder_gate dut (.clk(clk), .a_in(a_in), .b_in(b_in), .cin(cin),
                   .sum_out(sum_out), .cout(cout));

  initial begin
    $readmemb("stim.txt", vecs);
    fh = $fopen("iv_out.txt", "w");
    for (i = 0; i < CYCLES; i = i + 1) begin
      clk  = 0;                       // t = 10*i : apply this cycle's inputs
      cin  = vecs[i][16];
      a_in = vecs[i][15:8];
      b_in = vecs[i][7:0];
      #4;
      $fwrite(fh, "%b%b\n", sum_out, cout);
      #1;
      clk = 1;                        // t = 10*i+5 : rising edge
      #5;
    end
    $fclose(fh);
    $display("iverilog: wrote %0d cycles to iv_out.txt", CYCLES);
    $finish;
  end
endmodule
