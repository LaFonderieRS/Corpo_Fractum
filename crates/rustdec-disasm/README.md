# rustdec-disasm

Multi-architecture disassembler built on [Capstone](https://www.capstone-engine.org/).
Converts raw byte slices into typed `Instruction` values that the rest of the
pipeline — CFG builder, lifter, function detector — can query without
touching Capstone directly.

## Pipeline position

```
BinaryObject ──► rustdec-disasm ──► Vec<Instruction>
                                          │
                                          ▼
                               rustdec-analysis / rustdec-lift
```

## Key types

| Type | Description |
|---|---|
| `Disassembler` | Holds the Capstone context for a given architecture |
| `Instruction` | One decoded instruction: address, bytes, mnemonic, operands, size |
| `DisasmError` | `UnsupportedArch` or a raw Capstone error |

## Public API

```rust
use rustdec_disasm::Disassembler;
use rustdec_loader::Arch;

let disasm = Disassembler::for_arch(Arch::X86_64)?;
let insns  = disasm.disassemble(&section.data, section.virtual_addr)?;

for insn in &insns {
    println!("{}", insn.display());   // "0x00401000  ret       "

    if insn.is_terminator() { /* ret, jmp, hlt, ud2 */ }
    if insn.is_branch()     { /* je, jne, jl, … */    }
    if insn.is_call()       { /* call, callq, lcall */ }

    if let Some(target) = insn.branch_target() {
        println!("  → {target:#x}");
    }
}
```

## Instruction classification

| Method | Returns `true` for |
|---|---|
| `is_terminator()` | `ret`/`retq`, `jmp`/`jmpq`, `hlt`, `ud2`, `int3` |
| `is_branch()` | All conditional jumps (`je`, `jne`, `jl`, `jge`, …) |
| `is_call()` | `call`/`callq`, `lcall` |

> **Note on AT&T syntax:** Capstone emits size-suffixed mnemonics in AT&T mode
> (`"retq"`, `"callq"`, `"jmpq"`).  All three classification methods handle
> both the bare Intel form and the suffixed AT&T form transparently.

## `branch_target()` extraction

Parses the operand string to extract a direct code address.  Returns `None`
for indirect targets (e.g. `jmp *%rax`) and for addresses below `0x1000`
(which are almost certainly immediates, not code addresses).

Handles both `0x`-prefixed hex (`0x401234`) and bare decimal emitted by some
Capstone versions for short jumps.

## Supported architectures

| `Arch` variant | Notes |
|---|---|
| `X86` | 32-bit, AT&T syntax |
| `X86_64` | 64-bit, AT&T syntax |
| `Arm32` | ARM (32-bit), ARM mode |
| `Arm64` | AArch64 |
| `RiscV64` | RISC-V 64 |

Passing `Arch::Unknown` or any unlisted variant returns `DisasmError::UnsupportedArch`.

## Dependencies

- [`capstone`](https://crates.io/crates/capstone) — Capstone bindings
- [`rustdec-loader`](../rustdec-loader) — `Arch` enum
