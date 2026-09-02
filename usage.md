# Getting Started to use GEM

**Caveats**: currently GEM only supports non-interactive testbenches. This means the input to the circuit needs to be a static waveform (e.g., VCD). Registers and clock gates inside the circuit are allowed, but latches and other asynchronous sequential logics are currently unsupported.

**Dataset**: Some (namely, netlists after AIG transformation in Steps 1-2 below, and reference VCDs) input data is available [here](https://drive.google.com/drive/folders/1M42vFoVZhG4ZjyD1hqYD0Hrw8F1rwNXd?usp=drive_link) .

## Step 0. Download the AIG Process Kit
Go to [aigpdk](./aigpdk) directory where you can download `aigpdk.lib`, `aigpdk_nomem.lib`, `aigpdk.v`, and `memlib_yosys.txt`. You will need them later in the flow.

Before continuing, make sure your design contains only synchronous logic.
If your design has clock gates implemented in your RTL code, you need to replace them manually with instantiations to the `CKLNQD` module in `aigpdk.v`.
Also, you are advised to be familiar with where memory blocks (e.g., caches) are implemented in your design so you can check that the memory blocks are mapped correctly later.

## Step 1. Memory Synthesis with Yosys
This step makes use of the open-source [Yosys](https://github.com/YosysHQ/yosys) synthesizer to recognize and map the memory blocks automatically.

Download and compile the latest version of Yosys. Then run yosys shell with the following synthesis script.

``` tcl
# replace this with paths to your RTL code, and add `-I`, `-D`, `-sv` etc when necessary
read_verilog xx.v yy.v top.v

# replace TOP_MODULE with your top module name
hierarchy -check -top TOP_MODULE

# simplify design before mapping
proc;;
opt_expr; opt_dff; opt_clean
memory -nomap

# map the rams
# point -lib path to your downloaded memlib_yosys.txt
memory_libmap -lib path/to/memlib_yosys.txt -logic-cost-rom 100 -logic-cost-ram 100
```

The `memory_libmap` command will output a list of RAMs it found and mapped.

- If you see `$__RAMGEM_SYNC_`, it means the mapping is successful.
- If you see `$__RAMGEM_ASYNC_`, it means this RAM is found to have asynchronous READ port. You need to confirm if it is the case.
  - If it is a synchronous one but accidentally recognized as asynchronous, you might need to patch the RTL code to fix it. There might be multiple reasons it cannot be recognized as synchronous. For example, [when the read and write clocks are different](https://github.com/YosysHQ/yosys/issues/4521).
  - If it is indeed asynchronous, check its size. If its size is very small and affordable to be synthesized using registers and mux trees (which is *very* expensive for large RAM banks), you can remove the `$__RAMGEM_ASYNC_` block in `memlib_yosys.txt`, re-run Yosys to force the use of registers.
- If you see `using FF mapping for memory`, it means the memory is recognized, but due to it being nonstandard (e.g., special global reset or nontrivial initialization), GEM will fall back to registers and mux trees. If the size of the memory is small, this is usually not an issue. Otherwise, you are advised to try other implementations.

After a successful mapping, use the following command to write out the mapped RTL as a single Verilog file.
``` tcl
write_verilog memory_mapped.v
```

Check the correctness of this step by simulating `memory_mapped.v` with your reference CPU simulator.

## Step 1.5 (optional). Word-level MAC (DSP48E2) inference

GEM can evaluate a `27x18` multiply-accumulate natively as a single macro
endpoint (a `GEM_DSP48E2` cell) instead of shredding it into a multiplier's
worth of AIG gates. This is opt-in: skip this step and `a*b+c` still maps to
ordinary logic.

The macro GEM understands is a fixed slice of a Xilinx `DSP48E2`:

- `PREG = 1` — the accumulator `P` is the *only* registered stage.
- `AREG`/`BREG`/`CREG`/`DREG`/`ADREG`/`MREG` all `0` — every input path is
  combinational.
- `OPMODE` is pre-decoded to a 2-bit `OPMODE_S`: `0 = P<=C` (bypass),
  `1 = P<=A*B`, `2 = P<=P+A*B`.
- optional 27-bit signed pre-adder `A+D` (selected by `USE_D`), 45-bit signed
  product, 48-bit accumulator; `RSTP` is a synchronous reset of `P`.

`aigpdk/gemmacro_map.v` is a Yosys `techmap` file that rewrites a `DSP48E2`
cell in that configuration into `GEM_DSP48E2`, and raises a `$error` (naming
what to change) for anything outside it. `aigpdk/gemmacro.v` carries the
matching behavioural model, used both as the techmap target and for reference
simulation.

### Path A — instantiate the DSP directly (validated)

The dependable route today is explicit instantiation.

Simplest: instantiate `GEM_DSP48E2` straight from your RTL (ports:
`CLK, CEP, RSTP, USE_D, A[26:0], D[26:0], B[17:0], C[47:0], OPMODE_S[1:0],
P[47:0]`) and just link the model in — no techmap:

``` tcl
read_verilog path/to/aigpdk/gemmacro.v
```

Or, if your RTL instantiates a real `DSP48E2` primitive with `PREG=1` and the
other registers `0`, rewrite it:

``` tcl
# same Yosys session, after memory_libmap, before logic mapping.
read_verilog path/to/aigpdk/gemmacro.v
techmap -map path/to/aigpdk/gemmacro_map.v   ;# DSP48E2 -> GEM_DSP48E2
stat                                          ;# confirm GEM_DSP48E2 appears
```

A static `OPMODE` folds to a constant `OPMODE_S`; a data-dependent `OPMODE`
(e.g. a `load` select) synthesises into a handful of gates feeding the macro's
`OPMODE_S` input.

### Path B — infer from `a*b + acc` RTL (version-dependent)

In principle Yosys's own Xilinx DSP inference produces a `PREG=1` `DSP48E2`
from plain accumulator RTL, which Path A's `techmap` then picks up. The
sequence mirrors `synth_xilinx`'s `map_dsp` label — note `mul2dsp` must run on
the raw `$mul` (do **not** `alumacc` first, or the multiply is folded into a
`$macc` it cannot see):

``` tcl
read_verilog path/to/aigpdk/gemmacro.v

memory_dff
techmap -map +/mul2dsp.v -map +/xilinx/xcu_dsp_map.v \
  -D DSP_A_MAXWIDTH=27 -D DSP_B_MAXWIDTH=18 \
  -D DSP_A_MINWIDTH=2  -D DSP_B_MINWIDTH=2 \
  -D DSP_NAME=$__MUL27X18
select a:mul2dsp ; setattr -unset mul2dsp ; select -clear
opt_expr -fine ; wreduce
xilinx_dsp -family xcup      ;# meant to fold pre-adder / post-adder / PREG in
chtype -set $mul t:$__soft_mul

techmap -map path/to/aigpdk/gemmacro_map.v   ;# DSP48E2 -> GEM_DSP48E2
```

> **Checked against Yosys 0.68 (git sha1 38e001a6f):** the UltraScale+ DSP map
> file is `+/xilinx/xcu_dsp_map.v` — there is no `xcup_dsp_map.v`. More
> importantly, this release's `xilinx_dsp` does **not** fold a `P <= P + A*B`
> accumulator into `PREG` + the post-adder: it emits `DSP48E2` with `PREG=0` and
> leaves the adder and accumulator flop in fabric, so `gemmacro_map.v` rejects it
> with the `PREG=0` `$error`. This was re-checked over `p <= p + a*b`,
> `p <= c + a*b` and even a plain `p <= a*b`, through both the manual sequence
> above and `synth_xilinx -family {xcup,xcu,xc7}` — none of them fold, and the
> `xilinx_dsp` pass reports no matches. Until a Yosys whose `xilinx_dsp` folds
> the output register is pinned, use **Path A**. Re-validate the pass / map-file
> names and the `DSP48E2` port set (`help DSP48E2`,
> `` `yosys-config --datdir`/xilinx/ ``) whenever you change the Yosys version.
> (The `DSP48E2` port and parameter list is identical in 0.33 and 0.68, so
> `gemmacro_map.v` did not need to change across that bump.)

Requirements for a MAC to map (enforced by `gemmacro_map.v`):

- **`PREG` must be 1** — a registered accumulator. `PREG=0` `$error`s out.
- **`AREG`/`BREG`/`CREG`/`DREG`/`ADREG`/`MREG` must be 0** — all macro inputs
  are combinational. Any input pipelining `$error`s out; move it outside the
  MAC or register it as ordinary flops.
- One `27x18` tile per macro. Wider multiplies are split by `mul2dsp.v` into
  cascaded partial products; the `DSP48E2 -> GEM_DSP48E2` rewrite of a cascade
  is untested.
- `OPMODE` patterns other than the three above (e.g. `Z=C` add-to-C) map to
  `OPMODE_S=0` (bypass) rather than erroring — check your decode if you rely
  on an exotic `OPMODE`.

Confirm `GEM_DSP48E2` instances appear (`stat`), then continue to Step 2.
Check correctness by simulating the result with `gemmacro.v` linked in.

## Step 1.6 (optional). Carry-chain (CARRY4) inference

GEM also evaluates the Xilinx 7-series `CARRY4` carry chain natively, as a
**Class C (combinational)** word-level macro: an `N`-bit adder becomes
`ceil(N/4)` `CARRY4` slices, and GEM's AIG frontend fuses a maximal
`CO[3] -> CI` cascade into a single `MacroKind::Carry4 { chain_len }` endpoint
whose native datapath ripples all `4*chain_len` carries in a fixed
parallel-prefix step (no warp divergence). Each distinct carry-chain *depth*
in the design costs one forced major-stage split (one extra grid sync per
simulated cycle) — see the deferred-work note below.

`aigpdk/gemcarry.v` is the `CARRY4` blackbox model (port-identical to the
vendor primitive, with a behavioural body for reference simulation).
`aigpdk/gemcarry_map.v` splits an UltraScale `CARRY8` into two chained
`CARRY4`.

### Path A — instantiate `CARRY4` directly (recommended)

Instantiate `CARRY4` from your RTL (ports `CO[3:0] O[3:0] CI CYINIT DI[3:0]
S[3:0]`; `C[0] = CI | CYINIT`, exactly one driven), then:

```tcl
read_verilog path/to/aigpdk/gemcarry.v
# ... your design ...
stat                                       ;# confirm CARRY4 appears
```

### Path B — infer from `+` RTL (validated on Yosys 0.68)

Inject the Xilinx arithmetic map into the *generic* `synth` flow, between the
coarse and fine stages — this keeps the rest of Step 2's aigpdk mapping intact:

```tcl
read_verilog path/to/aigpdk/gemcarry.v
# ... your design ...
synth -flatten -run begin:fine
techmap -map +/xilinx/arith_map.v -D LUT_SIZE=6  ;# $alu/$add -> CARRY4 chains
techmap -map path/to/aigpdk/gemcarry_map.v       ;# CARRY8 -> 2x CARRY4 (if any)
synth -run fine:
delete t:$print
dfflibmap -liberty path/to/aigpdk_nomem.lib ; opt_clean -purge
abc       -liberty path/to/aigpdk_nomem.lib ; opt_clean -purge
write_verilog -noattr gatelevel.gv
```

> **Checked against Yosys 0.68 (git sha1 38e001a6f).** `arith_map.v` still
> emits real `CARRY4` cells (not a `$lut` carry) for any `LUT_SIZE != 4`
> target, and the script above takes a 16-bit `sum <= a + b + cin` all the way
> to **4 `CARRY4` + 16 `DFF` + aigpdk AND2 glue**, which GEM parses and fuses.
> Get the `-run` labels right: `begin:fine` then `fine:`. A wrong label pair is
> not an error — `synth` just runs its whole generic flow and the carry chain is
> silently shredded into ~140 AIG gates with no `CARRY4` at all, so always
> `stat` after the `arith_map` techmap.
>
> Note that `synth_xilinx` on its own also infers `CARRY4` (4 of them for the
> same design), but it leaves `LUT2`/`FDRE` behind, which the aigpdk flow does
> not map — that is why the recipe above uses generic `synth`. Pass / map-file
> names drift between Yosys releases, so re-confirm against your pinned Yosys
> when you change versions.

**Deferred / known limits:**
- Every distinct carry-chain depth costs one forced major-stage split. The
  zero-grid-sync alternative (an intra-boomerang macro evaluation slot) is
  follow-up work.
- Fused segments are capped at `chain_len <= 16` (64 carry bits); a longer
  cascade splits into `<=16`-slice segments joined by one extra macro level.
- An un-fused `CARRY4` (a fusion-pattern miss) is correctness-safe but slow —
  each slice becomes its own macro level / split.

## Step 1.7 (optional). Shift-register-LUT (SRLC32E) inference

GEM also evaluates the Xilinx `SRLC32E` 32-bit shift-register LUT natively.
It is the one primitive that is **both** scheduling classes at once, and GEM
handles that by decomposing the cell into the two that already exist, joined
by 32 ordinary AIG source pins:

| half | class | inputs | outputs | native datapath |
|---|---|---|---|---|
| `Srl32Shift` | R (registered) | `D`, `en_iv = CLK & CE` | `state[0..32]` | `(state << 1) \| D` |
| `Srl32Read` | C (combinational) | `state[0..32]`, `A[4:0]` | `Q` | `(state >> A) & 1` |

Consequences worth knowing before you synthesise:

- **`Q31` is free.** The cascade port *is* `state[31]` — an ordinary AIG
  source pin, not a macro output. A `Q31 -> D` cascade (a 64/96-deep delay
  line, the common FPGA usage) creates **no** Class C macro, costs **zero**
  forced major-stage splits and **zero** extra grid syncs per cycle.
- **`Q` costs one split per distinct address-ready level**, inherited from the
  Class C model of Step 1.6. If a shift register is only ever used as a delay
  line, leave `Q` unconnected and read `Q31`.
- **`CE` is free too.** It is folded into the macro's write-out clock enable
  (`en_iv = CLK & CE`), exactly like a DFF's, so the datapath stays
  unconditional and branchless; `CE == 0` simply holds the committed state.
- The read is **asynchronous** — `Q` is visible in the same simulated cycle
  as the address that selected it, unlike the SRAM's registered read port.
  Both `Q` and `Q31` see the pre-edge state, matching the DFF `Q` convention.

`aigpdk/gemsrl.v` is the `SRLC32E` blackbox model (port-identical to the
vendor primitive, with a behavioural body for reference simulation).
`aigpdk/gemsrl_map.v` maps `SRL16E` and Yosys's `$__XILINX_SHREG_` onto it.

### Path A — instantiate `SRLC32E` directly (recommended)

Instantiate `SRLC32E` from your RTL (ports `Q Q31 A[4:0] CE CLK D`), then:

```tcl
read_verilog path/to/aigpdk/gemsrl.v
# ... your design ...
stat                                       ;# confirm SRLC32E appears
```

### Path B — infer from an RTL shift register (validated on Yosys 0.68)

```tcl
read_verilog path/to/aigpdk/gemsrl.v
# ... your design; synth_xilinx already runs the SRL inference internally ...
synth_xilinx -flatten -noiopad -noclkbuf
setparam -unset INIT -unset IS_CLK_INVERTED t:SRLC32E
setparam -unset INIT -unset IS_CLK_INVERTED t:SRL16E
techmap -map path/to/aigpdk/gemsrl_map.v   ;# SRL16E -> SRLC32E
stat
write_verilog -noattr design.gv
```

> **Checked against Yosys 0.68 (git sha1 38e001a6f).** `synth_xilinx` infers
> SRLs from plain RTL — `always @(posedge clk) sh <= {sh[N-2:0], d};` — with
> no map file needed for the 32-deep case:
>
> | RTL depth | Yosys 0.68 emits | GEM |
> |---|---|---|
> | 2–16 | `SRL16E` (scalar `A0..A3`) | `gemsrl_map.v` rewrites it to `SRLC32E` |
> | 18–32 | `SRLC32E` (bus `A[4:0]`) | consumed directly |
> | > 32 | a `Q31 -> D` cascade | consumed directly, still zero splits |
>
> **The clock enable is preserved.** `if (ce) sh <= {sh[N-2:0], d};` comes out
> as `SRLC32E` with `.CE(ce)`, and an addressed read (`assign q = sh[a];`)
> comes out as one `SRLC32E` with a dynamic 5-bit `A` bus *and* a live `CE` —
> exactly GEM's `Srl32Shift` + `Srl32Read` decomposition, with no
> hand-instantiation. (Yosys 0.33 dropped a non-constant `CE` here and tied it
> to `1'b1`; if you are on that release, use Path A.)
>
> The two extra lines in the script above are not optional:
> `-noiopad -noclkbuf` keeps `IBUF`/`OBUF`/`BUFG` out of the netlist, and the
> `setparam -unset` calls remove the `INIT` / `IS_CLK_INVERTED` parameters
> Yosys attaches to the inferred cell. **GEM's netlist reader does not support
> instance parameters at all** — a netlist containing `SRLC32E #(...) u (...)`
> fails with an `NL_SV_PARSE` error. GEM ignores `INIT` regardless (the shift
> register starts at zero), so stripping it loses nothing.

**Deferred / known limits:**
- Each SRL whose `Q` is used routes its 32 state bits through the boomerang
  (32 extra global-read bits and 32 write-out slots). The zero-copy
  alternative — a Class C state-feedback load — is follow-up work.
- A `Srl32Shift` reserves the uniform Class R footprint of 4 packed lanes and
  2 state words even though it needs 1 input bit and 1 state word; removing
  the waste would mean variable-width Class R lanes.
- `SRL16E` maps to `SRLC32E` with a zero-extended address (exact: stages
  0..15 read identically). An inverted clock or an active-low enable is
  rejected with a synthesis `$error` rather than mis-modelled.

## Step 2. Logic Synthesis
This step maps all combinational and sequential logic into a special set of standard cells we defined in `aigpdk.lib`.
The quality of synthesis is directly tied to GEM's final performance, so we suggest you use a commercial synthesis tool like DC. You can also use Yosys to complete this if you do not have access to a commercial synthesis tool.

Check the correctness of this step by simulating `gatelevel.gv` with your reference CPU simulator.

### Use Synopsys DC
First, you need to compile `aigpdk.lib` to `aigpdk.db` using Library Compiler.

With that, you synthesize the `memory_mapped.v` obtained before under `aigpdk.db`.

Some key commands you may use on top of your existing DC flow:

``` tcl
# change path/to/aigpdk.db to a correct path. same for other commands.
set_app_var link_path path/to/aigpdk.db
set_app_var target_library path/to/aigpdk.db
read_file -format db $target_library

# elaborate TOP_MODULE
# current_design TOP_MODULE

# timing settings like create_clock ... are recommended. GEM benefits from timing-driven synthesis.

compile_ultra -no_seq_output_inversion -no_autoungroup
optimize_netlist -area

write -format verilog -hierarchy -out gatelevel.gv
```

### Use Yosys: Example script
``` tcl
# if you exited Yosys in step 2, you can read back in your memory_mapped.v yourself.
# read_verilog memory_mapped.v
# hierarchy -check -top TOP_MODULE

# synthesis
synth -flatten
delete t:$print

# change path/to/aigpdk_nomem.lib to a correct path. same for other commands.
dfflibmap -liberty path/to/aigpdk_nomem.lib
opt_clean -purge
abc -liberty path/to/aigpdk_nomem.lib
opt_clean -purge
techmap
abc -liberty path/to/aigpdk_nomem.lib
opt_clean -purge

# write out
write_verilog gatelevel.gv
```

## Step 3. Download and Compile GEM
Make sure CUDA is installed on your Linux machine.

Download and install Rust toolchain. This is as simple as a one-liner in your terminal. We recommend [https://rustup.rs](https://rustup.rs/).

Clone GEM along with its dependency.
``` sh
git clone https://github.com/NVlabs/GEM.git
cd GEM
git submodule update --init --recursive
```

GEM comes with a `cut_map_interactive` command and a `cuda_test` command, that correspond to `compile` and `simulate` steps of a classical CPU simulator. See their help usage with the following command under `GEM`:
``` sh
cargo run -r --features cuda --bin cut_map_interactive -- --help

cargo run -r --features cuda --bin cuda_test -- --help
```

## Map the Design with GEM
~~GEM depends on an external hypergraph partitioner binary. We recommend hmetis 2.0. You can download its binary and put it in a proper location.~~
GEM no longer depends on an external hypergraph partitioner. We now compile and link to [mt-kahypar-sc](https://github.com/gzz2000/mt-kahypar-sc) automatically. This is experimental and if you encounter partitioning issue you can raise it to us.

Run the following command to start the Boolean processor mapping.

``` sh
cargo run -r --features cuda --bin cut_map_interactive -- path/to/gatelevel.gv path/to/result.gemparts
```

The mapped result will be stored in a binary file `result.gemparts`.

If the mapping failed due to failure to partition deep circuits (which often shows as trying to partition a circuit with only 0 or 1 endpoints), try adding a `--level-split` option to force a stage split. For example `--level-split 30` or `--level-split 20,40`. If you used this, remember to add the same `--level-split` option when you simulate.

## Simulate the Design
Run the following. Replace `NUM_BLOCKS` with twice the number of physical streaming multiprocessors (SMs) of your GPU. If ports in your `input.vcd` are not in top-level, add a `--input-vcd-scope` to specify it.
``` sh
cargo run -r --features cuda --bin cuda_test -- path/to/gatelevel.gv path/to/result.gemparts path/to/input.vcd path/to/output.vcd NUM_BLOCKS --input-vcd-scope input/vcd/scope --output-vcd-scope desired/output/scope
```

The simulated output ports value will be stored in `output.vcd`.

**Caveat**: The actual GPU simulation runtime will also be outputted. You might see a long time before GPU enters due to reading and parsing `input.vcd`. You are recommended to develop your own pipeline to feed the input waveform into GEM CUDA kernels.
