#!/usr/bin/env python3
"""Generate the Zenith benchmark family as SystemVerilog (IEEE 1800-2012).

    gen_bench.py OUTDIR

Writes, for each design, DESIGN.sv plus DESIGN.ports (the input port:width list
gen_stim.py needs). Every design pipelines its primary inputs through a
register chain so lane i sees lane i-1's data one cycle later: primary I/O
stays small regardless of N, and every netlist mixes ordinary AIG logic (the
DFF chain, XOR reductions) with the macros - the heterogeneous graph the PS is
about. Outputs are XOR-reduced so the checker still depends on every lane.

  mac_array_N     : N DSP48E2 MAC units (P <= P + A*B, PREG=1) + 48-bit XOR tree
  carry_adders_KxW: K independent W-bit adders, each a chain of W/4 CARRY4
                    (a chain fuses into ONE GEM macro) + XOR-reduced sums
  srl_lines_N     : N SRLC32E delay lines with per-lane static addresses, half
                    of them cascaded through Q31 into a 64-deep line
  mixed_N         : N of each, sharing the pipeline (the hidden-benchmark shape)
"""
import os, sys

OUT = sys.argv[1] if len(sys.argv) > 1 else "designs"
os.makedirs(OUT, exist_ok=True)

DSP_PORTS = """      .CLK(clk), .A({a}), .B({b}), .C(48'd0), .D(27'd0),
      .OPMODE(9'b000100101), .INMODE(5'b00000), .ALUMODE(4'b0000),
      .CARRYIN(1'b0), .CARRYINSEL(3'b000),
      .CEA1(1'b0), .CEA2(1'b0), .CEB1(1'b0), .CEB2(1'b0), .CEC(1'b0), .CED(1'b0),
      .CEAD(1'b0), .CEM(1'b0), .CEP(1'b1), .CEALUMODE(1'b0), .CECARRYIN(1'b0),
      .CECTRL(1'b0), .CEINMODE(1'b0),
      .RSTA(1'b0), .RSTB(1'b0), .RSTC(1'b0), .RSTD(1'b0), .RSTM(1'b0), .RSTP(rst),
      .RSTALLCARRYIN(1'b0), .RSTALUMODE(1'b0), .RSTCTRL(1'b0), .RSTINMODE(1'b0),
      .ACIN(30'd0), .BCIN(18'd0), .PCIN(48'd0), .CARRYCASCIN(1'b0), .MULTSIGNIN(1'b0),
      .P({p})"""

DSP_PARAMS = """#(.AREG(0), .BREG(0), .CREG(0), .DREG(0), .ADREG(0), .MREG(0), .PREG(1),
             .USE_MULT("MULTIPLY"), .USE_SIMD("ONE48"))"""

def write(name, ports, body):
    with open(os.path.join(OUT, name + ".sv"), "w") as f:
        f.write(body)
    with open(os.path.join(OUT, name + ".ports"), "w") as f:
        f.write(" ".join(ports) + "\n")
    print("wrote", name)

def mac_array(n):
    name = "mac_array_%d" % n
    write(name, ["rst:1", "a_in:27", "b_in:18"], f"""
module {name} (
  input  logic        clk, rst,
  input  logic [26:0] a_in,
  input  logic [17:0] b_in,
  output logic [47:0] p_xor
);
  logic [{n}-1:0][26:0] a_pipe;
  logic [{n}-1:0][17:0] b_pipe;
  logic [47:0] p [{n}];
  always_ff @(posedge clk) begin
    a_pipe[0] <= a_in; b_pipe[0] <= b_in;
    for (int i = 1; i < {n}; i++) begin
      a_pipe[i] <= a_pipe[i-1]; b_pipe[i] <= b_pipe[i-1];
    end
  end
  genvar g;
  generate for (g = 0; g < {n}; g++) begin : lane
    DSP48E2 {DSP_PARAMS} u_dsp (
{DSP_PORTS.format(a="a_pipe[g]", b="b_pipe[g]", p="p[g]")}
    );
  end endgenerate
  always_comb begin
    p_xor = '0;
    for (int i = 0; i < {n}; i++) p_xor ^= p[i];
  end
endmodule
""")

