# Part B — Yosys verification runbook (Yosys 0.68, WSL)

Step-by-step commands to build the Part B **CARRY4 carry-chain** frontend
through Yosys and prove it correct, with the exact output each step should
produce. Every command and every transcript below was executed against
**Yosys 0.68 (git sha1 38e001a6f)** on WSL2 Ubuntu-24.04; the outputs are copied
from that run, not reconstructed.

What this runbook establishes, in order:

| Step | Claim being checked |
|---|---|
| 1 | The CARRY4 datapath itself is right (CPU unit + differential tests). |
| 2 | The premise holds: `gemcarry.v` elaborates and 0.68 still emits real `CARRY4`. |
| 3 | Path A — a hand-instantiated `CARRY4` survives the aigpdk mapping flow. |
| 4 | That gate-level netlist is functionally identical to the source RTL. |
| 5 | Path B — plain `a + b + cin` RTL *infers* `CARRY4` end-to-end into aigpdk. |
| 6 | Why the recipe uses generic `synth` and not `synth_xilinx`. |
| 7 | `gemcarry_map.v` splits a `CARRY8` into two chained `CARRY4`, arithmetic intact. |
| 8 | GEM reads both netlists and **fuses** the cascade into one macro. |
| 9 | GEM's own simulator matches Icarus on the Yosys netlist, cycle for cycle. |
| 10 | The GPU kernel agrees with the CPU on that same netlist (optional). |

Steps 2–10 are the Yosys part. Step 1 is a two-minute prerequisite: if the
substrate is broken there is no point looking at the frontend.

Part B's frontend story is materially better than Part A's. For the DSP,
inference from plain RTL does **not** work on 0.68 (`xilinx_dsp` will not fold
the accumulator into `PREG`), so only explicit instantiation is validated. For
CARRY4, **both** paths work: Step 3 maps a hand-written `CARRY4`, and Step 5
takes `sum <= a + b + cin` all the way to native carry macros with no vendor
primitive anywhere in the source.

---

## Step 0. Environment

Everything runs inside WSL. Set the variables once per shell:

```bash
export GEM=/mnt/c/Users/Devansh/GEM
export YOSYS=$HOME/yosys/build/yosys
export PATH=$HOME/oss-cad-suite/bin:$PATH      # for iverilog
export CARGO_TARGET_DIR=$HOME/gem-target       # keep Linux builds off the Windows target/
```

> **Use `$HOME/yosys/build/yosys`, never `/usr/bin/yosys`.** The pinned version
> for this project is 0.68, built from source. If a distro Yosys (0.33) is
> installed it will give different answers — in particular 0.33 needs **5**
> `CARRY4` for the Step 5 design where 0.68 needs 4.

Create a scratch working directory and copy the fixtures in:

