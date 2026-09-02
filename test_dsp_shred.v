module test_dsp(
    input  wire        CLK,
    input  wire [26:0] A,
    input  wire [17:0] B,
    input  wire [47:0] C,
    output wire [47:0] P
);

    GEM_DSP48E2 dsp_inst (
        .CLK(CLK),
        .A(A),
        .B(B),
        .C(C),
        .P(P)
    );

endmodule