def carry_adders(k, w):
    name = "carry_adders_%dx%d" % (k, w)
    nc = w // 4
    write(name, ["rst:1", "x_in:%d" % w, "y_in:%d" % w], f"""
module {name} (
  input  logic         clk, rst,
  input  logic [{w-1}:0] x_in, y_in,
  output logic [{w-1}:0] s_xor,
  output logic         c_xor
);
  logic [{k}-1:0][{w-1}:0] x_pipe, y_pipe;
  logic [{w-1}:0] o [{k}];
  logic [{w-1}:0] co [{k}];
  logic [{w-1}:0] s [{k}], di [{k}];
  always_ff @(posedge clk) begin
    x_pipe[0] <= x_in; y_pipe[0] <= y_in;
    for (int i = 1; i < {k}; i++) begin
      x_pipe[i] <= x_pipe[i-1]; y_pipe[i] <= y_pipe[i-1];
    end
  end
  genvar g, c;
  generate for (g = 0; g < {k}; g++) begin : adder
    assign s[g]  = x_pipe[g] ^ y_pipe[g];   // propagate
    assign di[g] = x_pipe[g];               // generate data
    for (c = 0; c < {nc}; c++) begin : slice
      if (c == 0) begin : first
        CARRY4 u (.CO(co[g][3:0]), .O(o[g][3:0]), .CI(1'b0), .CYINIT(1'b0),
                  .DI(di[g][3:0]), .S(s[g][3:0]));
      end else begin : rest
        CARRY4 u (.CO(co[g][4*c+3:4*c]), .O(o[g][4*c+3:4*c]), .CI(co[g][4*c-1]),
                  .CYINIT(1'b0), .DI(di[g][4*c+3:4*c]), .S(s[g][4*c+3:4*c]));
      end
    end
  end endgenerate
  logic [{w-1}:0] s_acc; logic c_acc;
  always_comb begin
    s_acc = '0; c_acc = 1'b0;
    for (int i = 0; i < {k}; i++) begin s_acc ^= o[i]; c_acc ^= co[i][{w-1}]; end
  end
  always_ff @(posedge clk) begin s_xor <= rst ? '0 : s_acc; c_xor <= rst ? 1'b0 : c_acc; end
endmodule
""")

def srl_lines(n):
    name = "srl_lines_%d" % n
    write(name, ["rst:1", "d_in:1", "ce:1", "addr:5"], f"""
module {name} (
  input  logic       clk, rst, d_in, ce,
  input  logic [4:0] addr,
  output logic       q_xor, q31_xor
);
  logic [{n}-1:0] d_pipe;
  logic q [{n}], q31 [{n}], qc [{n}];
  always_ff @(posedge clk) begin
    d_pipe[0] <= d_in;
    for (int i = 1; i < {n}; i++) d_pipe[i] <= d_pipe[i-1];
  end
  genvar g;
  generate for (g = 0; g < {n}; g++) begin : line
    // even lanes: dynamic address; odd lanes: static per-lane tap + Q31 cascade
    if (g % 2 == 0) begin : dyn
      SRLC32E u (.Q(q[g]), .Q31(q31[g]), .A(addr), .CE(ce), .CLK(clk), .D(d_pipe[g]));
      assign qc[g] = 1'b0;
    end else begin : casc
      SRLC32E u0 (.Q(q[g]),  .Q31(q31[g]), .A(5'(g % 32)), .CE(ce), .CLK(clk), .D(d_pipe[g]));
      SRLC32E u1 (.Q(qc[g]), .Q31(),       .A(5'(31 - g % 32)), .CE(ce), .CLK(clk), .D(q31[g]));
    end
  end endgenerate
  logic qa, q31a;
  always_comb begin
    qa = 1'b0; q31a = 1'b0;
    for (int i = 0; i < {n}; i++) begin qa ^= q[i] ^ qc[i]; q31a ^= q31[i]; end
  end
  always_ff @(posedge clk) begin q_xor <= rst ? 1'b0 : qa; q31_xor <= rst ? 1'b0 : q31a; end
endmodule
""")