```bash
mkdir -p ~/gemB/out ~/gemB/log && cd ~/gemB
cp $GEM/tests/yosys/partb/*.v $GEM/tests/yosys/partb/*.py .
$YOSYS -V
iverilog -V 2>/dev/null | head -1
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
| `carry_top.v` | Path A design (`yadder`): an 8-bit registered adder around two hand-instantiated `CARRY4` slices chained `CO[3] -> CI`. |
| `add_rtl.v` | Path B: plain `sum <= a + b + cin`, at 16 bits (`add_rtl`) and 8 bits (`add_rtl8`). No vendor primitive. |
| `carry8_top.v` | An explicitly instantiated `CARRY8`, the only way to reach the `gemcarry_map.v` split rule on 0.68. |
| `tb_b1.v`, `tb_carry8.v`, `tb_e2e.v` | Icarus testbenches for Steps 4, 7 and 9. |
| `mkstim.py`, `cmpvcd.py` | Stimulus generator and VCD comparator for Step 9. |
| `run_all.sh` | Runs Steps 1–10 unattended (Step 11). |

The GEM-side files under test are `aigpdk/gemcarry.v` (the `CARRY4` blackbox +
behavioural model), `aigpdk/gemcarry_map.v` (`CARRY8` → 2× `CARRY4`), the
`CARRY4` cell in `aigpdk/aigpdk.lib`, and `usage.md` Step 1.6.

---

## Step 1. Baseline: the CARRY4 datapath on CPU

Before touching Yosys, confirm the substrate the frontend feeds.

```bash
cd $GEM
cargo test
```

Expected — 18 library unit tests, of which **four** are the CARRY4 datapath
(`carry4_single_slice_exhaustive_s`, `carry4_ripple_adder_matches_u64_add`,
`carry4_prefix_matches_reference_random`, `carry4_packing_roundtrips_every_bit`)
and the remainder cover Parts A and C, plus 14 `macro_test` integration tests:

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

Expected (Part B lines shown; the `mac_accumulator`/`srl_*`/`heterogeneous`
lines belong to Parts A and C and must also pass):

```
[simple_dff] 9 aigpins, 9 endpoint groups, 0 macro(s), 1 major stage(s), reg/io state 130 words, blocks_data hash 4367312310778971198
[simple_dff] differential check passed over 256 cycles
...
[ripple_adder] 67 aigpins, 9 endpoint groups, 1 macro(s), 2 major stage(s), reg/io state 259 words, blocks_data hash 2711449478675204136
[ripple_adder] differential check passed over 400 cycles
[carry_then_logic_then_carry] 51 aigpins, 4 endpoint groups, 2 macro(s), 3 major stage(s), reg/io state 389 words, blocks_data hash 17676642068319116734
[carry_then_logic_then_carry] differential check passed over 400 cycles
[mixed_macros] 98 aigpins, 5 endpoint groups, 2 macro(s), 2 major stage(s), reg/io state 264 words, blocks_data hash 9165969952107539029
[mixed_macros] differential check passed over 400 cycles
[yadder] 75 aigpins, 9 endpoint groups, 1 macro(s), 2 major stage(s), reg/io state 259 words, blocks_data hash 2719296911916927336
[yadder] differential check passed over 400 cycles
...
[structural] Class C staging checks passed
macro_test: all differential checks passed
```

Four things to actually read here, not just skim:

- `[simple_dff] ... hash 4367312310778971198` — the pinned macro-free script
  hash. It proves Part B did not perturb the script ABI for designs with no
  macro in them. A changed hash is a regression in `flatten.rs` / `pe.rs`, not a
  frontend problem.
- `[ripple_adder] ... 1 macro(s), 2 major stage(s)` — two `CARRY4` slices fused
  into **one** `Carry4 { chain_len: 2 }` endpoint. Class C macros always route
  through staged-IO scratch, so one macro level costs one forced split, hence 2
  stages, not 1.
- `[carry_then_logic_then_carry] ... 2 macro(s), 3 major stage(s)` — the
  macro → logic → macro fixpoint. The two CARRY4s deliberately do *not* fuse
  (`CI` is tied low on the second), so they land at different macro levels.
- `[yadder] ... hash 2719296911916927336` — this is the Yosys-frontend
  regression case. Step 3 regenerates exactly that netlist.

---

## Step 2. Premise: does Yosys 0.68 still give us a `CARRY4` to catch?

Two things have to be true before either path can work, and both are things a
Yosys upgrade can silently break.

**2a. The blackbox model elaborates.**

```bash
cd ~/gemB
$YOSYS -q -p "read_verilog $GEM/aigpdk/gemcarry.v; hierarchy; stat"
echo "exit status = $?"
```

Expected — **no output at all**, and exit 0:

```
exit status = 0
```

That is the pass: under `-q`, a blackbox module that parses and elaborates
cleanly has nothing to report, and `stat` finds no cells to count inside it.
Any diagnostic here means `gemcarry.v` is out of step with the installed Yosys.

**2b. `arith_map.v` still emits a real `CARRY4`, not a `$lut` carry.** This is
the whole premise of Path B — Yosys targets with `LUT_SIZE == 4` get a different
carry structure, and a future release could change the default.

```bash
grep -n "CARRY4" $($YOSYS-config --datdir)/xilinx/arith_map.v | head -5
```

```
56:	localparam CARRY4_COUNT = (WIDTH + 3) / 4;
57:	localparam MAX_WIDTH    = CARRY4_COUNT * 4;
68:	generate for (i = 0; i < CARRY4_COUNT; i = i + 1) begin:slice
70:			CARRY4 carry4
79:			CARRY4 carry4
```

And confirm it fires on a real `$alu`:

```bash
$YOSYS -q -l log/premise.log -p "
read_verilog $GEM/aigpdk/gemcarry.v
read_verilog add_rtl.v
hierarchy -check -top add_rtl8
synth -flatten -run begin:fine
techmap -map +/xilinx/arith_map.v -D LUT_SIZE=6
stat"
```

Expected — the 8-bit add became two carry slices, with the rest of the design
still at Yosys's internal cell level (`synth` has not run its `fine:` half yet):

```
=== add_rtl8 ===

   ...
        8 cells
        1   $dff
        1   $mux
        1   $not
        2   $pos
        1   $xor
        2   CARRY4
