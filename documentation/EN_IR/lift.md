# Lifter

`rustdec-lift` translates x86-64 machine instructions into SSA IR statements. It operates on one function at a time and is called by `rustdec-analysis` in parallel across all detected functions.

---

## Entry point

```rust
pub fn lift_function(func: &mut IrFunction, insns: &[Instruction], symbols: &SymbolMap)
```

The caller provides:
- `func` — an `IrFunction` whose CFG has already been built (basic block graph, no statements yet).
- `insns` — the flat instruction slice for the whole function.
- `symbols` — the symbol map used for constant-to-symbol resolution.

After this call, every basic block in the CFG has its `stmts` and `terminator` populated.

---

## Pipeline inside `lift_function`

```
1. Per-block lifting (parallel via rayon)
     lift_block_with_regs(block_insns) → (stmts, ret_var, reg_names)

2. Frame analysis
     analyse_frame(func)
     → discover stack slots, name locals/args/saved-regs
     → rewrite [rbp±N] / [rsp±N] loads/stores to named slots
     → detect contiguous arrays, rewrite as ArrayAccess

3. Constant-to-symbol resolution
     scan all Const values against the symbol map
     → replace matching addresses with Expr::Symbol{kind, name}

4. Dead-code elimination
     mark unused Assign statements as Nop

5. Return-type inference
     inspect Return terminators to determine ret_ty

6. ABI arity inference
     count distinct argument registers used → set func.params

7. Stack-canary elimination
     remove __stack_chk_fail paths and their guards
```

---

## Register table

`RegisterTable` maps physical x86-64 registers to SSA variable ids. It handles the full register alias hierarchy:

```
rax (64) → eax (32) → ax (16) → al / ah (8)
rcx, rdx, rbx, rsp, rbp, rsi, rdi
r8 – r15  (with r8d, r8w, r8b aliases)
xmm0 – xmm15
```

Reading a narrow alias (e.g. `eax`) zero-extends from the current 64-bit variable. Writing a 32-bit register zero-extends to 64 bits (x86-64 ABI rule). Writing an 8/16-bit alias inserts a mask-and-or.

The ABI register seed pre-allocates System V x86-64 registers before lifting begins:

| Class | Registers |
|---|---|
| Arguments | rdi, rsi, rdx, rcx, r8, r9 |
| Return value | rax |
| Callee-saved | rbx, r12, r13, r14, r15 |
| Frame pointers | rsp, rbp |

---

## Flag tracker

`FlagTracker` maintains live SSA variables for the four arithmetic flags after each instruction that sets them:

| Flag | Updated by |
|---|---|
| ZF (zero) | add, sub, cmp, test, and, or, xor, inc, dec, … |
| SF (sign) | same |
| CF (carry) | add, sub, adc, sbb, shl, shr, … |
| OF (overflow) | add, sub, imul, … |

Conditional jump instructions (`jz`, `jnz`, `jl`, `jge`, `jb`, …) read the appropriate flag variable and emit a `Branch` terminator with the flag value as the condition. The original mnemonic is preserved in `Branch::mnemonic`.

---

## Instruction coverage

`lift_block_with_regs` handles over 100 x86-64 mnemonics, including:

**Data movement:** `mov`, `movzx`, `movsx`, `movsxd`, `lea`, `push`, `pop`, `xchg`

**Arithmetic:** `add`, `sub`, `imul`, `mul`, `idiv`, `div`, `inc`, `dec`, `neg`

**Logic / shifts:** `and`, `or`, `xor`, `not`, `shl`, `shr`, `sar`, `rol`, `ror`

**Comparisons:** `cmp`, `test`, `setcc` family

**Control flow:** `call`, `ret`, `jmp`, `jcc` family (all 16 conditions), `ud2`, `hlt`

**String / SSE:** `rep movs`, `rep stos`, `movss`, `movsd`, `addss`, `addsd`, basic vector loads

Unrecognised instructions produce `Expr::Opaque(mnemonic)` so lifting never hard-fails.

---

## Frame analysis

`analyse_frame` is a separate pass that runs after block lifting. It:

### Prologue detection

Identifies the standard x86-64 frame prologue:
```asm
push rbp
mov  rbp, rsp
sub  rsp, N        ← extracts frame_size = N
```

Removes the prologue assignments from the IR (they are ABI boilerplate, not logic).

### Epilogue detection

Removes `leave` (equivalent to `mov rsp, rbp; pop rbp`) and bare `pop rbp` patterns.

### Callee-saved register elimination

Removes push/pop pairs for rbx, r12, r13, r14, r15 at function entry/exit.

### Stack slot discovery

Scans every `Load` and `Store` expression for `[rbp ± offset]` and `[rsp ± offset]` patterns. Each unique offset becomes a named `StackSlot`:

| Offset range | Origin | Name pattern |
|---|---|---|
| rbp - N (N > 0) | `Local` | `local_0`, `local_1`, … |
| rbp + N (N > 8) | `StackArg` | `arg_0`, `arg_1`, … |
| callee-saved spills | `SavedReg` | `saved_rbx`, `saved_r12`, … |

### Array detection

When multiple adjacent stack slots have identical type and uniform stride, they are merged into a single slot with `ArrayInfo { count, stride }`. Accesses are rewritten as `ArrayAccess { name, index, elem_ty }` expressions.

Example:
```c
// before detection
local_0 = …;  local_8 = …;  local_16 = …;

// after detection
buf[0] = …;   buf[1] = …;   buf[2] = …;
```

### Red zone

Leaf functions on x86-64 may use the 128-byte red zone below RSP without adjusting the stack pointer. The frame analyser detects this pattern and names the slots accordingly.

### Dynamic allocation

`sub rsp, reg` (where the right-hand side is a variable, not a constant) is recognised as a dynamic `alloca` and marked separately.

---

## Copy propagation

After frame analysis, the lifter performs a light copy-propagation pass on stack slots: if a slot is written exactly once with a constant and read without intervening writes, the slot reference is replaced with the constant inline.

---

## Stack canary

If `__stack_chk_fail` is detected as a call target, the lifter removes the entire canary check sequence:
- The `fs:0x28` load into a local slot.
- The XOR comparison at function exit.
- The conditional branch to `__stack_chk_fail`.
- The `__stack_chk_fail` call block itself.

This cleans up the output significantly for any binary compiled with `-fstack-protector`.
