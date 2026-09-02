## Task/Problem Statement

The challenge is to fork the official NVIDIA GEM simulator codebase (https://github.com/NVlabs/GEM) and architect a heterogeneous GPU-accelerated logic simulator. Teams must modify GEM's existing boomerang scheduling layer to natively evaluate word-level hardware macros without allowing the frontend to shred them into primitive bitwise gates.

### Required Implementation Areas

At the software and theoretical level, the following must be modeled and implemented:

1. **Boomerang Scheduler Extension**: Modifying the Levelized Directed Acyclic Graph (DAG) scheduling equations to group and schedule mixed-width operations (1-bit boolean And-Inverter graphs executing alongside 48-bit arithmetic math) without stalling CUDA warps.

2. **Heterogeneous Memory Allocator**: Designing a memory packing strategy in GPU VRAM that maps 1-bit boolean states alongside 64-bit aligned contiguous memory blocks for arithmetic macros, enabling coalesced reads.

3. **Native Macro Evaluation**: Executing standard functional units on the GPU ALU using native `int64_t` types without SIMT warp divergence.

### Required Hardware Primitives

To ensure standardization across all participating halls, teams are strictly required to implement native GPU evaluation models for the following specific hardware primitives, mapped directly to standard Xilinx FPGA architectures:

#### A. The MAC Unit (DSP48E2 Simplified Subset)

To eliminate ambiguity regarding pipeline delays, the DSP model is constrained to the following precise configuration:

- All input/internal registers (AREG, BREG, CREG, DREG, ADREG, MREG) are strictly combinational (set to 0)
- Only the final accumulator output register (PREG) is clocked (set to 1)
- The logic must compute in a single pass using signed two's-complement arithmetic

**Pre-Adder Logic**: Implement the 27-bit D input, 27-bit A input, and 18-bit B input. The pre-adder computes `AD = A + D` or passes A directly.

**Multiplier Logic**: Compute the 45-bit combinational product `M = AD * B`.

**48-bit ALU & Control**: A 48-bit ALU that writes to the clocked P register. To avoid parsing the complex 9-bit Xilinx OPMODE, the Yosys parser must extract the intent and pass a simplified 2-bit state to the GPU kernel:

- State 0: Bypass (`P_next = C`)
- State 1: Multiply-Only (`P_next = M`)
- State 2: Multiply-Accumulate (`P_next = P_current + M`)

**Note on Output Pins**: You can safely ignore the dedicated OVERFLOW and UNDERFLOW output pins present on real DSP48E2 blocks. Only simulate the core 48-bit P output as defined above.

#### B. The Carry Chain (CARRY4 Primitive)

Teams must model the exact Xilinx CARRY4 silicon block. The model must accept:

- 4-bit sum input `S[3:0]`
- 4-bit data input `DI[3:0]`
- Cascade carry-in `CIN`
- Carry initialization bit `CYINIT`

It must compute the 4-bit carry-out `CO[3:0]` and the XOR'd result `O[3:0]` in a single GPU execution step using the following strict logic:

- `C[0] = CYINIT | CIN` (In valid RTL, only one of these is active)
- `C[i+1] = (S[i] & C[i]) | (~S[i] & DI[i])` for i in 0 to 3
- `O[i] = S[i] ^ C[i]` for i in 0 to 3
- `CO[i] = C[i+1]` for i in 0 to 3

#### C. Shift Register LUT (SRLC32E Primitive)

A 32-bit shift register that strictly follows clock-edge synchronization.

**Inputs**:
- 1-bit Data (D)
- 1-bit Clock Enable (CE)
- 5-bit Address (A[4:0])

**Behavior**: On the global rising edge, if CE == 1, the internal 32-bit state shifts left (LSB to MSB), and D is loaded into index 0.

**Outputs**:
- The read port natively outputs the bit at the dynamic address `A[4:0]` combinationally
- The cascade port Q31 always outputs the bit at index 31 combinationally


