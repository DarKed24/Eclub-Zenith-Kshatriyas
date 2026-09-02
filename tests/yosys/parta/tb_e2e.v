// Drives the aigpdk gate netlist with the same vectors GEM gets from
// stim.vcd and writes the four primary outputs to iv_out.txt, one line per
// cycle.
//
// Sampling convention: the line for cycle i is taken *just before* the rising
// edge at t = 10*i+5, i.e. the state the registers settled into at the
// previous edge. That is exactly what GEM's naive_sim timestamps at that edge
// (the same pre-edge `Q` convention its DFF and macro outputs follow), so the
// two files line up cycle for cycle with no shift.
`timescale 1ns/1ps
module tb_e2e;
  parameter CYCLES = 256;

  reg  [17:0] vecs [0:CYCLES-1];
  reg  clk = 0;
  reg  rst, load;
  reg  [7:0] a_in, b_in;
  wire and_out, or_out, inv_out, areg_out;
  integer i, fh;

  mac_top_gate dut (.clk(clk), .rst(rst), .load(load),
                    .a_in(a_in), .b_in(b_in),
                    .and_out(and_out), .or_out(or_out),
                    .inv_out(inv_out), .areg_out(areg_out));

  initial begin
    $readmemb("stim.txt", vecs);
    fh = $fopen("iv_out.txt", "w");
    for (i = 0; i < CYCLES; i = i + 1) begin
      clk  = 0;                       // t = 10*i : apply this cycle inputs
      rst  = vecs[i][17];
      load = vecs[i][16];
      a_in = vecs[i][15:8];
      b_in = vecs[i][7:0];
      #4;
      $fwrite(fh, "%b%b%b%b\n", and_out, or_out, inv_out, areg_out);
      #1;
      clk = 1;                        // t = 10*i+5 : rising edge
      #5;
    end
    $fclose(fh);
    $display("iverilog: wrote %0d cycles to iv_out.txt", CYCLES);
    $finish;
  end
endmodule
