module a(input [7:0] x, input [7:0] y, input ci, output [7:0] s, output co);
assign {co,s} = x + y + ci;
endmodule