def mixed(n):
    name = "mixed_%d" % n
    w = 32; nc = w // 4
    write(name, ["rst:1", "a_in:27", "b_in:18", "x_in:32", "y_in:32", "ce:1", "addr:5"], f"""
module {name} (
  input  logic        clk, rst, ce,
  input  logic [26:0] a_in,
  input  logic [17:0] b_in,
  input  logic [31:0] x_in, y_in,
  input  logic [4:0]  addr,
  output logic [47:0] p_xor,
  output logic [31:0] s_xor,
  output logic        q_xor
);
  logic [{n}-1:0][26:0] a_pipe;  logic [{n}-1:0][17:0] b_pipe;
  logic [{n}-1:0][31:0] x_pipe, y_pipe;
  logic [47:0] p [{n}];
  logic [31:0] o [{n}], co [{n}], s [{n}];
  logic q [{n}];
  always_ff @(posedge clk) begin
    a_pipe[0] <= a_in; b_pipe[0] <= b_in; x_pipe[0] <= x_in; y_pipe[0] <= y_in;
    for (int i = 1; i < {n}; i++) begin
      a_pipe[i] <= a_pipe[i-1]; b_pipe[i] <= b_pipe[i-1];
      x_pipe[i] <= x_pipe[i-1]; y_pipe[i] <= y_pipe[i-1];
    end
  end
  genvar g, c;
  generate for (g = 0; g < {n}; g++) begin : lane
    DSP48E2 {DSP_PARAMS} u_dsp (
{DSP_PORTS.format(a="a_pipe[g]", b="b_pipe[g]", p="p[g]")}
    );
    assign s[g] = x_pipe[g] ^ y_pipe[g];
    for (c = 0; c < {nc}; c++) begin : slice
      if (c == 0) begin : first
        CARRY4 u (.CO(co[g][3:0]), .O(o[g][3:0]), .CI(1'b0), .CYINIT(1'b0),
                  .DI(x_pipe[g][3:0]), .S(s[g][3:0]));
      end else begin : rest
        CARRY4 u (.CO(co[g][4*c+3:4*c]), .O(o[g][4*c+3:4*c]), .CI(co[g][4*c-1]),
                  .CYINIT(1'b0), .DI(x_pipe[g][4*c+3:4*c]), .S(s[g][4*c+3:4*c]));
      end
    end
    // the SRL delays the adder's carry-out; its read tap is data dependent
    SRLC32E u_srl (.Q(q[g]), .Q31(), .A(addr ^ o[g][4:0]), .CE(ce), .CLK(clk), .D(co[g][31]));
  end endgenerate
  logic [47:0] pa; logic [31:0] sa; logic qa;
  always_comb begin
    pa = '0; sa = '0; qa = 1'b0;
    for (int i = 0; i < {n}; i++) begin pa ^= p[i]; sa ^= o[i]; qa ^= q[i]; end
  end
  always_ff @(posedge clk) begin
    p_xor <= rst ? '0 : pa; s_xor <= rst ? '0 : sa; q_xor <= rst ? 1'b0 : qa;
  end
endmodule
""")

for n in (4, 16, 64, 256):
    mac_array(n)
for k, w in ((4, 32), (16, 64), (64, 64), (256, 64)):
    carry_adders(k, w)
for n in (8, 32, 128, 512):
    srl_lines(n)
for n in (4, 16, 64):
    mixed(n)
