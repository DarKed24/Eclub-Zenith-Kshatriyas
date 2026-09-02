# Part A — Yosys verification runbook (Yosys 0.68, WSL)

Step-by-step commands to build the Part A DSP48E2 frontend through Yosys and
prove it correct, with the exact output each step should produce. Every command
and every transcript below was executed against **Yosys 0.68 (git sha1
38e001a6f)** on WSL2 Ubuntu-24.04; the outputs are copied from that run, not
reconstructed.

What this runbook establishes, in order:

| Step | Claim being checked |
|---|---|
| 1 | The macro datapath itself is right (CPU unit + differential tests). |
| 2 | A design with a `GEM_DSP48E2` in it survives the aigpdk mapping flow. |
| 3 | That gate-level netlist is functionally identical to the source RTL. |
| 4 | `gemmacro_map.v` rewrites a real `DSP48E2` into `GEM_DSP48E2`. |
| 5 | The rewrite preserves the *arithmetic*, not just the cell name. |
| 6 | Unsupported DSP configurations are rejected with a clear `$error`. |
| 7 | Path B (inference from plain RTL) still does not work on 0.68. |
| 8 | GEM's own simulator reads the Yosys netlist and matches Icarus on it. |

Steps 2–8 are the Yosys part. Step 1 is a two-minute prerequisite: if the
substrate is broken there is no point looking at the frontend.

---

## Step 0. Environment

Everything runs inside WSL. Set three variables once per shell:

```bash
export GEM=/mnt/c/Users/Devansh/GEM
export YOSYS=$HOME/yosys/build/yosys
export PATH=$HOME/oss-cad-suite/bin:$PATH      # for iverilog
export CARGO_TARGET_DIR=$HOME/gem-target       # keep Linux builds off the Windows target/
```

> **Use `$HOME/yosys/build/yosys`, never `/usr/bin/yosys`.** The pinned version
> for this project is 0.68, built from source. If a distro Yosys (0.33) is
> installed it will silently give different answers in Steps 4 and 7.

Create a scratch working directory and copy the fixtures in:

```bash
mkdir -p ~/gemA/out ~/gemA/log && cd ~/gemA
cp $GEM/tests/yosys/parta/*.v $GEM/tests/yosys/parta/*.py .
$YOSYS -V
iverilog -V | head -1
```

Expected:

```
Yosys 0.68 (git sha1 38e001a6f, GNU /usr/bin/c++ 13.3.0)
Icarus Verilog version 14.0 (devel) (s20260301-391-g64f13540a-dirty)
```

If `$YOSYS -V` prints anything other than **0.68**, stop — the rest of this
document does not describe what you will see.

The fixtures you just copied are:

| File | Role |
|---|---|
| `mac_top.v` | Path A1 design: an 8×8 MAC around a hand-instantiated `GEM_DSP48E2` plus RTL glue. |
| `dsp_static.v` | Path A2: `PREG=1` `DSP48E2` with `OPMODE = 9'h025` (accumulate) and `9'h005` (multiply-only). |
| `dsp_dyn.v` | Path A2 with a data-dependent `OPMODE`, plus the two configurations that must be rejected. |
| `dsp_rtl.v` | Path B: plain `p <= p + a*b` accumulator RTL, in three shapes. |
| `tb_a1.v`, `tb_opmode.v`, `tb_e2e.v` | Icarus testbenches for Steps 3, 5 and 8. |
| `mkstim.py`, `cmpvcd.py` | Stimulus generator and VCD comparator for Step 8. |

---

## Step 1. Baseline: the macro datapath on CPU

Before touching Yosys, confirm the substrate the frontend feeds.

```bash
cd $GEM
cargo test
```

Expected — 18 library unit tests, of which 9 are the DSP48E2 datapath
(`opmode_states`, `pre_adder_wraps_at_27_bits`, `accumulator_wraps_at_48_bits`,
`rstp_zeroes_output`, `signed_extremes`, `use_d_pre_adder`,
`negative_accumulator_feedback`, `input_bit_slots_are_a_permutation`,
`packing_roundtrips_every_bit`) and the remainder cover Parts B and C:

