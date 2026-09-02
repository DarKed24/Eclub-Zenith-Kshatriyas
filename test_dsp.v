module test_dsp (
    input  wire clk,
    input  wire [26:0] a,
    input  wire [17:0] b,
    input  wire [47:0] c,
    output wire [47:0] p
);

    DSP48E2 #(
        .PREG(1),
        .AREG(0)
    ) dsp_inst (
        .CLK(clk),
        .A(a),
        .B(b),
        .C(c),
        .P(p)
    );

endmodule
