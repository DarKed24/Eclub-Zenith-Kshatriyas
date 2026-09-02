// Part A worked example: an 8x8 multiply-accumulate built around a
// hand-instantiated GEM_DSP48E2, with ordinary RTL glue so the aigpdk flow
// has real 1-bit logic to map alongside the 48-bit macro.
//
//   load=1 -> OPMODE_S=1 -> P <= A*B      (load a fresh product)
//   load=0 -> OPMODE_S=2 -> P <= P + A*B  (accumulate)
//   rst    -> RSTP        -> P <= 0       (synchronous, beats CEP)
module mac_top (
    input  wire        clk,
    input  wire        rst,
    input  wire        load,
    input  wire [7:0]  a_in,
    input  wire [7:0]  b_in,
    output wire        and_out,
    output wire        or_out,
    output wire        inv_out,
    output wire        areg_out
);
  reg [7:0] a_reg, b_reg;
  always @(posedge clk) begin
    a_reg <= a_in & {8{~rst}};
    b_reg <= b_in;
  end

  wire [47:0] p_int;

  GEM_DSP48E2 dsp (
    .CLK(clk), .CEP(1'b1), .RSTP(rst), .USE_D(1'b0),
    .A({19'b0, a_reg}), .D(27'b0),
    .B({10'b0, b_reg}), .C(48'b0),
    .OPMODE_S({~load, load}), .P(p_int)
  );

  assign and_out  = p_int[0] & p_int[5];
  assign or_out   = p_int[3] | p_int[9];
  assign inv_out  = ~p_int[12];
  assign areg_out = a_reg[3] & b_reg[2];
endmodule