```
     Running unittests src/lib.rs (...)
running 18 tests
test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

     Running unittests src/bin/macro_test.rs (...)
running 14 tests
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Then the end-to-end differential harness:

```bash
cargo run -q --bin macro_test 2>&1 | grep -E '^\[|all differential'
```

Expected (Part A lines shown; the `carry`/`srl`/`heterogeneous` lines belong to
Parts B and C and must also pass):

```
[simple_dff] 9 aigpins, 9 endpoint groups, 0 macro(s), 1 major stage(s), reg/io state 130 words, blocks_data hash 4367312310778971198
[simple_dff] differential check passed over 256 cycles
[mac_accumulator] 95 aigpins, 21 endpoint groups, 1 macro(s), 1 major stage(s), reg/io state 132 words, blocks_data hash 1388458641175822285
[mac_accumulator] differential check passed over 512 cycles
...
macro_test: all differential checks passed
```

Two things to actually read here, not just skim:

- `[simple_dff] ... blocks_data hash 4367312310778971198` — this is the pinned
  macro-free script hash. It proves Part A did not perturb the script ABI for
  designs with no macro in them. A changed hash means a regression in
  `flatten.rs` / `pe.rs`, not a frontend problem.
- `[mac_accumulator] ... 1 macro(s)` — one `GEM_DSP48E2` became one macro
  endpoint, and 512 random cycles agree bit-exactly with an independent
  behavioural evaluator.

---

## Step 2. Path A1 — a `GEM_DSP48E2` through the aigpdk flow

This is the flow `usage.md` Step 1.5 Path A + Step 2 describe: instantiate the
macro directly, then run the ordinary aigpdk logic-mapping script over it. The
question is whether the macro survives synthesis intact while the surrounding
1-bit logic maps to aigpdk cells.

`mac_top.v` is written so the answer is checkable by inspection — it is the
RTL that should produce exactly the hand-written fixture
`tests/data/mac_accumulator.gv`:

```verilog
  GEM_DSP48E2 dsp (
    .CLK(clk), .CEP(1'b1), .RSTP(rst), .USE_D(1'b0),
    .A({19'b0, a_reg}), .D(27'b0),
    .B({10'b0, b_reg}), .C(48'b0),
    .OPMODE_S({~load, load}), .P(p_int)
  );
```

Write the script and run it:

```bash
cd ~/gemA
cat > a1.ys <<EOF
read_verilog $GEM/aigpdk/gemmacro.v
read_verilog mac_top.v
hierarchy -check -top mac_top

synth -flatten
delete t:\$print

dfflibmap -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge
abc -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge
techmap
abc -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge

stat
write_verilog -noattr out/mac_top.gv
EOF

$YOSYS -q -l log/a1.log a1.ys
awk '/Printing statistics/{n++} n==2' log/a1.log | head -20
```

Expected — the **final** `stat` (there are two in the log; the first is from
inside `synth`, before liberty mapping, and still shows `$_ANDNOT_` / `$_DFF_P_`
cells):

```
=== mac_top ===

   ...
       30 cells
        2   AND2_00_0
        8   AND2_01_0
        1   AND2_11_1
       16   DFF
        1   GEM_DSP48E2
        2   INV
```

That cell mix is the pass criterion, and it is worth checking digit by digit:

- **`1 GEM_DSP48E2`** — the macro came through `synth -flatten`, two `abc` runs
  and `opt_clean -purge` untouched. It is a `(* blackbox *)` in `gemmacro.v`, so
  Yosys never looks inside it and never shreds the multiplier into gates.
- **`16 DFF`** — the two 8-bit input registers, mapped by `dfflibmap`. The `P`
  accumulator is *not* here: it lives inside the macro. Seeing 64 DFFs instead
  of 16 would mean the macro got flattened and `P` fell out into fabric.
- **`2 AND2_00_0`, `8 AND2_01_0`, `1 AND2_11_1`, `2 INV`** — the glue. This is
  cell-for-cell what `tests/data/mac_accumulator.gv` contains by hand, so the
  Yosys flow reproduces the already-verified fixture.

Now look at how the macro is wired in the output netlist:

```bash
grep -n 'GEM_DSP48E2' -A 12 out/mac_top.gv
```

Expected:

```verilog
  GEM_DSP48E2 dsp (
    .A({ 19'h00000, a_reg }),
    .B({ 10'h000, b_reg }),
    .C(48'h000000000000),
    .CEP(1'h1),
    .CLK(clk),
    .D(27'h0000000),
    .OPMODE_S({ _01_, load }),
    .P(p_int),
    .RSTP(rst),
    .USE_D(1'h0)
  );
```

Note there is **no parameter list** on the instance. That matters: GEM's netlist
reader does not support instance parameters at all, and a `GEM_DSP48E2 #(...)`
would fail Step 8 with an `NL_SV_PARSE` error.

---

## Step 3. Path A1 — is the netlist actually equivalent to the RTL?

Step 2 only counted cells. This step proves the mapped netlist computes the same
thing as the source RTL, by simulating both against each other. `tb_a1.v`
instantiates the RTL `mac_top` and the synthesized netlist side by side, drives
512 random cycles (including random `rst` and `load`), and compares all four
primary outputs every cycle.

The gate netlist has the same module name as the RTL, so rename the copy used
for the comparison:

```bash
cd ~/gemA
sed 's/^module mac_top(/module mac_top_gate(/' out/mac_top.gv > out/mac_top_gate.v

iverilog -g2005 -o sim_a1 \
  $GEM/aigpdk/aigpdk.v \
  $GEM/aigpdk/gemmacro_behav.v \
  mac_top.v out/mac_top_gate.v tb_a1.v
./sim_a1
```

Expected:

```
PASS: 512 cycles, RTL and aigpdk gate netlist agree bit-exactly
tb_a1.v:39: $finish called at 5147000 (1ps)
```

Two details about the file list:

- `gemmacro_behav.v`, not `gemmacro.v`. They are the same module, but
  `gemmacro.v` carries the `(* blackbox *)` attribute for Yosys and has no
  simulatable body from Icarus's point of view. `gemmacro_behav.v` is the
  simulation copy.
- `aigpdk.v` supplies the `AND2_*` / `INV` / `DFF` cell models the netlist
  instantiates.

---

## Step 4. Path A2 — `DSP48E2` → `GEM_DSP48E2` via techmap

This is the other half of `usage.md` Path A: the design instantiates a real
Xilinx `DSP48E2` primitive with `PREG=1`, and `gemmacro_map.v` rewrites it.

The interesting part is the `OPMODE` decode. `gemmacro_map.v` writes it as plain
combinational logic, so a constant `OPMODE` must **fold** to a constant 2-bit
`OPMODE_S`, and a data-dependent one must **synthesise into gates** feeding the
macro's `OPMODE_S` port.

### 4a. Static `OPMODE = 9'h025` → accumulate

```bash
cd ~/gemA
cat > a2_static.ys <<EOF
read_verilog -lib +/xilinx/cells_xtra.v
read_verilog $GEM/aigpdk/gemmacro.v
read_verilog dsp_static.v
hierarchy -check -top dsp_static
proc;;
opt_expr; opt_clean
techmap -map $GEM/aigpdk/gemmacro_map.v
opt_expr -fine; opt_clean
stat
write_verilog -noattr out/dsp_static_mapped.v
EOF

$YOSYS -q -l log/a2_static.log a2_static.ys
sed -n '/=== dsp_static ===/,/Executing Verilog backend/p' log/a2_static.log
grep -n 'OPMODE_S' out/dsp_static_mapped.v
```

Expected — one cell, and nothing else left over:

```
=== dsp_static ===
   ...
        1 cells
        1   GEM_DSP48E2
```
```
    .OPMODE_S(2'h2),
```

`OPMODE = 9'h025` means X-mux `01`, Y-mux `01`, Z-mux `010`, i.e. `P <= P + A*B`.
`OPMODE_S` folding to the constant `2'h2` is the map file agreeing.

`read_verilog -lib +/xilinx/cells_xtra.v` is required — it supplies the
`DSP48E2` blackbox declaration so `hierarchy -check` does not fail on an unknown
module. It takes ~3 s; that is the whole runtime of this step.

### 4b. Static `OPMODE = 9'h005` → multiply-only

Same script with `-top dsp_static_mult` (that module is in the same
`dsp_static.v`):

```bash
sed 's/dsp_static$/dsp_static_mult/; s/dsp_static_mapped/dsp_static_mult_mapped/' \
    a2_static.ys > a2_mult.ys
$YOSYS -q -l log/a2_mult.log a2_mult.ys
grep -n 'OPMODE_S' out/dsp_static_mult_mapped.v
```

Expected — Z-mux `000` means `P <= A*B`:

```
    .OPMODE_S(2'h1),
```

### 4c. Data-dependent `OPMODE`

`dsp_dyn.v` drives `OPMODE` from a `load` input (`load ? 9'h005 : 9'h025`).

```bash
cat > a2_dyn.ys <<EOF
read_verilog -lib +/xilinx/cells_xtra.v
read_verilog $GEM/aigpdk/gemmacro.v
read_verilog dsp_dyn.v
hierarchy -check -top dsp_dyn
proc;;
opt_expr; opt_clean
techmap -map $GEM/aigpdk/gemmacro_map.v
opt_expr -fine; opt_clean
stat
write_verilog -noattr out/dsp_dyn_mapped.v
EOF
$YOSYS -q -l log/a2_dyn.log a2_dyn.ys
sed -n '/=== dsp_dyn ===/,/Executing Verilog backend/p' log/a2_dyn.log
```

Expected — the macro plus a handful of ordinary cells that will become AIG
gates in Step 2's flow:

```
=== dsp_dyn ===
   ...
       11 cells
        3   $eq
        3   $logic_and
        1   $logic_not
        3   $mux
        1   GEM_DSP48E2
```

This is the heterogeneous case the design is built for: 1-bit control logic
computed in the AIG feeding a word-level macro input. `grep OPMODE_S
out/dsp_dyn_mapped.v` should show a wire (`.OPMODE_S(_07_)`), not a constant.

---

## Step 5. Path A2 — does the rewrite preserve the arithmetic?

Step 4 checked that `OPMODE_S` folded to the expected number. That is a claim
about the decode table; it is not a claim that the result computes a MAC.
`tb_opmode.v` closes that by comparing the techmapped `dsp_static` against a
plain RTL golden model with the same enable/reset rules (`RSTP` beats `CEP`),
over 1000 random signed vectors:

```bash
cd ~/gemA
iverilog -g2005 -o sim_opmode \
  $GEM/aigpdk/gemmacro_behav.v out/dsp_static_mapped.v tb_opmode.v
./sim_opmode
```

Expected:

```
PASS: 1000 random vectors, techmapped DSP48E2 == golden MAC
tb_opmode.v:55: $finish called at 10027000 (1ps)
```

The golden model is deliberately written the obvious way:

```verilog
  always @(posedge clk)
    if (rst)      p <= 48'sd0;
    else if (ce)  p <= p + a * b;
```

so a `PASS` means the whole chain — `OPMODE` decode, `USE_D` derivation,
`RSTP`-over-`CEP` precedence, 48-bit wrapping — behaves as an ordinary
accumulator would.

---

## Step 6. Rejection guards

`gemmacro_map.v` accepts exactly one DSP configuration and must refuse the rest
loudly rather than mapping something it will simulate wrongly. Both rejected
modules live in `dsp_dyn.v`.

```bash
cd ~/gemA
for top in dsp_preg0 dsp_areg1; do
  cat > rej_$top.ys <<EOF
read_verilog -lib +/xilinx/cells_xtra.v
read_verilog $GEM/aigpdk/gemmacro.v
read_verilog dsp_dyn.v
hierarchy -check -top $top
proc;;
techmap -map $GEM/aigpdk/gemmacro_map.v
EOF
  echo "---------- $top"
  $YOSYS -q rej_$top.ys; echo "exit status = $?"
done
```

Expected — **a non-zero exit status is the passing result here**:

```
---------- dsp_preg0
/mnt/c/Users/Devansh/GEM/aigpdk/gemmacro_map.v:124: ERROR: GEM: DSP48E2 has PREG=0 - GEM only maps a *registered* MAC. .
exit status = 1
---------- dsp_areg1
/mnt/c/Users/Devansh/GEM/aigpdk/gemmacro_map.v:133: ERROR: GEM: DSP48E2 has a non-zero AREG/BREG/CREG/DREG/ADREG/MREG. .
exit status = 1
```

(Yosys prints only the first string argument of a multi-argument `$error`, which
is why each message ends in a stray ` .` — the rest of the guidance is in the
map file at the quoted line.)

If either of these *succeeds* and emits a `GEM_DSP48E2`, that is a real bug:
the earlier version of `gemmacro_map.v` defaulted its register parameters to
`1`, which made Yosys fire both `$error`s while elaborating the map file itself
— before matching any instance — so no DSP could ever map. The defaults are now
the accepted configuration (`PREG = 1`, everything else `0`) precisely so that
techmap re-elaborates with real instance parameters and these checks fire only
for genuinely unsupported cells. Step 4 passing and Step 6 failing are the two
halves of that fix; you need both.

---

## Step 7. Path B — inference from plain RTL (expected to *not* work)

This step documents a known limitation rather than a feature. Run it so you
notice if a future Yosys changes the answer.

The claim under test: does `xilinx_dsp` fold `p <= p + a*b` into a `DSP48E2`
with `PREG=1` and the post-adder, which `gemmacro_map.v` would then accept?

```bash
cd ~/gemA
cat > b_manual.ys <<EOF
read_verilog -lib +/xilinx/cells_xtra.v
read_verilog $GEM/aigpdk/gemmacro.v
read_verilog dsp_rtl.v
hierarchy -check -top dsp_rtl
proc;;
opt_expr; opt_dff; opt_clean
memory_dff
techmap -map +/mul2dsp.v -map +/xilinx/xcu_dsp_map.v -D DSP_A_MAXWIDTH=27 -D DSP_B_MAXWIDTH=18 -D DSP_A_MINWIDTH=2 -D DSP_B_MINWIDTH=2 -D DSP_NAME=\$__MUL27X18
select a:mul2dsp
setattr -unset mul2dsp
select -clear
opt_expr -fine
wreduce
xilinx_dsp -family xcup
chtype -set \$mul t:\$__soft_mul
opt_clean
stat
write_verilog -noattr out/dsp_rtl_manual.v
EOF

$YOSYS -q -l log/b_manual.log b_manual.ys
sed -n '/=== dsp_rtl ===/,/Executing Verilog backend/p' log/b_manual.log
grep -n '\.PREG' out/dsp_rtl_manual.v
```

Expected on 0.68 — the adder and the accumulator flop stay in fabric:

```
=== dsp_rtl ===
   ...
        3 cells
        1   $add
        1   $dff
        1   DSP48E2
```
```
    .PREG(32'sd0),
```

`$add` + `$dff` next to the DSP is the whole story: `xilinx_dsp` ran and packed
nothing. Feeding that into the map file gives the Step 6 error, correctly:

```bash
printf 'script b_manual.ys\ntechmap -map %s/aigpdk/gemmacro_map.v\n' "$GEM" > b_map.ys
$YOSYS -q b_map.ys; echo "exit status = $?"
```

```
/mnt/c/Users/Devansh/GEM/aigpdk/gemmacro_map.v:124: ERROR: GEM: DSP48E2 has PREG=0 - GEM only maps a *registered* MAC. .
exit status = 1
```

The stock flow does no better:

```bash
cat > b_synth.ys <<EOF
read_verilog -lib +/xilinx/cells_xtra.v
read_verilog dsp_rtl.v
hierarchy -check -top dsp_rtl
synth_xilinx -family xcup -flatten
stat
write_verilog -noattr out/dsp_rtl_xcup.v
EOF
$YOSYS -q -l log/b_synth.log b_synth.ys
sed -n '/=== dsp_rtl ===/,/design hierarchy/p' log/b_synth.log
grep -n '\.PREG' out/dsp_rtl_xcup.v
```

Expected — a DSP plus 48 `FDRE` and 12 `CARRY4` doing the accumulate in fabric:

```
        1   DSP48E2
       46   IBUF
       48   LUT2
       48   OBUF
       60 submodules
       12   CARRY4
       48   FDRE
```
```
    .PREG(32'sd0),
```

**Conclusion, unchanged from 0.33 to 0.68:** Path B does not work with stock
Yosys, for `p <= p + a*b`, `p <= c + a*b`, or even a plain registered
`p <= a*b`, through either the manual sequence or `synth_xilinx -family
{xcup,xcu,xc7}`. Use Path A (Steps 2–5). Closing this properly means either
patching `xilinx_dsp` or adding a GEM-side pattern match that folds the fabric
adder and flop into the macro.

Two incidental facts this step also pins, both easy to get wrong:
`+/xilinx/xcup_dsp_map.v` **does not exist** (the UltraScale+ file is
`+/xilinx/xcu_dsp_map.v`), and `alumacc` must not run before `mul2dsp` — it
folds `a*b + acc` into a `$macc` cell that `mul2dsp` cannot see.

---

## Step 8. End to end — GEM reads the Yosys netlist

The last question: does GEM's own frontend consume the netlist Yosys produced in
Step 2, recognise the macro, and simulate it correctly? This drives the same
stimulus through GEM's `naive_sim` and through Icarus on the same netlist, and
diffs them.

```bash
cd ~/gemA
python3 mkstim.py 256
```

```
wrote stim.vcd and stim.txt (256 cycles)
```

`mkstim.py` writes one set of vectors in two formats: `stim.vcd` for GEM,
`stim.txt` for `$readmemb` in the testbench. Inputs change on the falling edge
and the clock rises between them, so neither simulator sees a data/clock race.

Icarus reference first:

```bash
iverilog -g2005 -o sim_e2e \
  $GEM/aigpdk/aigpdk.v $GEM/aigpdk/gemmacro_behav.v \
  out/mac_top_gate.v tb_e2e.v
./sim_e2e
```

```
iverilog: wrote 256 cycles to iv_out.txt
tb_e2e.v:43: $finish called at 2560000 (1ps)
```

Then GEM:

```bash
$CARGO_TARGET_DIR/debug/naive_sim out/mac_top.gv stim.vcd naive.vcd
```

(Build it first with `cd $GEM && cargo build --bins` if it is not there.) The
line to look for in the log noise is:

```
INFO  clock ports detected: clk
```

Then compare:

```bash
python3 cmpvcd.py naive.vcd iv_out.txt
```

Expected:

```
PASS: 255 cycles compared (1 skipped as x), GEM naive_sim == Icarus on all 4 outputs
```

**About the skipped cycle and the sampling convention** — both are expected, and
worth understanding before you debug a spurious failure here:

- Cycle 0 is skipped because Verilog starts registers at `x` while GEM starts
  them at `0`. `cmpvcd.py` drops any reference line containing `x`, so exactly
  one cycle is skipped.
- `tb_e2e.v` samples the outputs **just before** each rising edge, not after.
  GEM's `naive_sim` timestamps the *pre-edge* state at a clock edge — the same
  `Q` convention its DFF and macro outputs follow throughout. Sampling after the
  edge instead makes every cycle mismatch by a one-cycle shift: the run before
  this convention was aligned reported `FAIL: 204 mismatching cycles out of 256`
  purely from that offset. If you see a wholesale failure here, check the offset
  before suspecting the macro.

---

## Step 9. Run everything at once

All of the above is scripted:

```bash
GEM=/mnt/c/Users/Devansh/GEM YOSYS=$HOME/yosys/build/yosys \
  bash $GEM/tests/yosys/parta/run_all.sh
```

It builds a fresh work directory (`~/gemA-run` by default), runs steps 1–8, and
exits non-zero on the first check that does not produce the expected result.
Full expected output:

```
=== 0. environment ===
Yosys 0.68 (git sha1 38e001a6f, GNU /usr/bin/c++ 13.3.0)
Icarus Verilog version 14.0 (devel) (s20260301-391-g64f13540a-dirty)

=== 1. CPU substrate (cargo) ===
  [ok]   cargo test
  [ok]   macro_test differential

=== 2. Path A1 - hand-instantiated GEM_DSP48E2 -> aigpdk netlist ===
  [ok]   GEM_DSP48E2 survives synthesis
  [ok]   16 DFF + aigpdk glue mapped

=== 3. Path A1 - RTL vs gate netlist (iverilog) ===
  [ok]   512 cycles bit-exact

=== 4. Path A2 - DSP48E2 -> GEM_DSP48E2 techmap ===
  [ok]   dsp_static -> OPMODE_S folded to 2'h2
  [ok]   dsp_static_mult -> OPMODE_S folded to 2'h1
  [ok]   dsp_dyn -> macro + gates driving OPMODE_S

=== 5. Path A2 - techmapped arithmetic vs golden MAC (iverilog) ===
  [ok]   1000 random vectors

=== 6. rejection guards ===
  [ok]   dsp_preg0 rejected with the PREG=0 error
  [ok]   dsp_areg1 rejected with the non-zero AREG error

=== 7. Path B - RTL inference (expected NOT to fold on 0.68) ===
  [ok]   xilinx_dsp leaves PREG=0 (documented 0.68 limitation)

=== 8. end-to-end - GEM naive_sim vs iverilog on the same netlist ===
PASS: 255 cycles compared (1 skipped as x), GEM naive_sim == Icarus on all 4 outputs
  [ok]   GEM == Icarus over 255 cycles

ALL PART A YOSYS CHECKS PASSED  (workdir: /home/devansh/gemA-run)
```

---

## Troubleshooting

| Symptom | Cause |
|---|---|
| `ERROR: Module ... DSP48E2 not found` | Missing `read_verilog -lib +/xilinx/cells_xtra.v` before `hierarchy`. |
| Both `$error`s fire on *every* techmap, even for a valid DSP | `gemmacro_map.v` register-parameter defaults are wrong again. Yosys elaborates a map module once with defaults before matching anything, so the defaults must be the accepted configuration. |
| `stat` after Step 2 shows 64 DFFs and no `GEM_DSP48E2` | The `(* blackbox *)` attribute was lost — you read `gemmacro_behav.v` (the simulation copy) instead of `gemmacro.v`. |
| Step 4 emits `GEM_DSP48E2` but `OPMODE_S` is an unexpected constant | Your `OPMODE` is not one of the three modelled patterns. Anything that is not X=`01`,Y=`01` with Z=`010`/`000` decodes to `OPMODE_S=0` (`P <= C`) by design, not by error. |
| `NL_SV_PARSE` error from `naive_sim` | The netlist has instance parameters on a cell. GEM's reader does not support them; strip with `setparam -unset` or `write_verilog -noattr`. |
| Step 8 fails on nearly every cycle | Sampling-convention offset, not a datapath bug — see the note at the end of Step 8. |
| Step 7 *passes* the fold (a `PREG=1` DSP) | Good news, not a failure: your Yosys now supports Path B. Re-run Step 4's techmap on its output and update `usage.md`. |
| Answers differ from this document | Check `$YOSYS -V`. A distro Yosys 0.33 at `/usr/bin/yosys` gives different results in Steps 4 and 7. |

---

## What this runbook does *not* cover

- **The GPU numeric differential.** `cuda_test --check-with-cpu` on
  `tests/data/mac_accumulator.gv`, plus a `compute-sanitizer` pass over the new
  `__shfl_sync` gather and the 64-bit `P` load, needs `cargo build --features
  cuda` and a GPU. `csrc/` compiles clean under `nvcc` 12.6 (`sm_86`, 0 warnings,
  0 register spill) but the numeric run is open. The specific risk it would
  close: the kernel applies `writeout_inv ^= c3` to the macro `P` words *after*
  storing them while the emulator does not, so the two agree only because
  `place_clken_datainv(…, 0)` forces `c3 == 0` for those bit positions.
- **Wide multipliers.** `mul2dsp` splits anything larger than 27×18 into a
  cascade of partial products; the `DSP48E2 -> GEM_DSP48E2` rewrite of a cascade
  is untested.
- **Parts B and C.** `CARRY4` and `SRLC32E` have their own flows in `usage.md`
  Steps 1.6 and 1.7. `macro_test` in Step 1 covers them on the CPU side.
