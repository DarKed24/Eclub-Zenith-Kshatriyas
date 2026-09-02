// Drives the aigpdk gate netlist with the same vectors GEM gets from stim.vcd
// and writes the two primary outputs to iv_out.txt, one line per cycle.
//
// Sampling convention: the line for cycle i is taken *just before* the rising
// edge at t = 10*i+5, i.e. with the registers and the 32-bit shift state
// holding what they settled into at the previous edge but with cycle i's
// `a_in` / `ce_in` already applied. That is exactly what GEM's naive_sim
// timestamps at that edge (the same pre-edge `Q` convention its DFF and macro
// outputs follow), so the two files line up cycle for cycle with no shift.
`timescale 1ns/1ps
module tb_e2e;
  parameter CYCLES = 256;

  reg  [6:0] vecs [0:CYCLES-1];
  reg  clk = 0;
  reg  d_in, ce_in;
  reg  [4:0] a_in;
  wire q_out, q31_out;
  integer i, fh;

  ysrl_gate dut (.clk(clk), .d_in(d_in), .a_in(a_in), .ce_in(ce_in),
                 .q_out(q_out), .q31_out(q31_out));

  initial begin
    $readmemb("stim.txt", vecs);
    fh = $fopen("iv_out.txt", "w");
    for (i = 0; i < CYCLES; i = i + 1) begin
      clk   = 0;                      // t = 10*i : apply this cycle's inputs
      ce_in = vecs[i][6];
      d_in  = vecs[i][5];
      a_in  = vecs[i][4:0];
      #4;
      $fwrite(fh, "%b%b\n", q_out, q31_out);
      #1;
      clk = 1;                        // t = 10*i+5 : rising edge
      #5;
    end
    $fclose(fh);
    $display("iverilog: wrote %0d cycles to iv_out.txt", CYCLES);
    $finish;
  end
endmodule