```

`2 CARRY4` for 8 bits is the number to check. If you see `$lut` cells or
`$alu` surviving, Path B is broken for your Yosys and Step 5 will not work.

---

## Step 3. Path A — a hand-instantiated `CARRY4` through the aigpdk flow

This is `usage.md` Step 1.6 Path A + Step 2: instantiate `CARRY4` directly, then
run the ordinary aigpdk logic-mapping script over it. The question is whether
the carry slices survive synthesis intact while the surrounding 1-bit logic maps
to aigpdk cells.

`carry_top.v` is written so the answer is checkable by inspection — it is the
RTL that produces exactly the checked-in fixture `tests/data/yosys_carry4.gv`:

```verilog
  wire [7:0] s = a ^ b;           // propagate
  wire [3:0] co_lo, co_hi;

  CARRY4 c_lo (
    .CO(co_lo), .O(sum_out[3:0]),
    .CI(cin), .CYINIT(1'b0),
    .DI(a[3:0]), .S(s[3:0])
  );

  CARRY4 c_hi (
    .CO(co_hi), .O(sum_out[7:4]),
    .CI(co_lo[3]), .CYINIT(1'b0),
    .DI(a[7:4]), .S(s[7:4])
  );
```

Write the script and run it:

```bash
cd ~/gemB
cat > b1.ys <<EOF
read_verilog $GEM/aigpdk/gemcarry.v
read_verilog carry_top.v
hierarchy -check -top yadder

synth -flatten
delete t:\$print

dfflibmap -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge
abc -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge

stat
write_verilog -noattr out/yadder.gv
EOF

$YOSYS -q -l log/b1.log b1.ys
awk '/Printing statistics/{n++} n==2' log/b1.log | head -20
```

Expected — the **final** `stat` (there are two in the log; the first is from
inside `synth`, before liberty mapping, and still shows `$_XOR_` / `$_DFF_P_`
cells):

```
=== yadder ===

   ...
       42 cells
       16   AND2_01_0
        8   AND2_11_1
        2   CARRY4
       16   DFF
```

That cell mix is the pass criterion, and it is worth checking digit by digit:

- **`2 CARRY4`** — both slices came through `synth -flatten`, `abc -liberty` and
  `opt_clean -purge` untouched. `CARRY4` is a `(* blackbox *)` in `gemcarry.v`,
  so Yosys never looks inside it and never shreds the carry chain into gates.
  (Note the protection comes from the blackbox attribute, *not* from the
  liberty: `aigpdk_nomem.lib` has no `CARRY4` cell at all — the `dont_touch`
  entry lives in the full `aigpdk.lib`, which this flow does not read.)
- **`16 DFF`** — the two 8-bit input registers, mapped by `dfflibmap`.
- **`16 AND2_01_0` + `8 AND2_11_1`** = 24 gates — exactly the 8 XORs of
  `s = a ^ b`, three aigpdk cells each. Nothing else: the adder's carry logic is
  *not* in the AIG.

Now confirm the netlist is byte-identical to the checked-in regression fixture:

```bash
diff out/yadder.gv $GEM/tests/data/yosys_carry4.gv && echo IDENTICAL
```

```
IDENTICAL
```

That matters more than it looks: `macro_test`'s `yadder` case (Step 1) drives
this exact file through 400 cycles of differential simulation, so reproducing it
bit-for-bit means the frontend output you just generated is the one already
proven correct. If the diff is non-empty after a Yosys upgrade, regenerate the
fixture and re-pin `YADDER_HASH` in `src/bin/macro_test.rs`.

Finally, look at how the slices are wired in the output:

```bash
grep -n 'CARRY4' -A 8 out/yadder.gv
```

```verilog
  CARRY4 c_hi (
    .CI(co_lo[3]),
    .CO(co_hi),
    .CYINIT(1'h0),
    .DI(a[7:4]),
    .O(sum_out[7:4]),
    .S(s[7:4])
  );
  CARRY4 c_lo (
    .CI(cin),
    .CO(co_lo),
    .CYINIT(1'h0),
    .DI(a[3:0]),
    .O(sum_out[3:0]),
    .S(s[3:0])
  );
```

`c_hi.CI` driven by `c_lo.CO[3]`, un-inverted and non-constant, is precisely the
pattern `fuse_carry4_chains` looks for. Step 8 confirms it fires.

There is also **no parameter list** on either instance. That matters: GEM's
netlist reader does not support instance parameters at all, and a
`CARRY4 #(...)` would fail Step 9 with an `NL_SV_PARSE` error.

---

## Step 4. Path A — is the netlist actually equivalent to the RTL?

Step 3 only counted cells. This step proves the mapped netlist computes the same
thing as the source RTL, by simulating both against each other. `tb_b1.v`
instantiates the RTL `yadder` and the synthesized netlist side by side, drives
512 random cycles, and compares `sum_out` and `cout` every cycle.

The gate netlist has the same module name as the RTL, so rename the copy used
for the comparison:

```bash
cd ~/gemB
sed 's/^module yadder(/module yadder_gate(/' out/yadder.gv > out/yadder_gate.v

iverilog -g2005 -o sim_b1 \
  $GEM/aigpdk/aigpdk.v \
  $GEM/aigpdk/gemcarry.v \
  carry_top.v out/yadder_gate.v tb_b1.v
./sim_b1
```

Expected:

```
PASS: 512 cycles, RTL and aigpdk gate netlist agree bit-exactly
tb_b1.v:36: $finish called at 5137000 (1ps)
```

One detail about the file list, and it differs from Part A: here you read
**`gemcarry.v` itself**, not a separate `_behav` copy. `gemcarry.v` carries the
`(* blackbox *)` attribute for Yosys *and* a simulatable body (guarded by
`` `ifndef GEM_CARRY4_NO_BEHAVIOUR``), and Icarus ignores the attribute. Part A's
`gemmacro.v` has no body, which is why Part A needs `gemmacro_behav.v`.

`aigpdk.v` supplies the `AND2_*` / `DFF` cell models the netlist instantiates.

---

## Step 5. Path B — infer `CARRY4` from plain `+` RTL

This is the step Part A cannot do. `add_rtl.v` contains no vendor primitive
whatsoever:

```verilog
  always @(posedge clk)
    sum <= a + b + cin;
```

The working recipe injects the Xilinx arithmetic map into the **generic** `synth`
flow, between its coarse and fine stages, rather than running `synth_xilinx` and
trying to unmap its `LUT*`/`FDRE` afterwards:

```bash
cd ~/gemB
cat > b2.ys <<EOF
read_verilog $GEM/aigpdk/gemcarry.v
read_verilog add_rtl.v
hierarchy -check -top add_rtl

synth -flatten -run begin:fine
techmap -map +/xilinx/arith_map.v -D LUT_SIZE=6     ;# \$alu -> CARRY4
techmap -map $GEM/aigpdk/gemcarry_map.v             ;# CARRY8 -> 2x CARRY4
synth -run fine:
delete t:\$print

dfflibmap -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge
abc -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge

stat
write_verilog -noattr out/add_rtl.gv
EOF

$YOSYS -q -l log/b2.log b2.ys
awk '/Printing statistics/{n++} n==NS' NS=$(grep -c 'Printing statistics' log/b2.log) log/b2.log | head -20
```

Expected — a 16-bit add becomes four carry slices plus the same
three-cells-per-XOR glue:

```
=== add_rtl ===

   ...
       68 cells
       32   AND2_01_0
       16   AND2_11_1
        4   CARRY4
       16   DFF
```

`4 CARRY4` for 16 bits (`ceil(16/4)`), 16 `DFF` for the output register, 48
AND2 for the 16 propagate XORs. Check the chain is actually cascaded:

```bash
grep -n 'CARRY4' -A 8 out/add_rtl.gv | head -22
```

```verilog
  CARRY4 _083_ (
    .CI(1'h0),
    .CO(_032_[3:0]),
    .CYINIT(cin),
    ...
  );
  CARRY4 _084_ (
    .CI(_032_[3]),
    .CO(_032_[7:4]),
    .CYINIT(1'h0),
    ...
```

`_084_.CI = _032_[3] = _083_.CO[3]` — the `CO[3] -> CI` link GEM fuses on.

### The `-run` label pitfall — run the control

This is the single easiest way to get Path B wrong, and **it is not an error**.
Get the label pair wrong (or omit the `arith_map` techmap entirely) and `synth`
silently runs its whole generic flow, shredding the carry chain into AIG gates:

```bash
cat > b2bad.ys <<EOF
read_verilog $GEM/aigpdk/gemcarry.v
read_verilog add_rtl.v
hierarchy -check -top add_rtl
synth -flatten
delete t:\$print
dfflibmap -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge
abc -liberty $GEM/aigpdk/aigpdk_nomem.lib
opt_clean -purge
stat
write_verilog -noattr out/add_rtl_shredded.gv
EOF
$YOSYS -q -l log/b2bad.log b2bad.ys
awk '/Printing statistics/{n++} n==NS' NS=$(grep -c 'Printing statistics' log/b2bad.log) log/b2bad.log | head -22
grep -c CARRY4 out/add_rtl_shredded.gv
```

Expected — **151 AIG gates and zero carry macros**, from the identical source:

```
=== add_rtl ===

   ...
      167 cells
       43   AND2_00_0
       15   AND2_01_0
        5   AND2_10_0
        4   AND2_11_0
       46   AND2_11_1
       16   DFF
       38   INV
```
```
0
```

68 cells with 4 `CARRY4` versus 167 cells with none, and Yosys reports success
either way. **Always `stat` after the `arith_map` techmap** — the cell count is
the only signal you get. Step 8 turns the same contrast into an AIG depth
number, which is the metric that actually matters for GEM.

Valid labels are `begin:fine` then `fine:`; `-run begin:alumacc` is *not* a
valid pair for this purpose and will land you in the shredded case.

---

## Step 6. Why not `synth_xilinx`?

The obvious flow — let Yosys's Xilinx target infer the carry chain — does infer
it, and is still the wrong tool here. Run it to see why:

```bash
cd ~/gemB
$YOSYS -q -l log/b2x.log -p "
read_verilog add_rtl.v
hierarchy -check -top add_rtl
synth_xilinx -flatten
stat"
awk '/Printing statistics/{n++} n==NS' NS=$(grep -c 'Printing statistics' log/b2x.log) log/b2x.log | head -22
```

Expected:

```
=== add_rtl ===

   ...
       87 cells
        1   BUFG
        4   CARRY4
       16   FDRE
       34   IBUF
       16   LUT2
       16   OBUF
```

The same **4 `CARRY4`** — the inference itself is fine. But everything around
them is Xilinx-technology cells (`LUT2`, `FDRE`, `IBUF`/`OBUF`/`BUFG`) that the
aigpdk flow does not map, and post-processing them back into `AND2_*`/`DFF` is
strictly harder than never leaving the generic flow. That is the whole reason
Step 5's recipe reaches for `+/xilinx/arith_map.v` inside `synth` instead.

(This step is informational — it is not a pass/fail gate beyond "still 4
`CARRY4` + `FDRE`". If a future Yosys changes it, Step 5 is what you re-verify.)

---

## Step 7. `CARRY8` → two chained `CARRY4`

`aigpdk/gemcarry_map.v` carries one rule: split an UltraScale `CARRY8` into two
`CARRY4` joined `CO[3] -> CI`. Yosys 0.68 **never emits a `CARRY8`** — even
`synth_xilinx -family xcup` produces `CARRY4` — so the only way to reach this
rule is to write the cell by hand, which is what `carry8_top.v` is for.

`hierarchy -check` runs before the techmap, so `CARRY8` has to be a known module
first; read the map file itself with `-lib` to declare it:

```bash
cd ~/gemB
cat > b8.ys <<EOF
read_verilog $GEM/aigpdk/gemcarry.v
read_verilog -lib $GEM/aigpdk/gemcarry_map.v
read_verilog carry8_top.v
hierarchy -check -top carry8_top
proc;;
techmap -map $GEM/aigpdk/gemcarry_map.v
opt_expr -fine; opt_clean
stat
write_verilog -noattr out/carry8_mapped.v
EOF

$YOSYS -q -l log/b8.log b8.ys
awk '/Printing statistics/{n++} n==NS' NS=$(grep -c 'Printing statistics' log/b8.log) log/b8.log | head -18
```

Expected — one `CARRY8` in, two `CARRY4` out, plus the `$xor` that is still the
design's own propagate term:

```
=== carry8_top ===

   ...
        3 cells
        1   $xor
        2   CARRY4
```

```bash
grep -n 'CARRY4' -A 8 out/carry8_mapped.v
```

```verilog
  CARRY4 \u.hi  (
    .CI(co[3]),
    .CO(co[7:4]),
    ...
  CARRY4 \u.lo  (
    .CI(cin),
    .CO(co[3:0]),
```

Cell names are not arithmetic, so check the arithmetic too. `tb_carry8.v`
compares the techmapped design against a plain `a + b + cin` over 1000 random
vectors:

```bash
iverilog -g2005 -o sim_c8 $GEM/aigpdk/gemcarry.v out/carry8_mapped.v tb_carry8.v
./sim_c8
```

```
PASS: 1000 random vectors, CARRY8 -> 2x CARRY4 == a + b + cin
tb_carry8.v:31: $finish called at 1000000 (1ps)
```

This closes an item Part B's implementation log listed as open ("elaborates but
not exercised against a real design"). It is exercised against a *hand-written*
`CARRY8` — no stock Yosys 0.68 flow produces one, so that caveat stands.

---

## Step 8. GEM reads the Yosys netlists and fuses the chain

Two questions: does GEM's frontend parse what Yosys produced, and does
`fuse_carry4_chains` actually collapse the cascade into one macro rather than
leaving a slice per level?

The cheap, decisive probe is AIG depth. `level_test` reports the number of AIG
levels in the design — a native carry chain contributes *one macro level*, a
shredded one contributes roughly one level per carry bit:

```bash
cd ~/gemB
for f in out/yadder.gv out/add_rtl.gv out/add_rtl_shredded.gv; do
  printf '%-28s ' "$f"
  $CARGO_TARGET_DIR/debug/level_test $f 2>&1 | grep -oP 'Number of levels: \K[0-9]+'
done
```

Expected:

```
out/yadder.gv                2
out/add_rtl.gv               2
out/add_rtl_shredded.gv      34
```

**2 versus 34** on functionally identical 16-bit adders is the entire point of
Part B. (The full histogram for `out/add_rtl.gv` is `[0]: 83, [1]: 32, [2]: 16`
— the 16 propagate XORs and the output register, and nothing else.)

Parsing alone is not fusion, though: two un-fused slices would also give a small
level count. The fusion evidence is the macro/stage count from `macro_test`,
which drives the byte-identical fixture from Step 3:

```bash
cd $GEM && cargo run -q --bin macro_test 2>&1 | grep '^\[yadder\]'
```

```
[yadder] 75 aigpins, 9 endpoint groups, 1 macro(s), 2 major stage(s), reg/io state 259 words, blocks_data hash 2719296911916927336
[yadder] differential check passed over 400 cycles
```

**`1 macro(s)`** for a design containing **two** `CARRY4` cells is the fusion
firing: `fuse_carry4_chains` replaced both slices with a single
`Carry4 { chain_len: 2 }` endpoint. Un-fused, they would sit at two different
macro levels and you would see `2 macro(s), 3 major stage(s)` — which is exactly
what `carry_then_logic_then_carry` reports in Step 1, because its second CARRY4
has `CI` tied low and so cannot fuse.

`2 major stage(s)` is expected and not a defect: Class C macro outputs always
route through the staged-IO scratch path, so each distinct macro level forces
one major-stage split (one grid sync per simulated cycle). Collapsing that to
one stage needs the intra-boomerang evaluation slot, which is deferred work.

---

## Step 9. End to end — GEM's simulator vs Icarus on the same netlist

The last functional question: does GEM's own simulator consume the netlist Yosys
produced in Step 3, recognise the carry macro, and simulate it correctly? This
drives one set of vectors through GEM's `naive_sim` and through Icarus on the
same netlist, and diffs them.

```bash
cd ~/gemB
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
  $GEM/aigpdk/aigpdk.v $GEM/aigpdk/gemcarry.v \
  out/yadder_gate.v tb_e2e.v
./sim_e2e
```

```
iverilog: wrote 256 cycles to iv_out.txt
tb_e2e.v:41: $finish called at 2560000 (1ps)
```

Then GEM:

```bash
$CARGO_TARGET_DIR/debug/naive_sim out/yadder.gv stim.vcd naive.vcd
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
PASS: 255 cycles compared (1 skipped as x), GEM naive_sim == Icarus on sum_out + cout
```

**Three conventions to understand before you debug a spurious failure here:**

- Cycle 0 is skipped because Verilog starts registers at `x` while GEM starts
  them at `0`. `cmpvcd.py` drops any reference line containing `x`, so exactly
  one cycle is skipped.
- `tb_e2e.v` samples the outputs **just before** each rising edge, not after.
  GEM's `naive_sim` timestamps the *pre-edge* state at a clock edge — the same
  `Q` convention its DFF outputs follow. Sampling after the edge instead makes
  every cycle mismatch by a one-cycle shift; if you see a wholesale failure
  here, check the offset before suspecting the carry datapath.
- `naive_sim` writes multi-bit ports **bit-blasted** (`$var wire 1 ! sum_out[7]`,
  not a single 8-bit vector), so `cmpvcd.py` reassembles `sum_out` MSB-first
  from its eight scalar identifiers. A comparator that assumes a vector `$var`
  reports `????????` for every cycle — that is a reader bug, not a simulation
  mismatch.

---

## Step 10. GPU differential (optional — needs CUDA + a GPU)

If the box has a CUDA build (`cargo build --features cuda`), the same netlist
can be pushed through the real kernel and checked against the CPU emulator. This
exercises the Class C kernel block: `gem_eval_carry4`, the shared-memory lane
gather, metadata `[12..16]` + `[16+i]`, the unconditional scratch commit, and
the extra `cooperative_groups::this_grid()` sync per Class C major stage.

```bash
cd ~/gemB
. ~/cudaenv.sh
$CARGO_TARGET_DIR/debug/cut_map_interactive out/yadder.gv out/yadder.gemparts
$CARGO_TARGET_DIR/debug/cuda_test out/yadder.gv out/yadder.gemparts \
    stim.vcd out/gpu.vcd 1 --check-with-cpu
```

Expected (on the RTX 2050 this was run on):

```
INFO  # of effective partitions in each stage: [1, 1]
DEBUG major stage 0: max total boomerang depth (w/ cost) 4
DEBUG major stage 1: max total boomerang depth (w/ cost) 4
INFO  Built script for 1 blocks, reg/io state size 259, sram size 0, script size 34304
Script hash: 2719296911916927336
INFO  total number of cycles: 257
INFO  running sanity test
INFO  sanity test passed!
```

Two things to read:

- `sanity test passed!` — the GPU and `emulate::simulate_block_v1` agree
  bit-exactly on every output, every cycle.
- `Script hash: 2719296911916927336` — **identical** to the `[yadder]` hash from
  Step 1 and 8. The `cut_map_interactive` + `cuda_test` entry point derives the
  same auto-split set and emits the same script bytes as the standalone
  `macro_test` harness. If these two hashes ever diverge, the split derivation
  has become entry-point-dependent, which would be a real bug.

`num_parts == 1` is special-cased in `cut_map_interactive`, so this does not
call `mt-kahypar` and runs in about a second.

---

## Step 11. Run everything at once

All of the above is scripted. Note the `export` — an inline
`GEM=... bash $GEM/...` does **not** work, because `$GEM` is expanded before the
assignment takes effect:

```bash
export GEM=/mnt/c/Users/Devansh/GEM
export YOSYS=$HOME/yosys/build/yosys
bash "$GEM/tests/yosys/partb/run_all.sh"
```

It builds a fresh work directory (`~/gemB-run` by default), runs Steps 1–10, and
reports a non-zero exit status equal to the number of failed checks. Step 10 is
skipped automatically when there is no CUDA build. Full expected output:

```
=== 0. environment ===
Yosys 0.68 (git sha1 38e001a6f, GNU /usr/bin/c++ 13.3.0)
Icarus Verilog version 14.0 (devel) (s20260301-391-g64f13540a-dirty)

=== 1. CPU substrate (cargo) ===
  [ok]   cargo test
  [ok]   macro_test differential

=== 2. premise - Yosys 0.68 arith_map.v still emits CARRY4 ===
  [ok]   $alu -> 2 CARRY4 on an 8-bit add

=== 3. Path A - explicit CARRY4 -> aigpdk netlist ===
  [ok]   2 CARRY4 survive synth + abc
  [ok]   16 DFF + AND2 glue mapped
  [ok]   byte-identical to tests/data/yosys_carry4.gv

=== 4. Path A - RTL vs gate netlist (iverilog) ===
  [ok]   512 cycles bit-exact

=== 5. Path B - infer CARRY4 from plain + RTL ===
  [ok]   16-bit add -> 4 CARRY4
  [ok]   16 DFF + AND2 glue mapped
  [ok]   control: plain synth shreds the chain (0 CARRY4) - the pitfall is real

=== 6. synth_xilinx contrast - why the generic flow is used ===
  [ok]   synth_xilinx also infers 4 CARRY4 but leaves LUT2/FDRE (not aigpdk cells)

=== 7. CARRY8 -> 2x CARRY4 techmap ===
  [ok]   one CARRY8 -> two chained CARRY4
  [ok]   1000 vectors == a + b + cin

=== 8. GEM reads the Yosys netlists and fuses the chain ===
  [ok]   Path A netlist: 2 AIG levels
  [ok]   Path B netlist: 2 AIG levels
  [ok]   control (shredded): 34 AIG levels - the chain really is native
  [ok]   the two slices fuse into ONE Carry4 macro (1 macro, 2 stages)

=== 9. end-to-end - GEM naive_sim vs iverilog on the same netlist ===
  PASS: 255 cycles compared (1 skipped as x), GEM naive_sim == Icarus on sum_out + cout
  [ok]   GEM == Icarus over 255 cycles

=== 10. GPU differential (optional) ===
  [ok]   cuda_test --check-with-cpu: GPU == CPU
  [ok]   script hash matches macro_test's YADDER_HASH

ALL PART B YOSYS CHECKS PASSED  (workdir: /home/devansh/gemB-run)
```

---

## Troubleshooting

| Symptom | Cause |
|---|---|
| Step 3 `stat` shows no `CARRY4` and a pile of `AND2_*` | `gemcarry.v` was not read, or was read without its `(* blackbox *)` attribute. The liberty does **not** protect `CARRY4` in this flow — `aigpdk_nomem.lib` has no such cell. |
| Step 5 produces 0 `CARRY4` but Yosys reports success | The `-run` label pitfall. Valid pair is `begin:fine` then `fine:`; a wrong pair silently runs the full generic flow. Always `stat` after the `arith_map` techmap. |
| Step 5 produces `LUT2`/`FDRE` alongside the `CARRY4` | You ran `synth_xilinx` instead of the generic `synth` + `arith_map` recipe. See Step 6. |
| `ERROR: Module '\CARRY8' ... is not part of the design` | `hierarchy -check` ran before `CARRY8` was declared. Add `read_verilog -lib $GEM/aigpdk/gemcarry_map.v` before `hierarchy` (Step 7). |
| Step 3 diff against `tests/data/yosys_carry4.gv` is non-empty | Either the Yosys version moved, or `carry_top.v` changed. Regenerate the fixture and re-pin `YADDER_HASH` in `src/bin/macro_test.rs` — that pin tracks the frontend, not the script ABI. |
| `NL_SV_PARSE` error from `naive_sim` / `level_test` | The netlist has instance parameters on a cell. GEM's reader does not support them; strip with `setparam -unset` or `write_verilog -noattr`. |
| Step 9 reports `gem=????????` every cycle | Comparator bug, not a simulation bug: `naive_sim` bit-blasts multi-bit ports. See the third note at the end of Step 9. |
| Step 9 fails on nearly every cycle with plausible-looking values | Sampling-convention offset — see the second note at the end of Step 9. |
| `[yadder]` reports `2 macro(s), 3 major stage(s)` | Fusion did not fire. Check that `c_hi.CI` is driven directly by `c_lo.CO[3]`, un-inverted and non-constant — that is the only pattern `fuse_carry4_chains` matches. |
| Step 8's shredded control shows few levels | You regenerated `add_rtl_shredded.gv` with the arith_map in it. The control must be the *plain* `synth -flatten` flow. |
| `run_all.sh` reports "no yosys at ..." | You used the inline `GEM=... bash $GEM/...` form; `$GEM` expands before the assignment lands. `export` the variables first (Step 11). |
| Answers differ from this document | Check `$YOSYS -V`. A distro Yosys 0.33 needs 5 `CARRY4` for the Step 5 design where 0.68 needs 4. |

---

## What this runbook does *not* cover

- **A `CARRY8` from a real synthesis flow.** Step 7 exercises `gemcarry_map.v`
  against a hand-instantiated `CARRY8` only. Yosys 0.68 never emits one — even
  `synth_xilinx -family xcup` produces `CARRY4` — so the UltraScale carry
  primitive has no path through Yosys's arith map in this release. Blocked
  externally, not by GEM.
- **`compute-sanitizer`** over the Class C shared-memory gather and the extra
  grid syncs. The WSL CUDA install here is a partial `dpkg-deb -x` extraction
  (`nvcc`/`nvvm`/`cudart` only) and does not include the tool. Step 10's numeric
  differential passes; the memcheck/racecheck pass is still open.
- **`chain_len > 16`.** Fused segments are capped at 16 slices (64 carry bits);
  a longer cascade splits into `<=16`-slice segments joined by one extra macro
  level. No fixture here is long enough to reach that.
- **The intra-boomerang (zero-grid-sync) Class C evaluation slot.** Every
  distinct carry-chain depth still costs one forced major-stage split, which is
  why Step 8 sees 2 stages for an 8-bit adder. Deferred perf work.
- **Parts A and C.** `GEM_DSP48E2` and `SRLC32E` have their own flows in
  `usage.md` Steps 1.5 and 1.7, and Part A has its own runbook in
  [.claude/yosysA.md](yosysA.md). `macro_test` in Step 1 covers all three on the
  CPU side.
