 ██████╗ ██████╗ ██████╗ ██████╗  ██████╗     ███████╗██████╗  █████╗  ██████╗████████╗██╗   ██╗███╗   ███╗
██╔════╝██╔═══██╗██╔══██╗██╔══██╗██╔═══██╗    ██╔════╝██╔══██╗██╔══██╗██╔════╝╚══██╔══╝██║   ██║████╗ ████║
██║     ██║   ██║██████╔╝██████╔╝██║   ██║    █████╗  ██████╔╝███████║██║        ██║   ██║   ██║██╔████╔██║
██║     ██║   ██║██╔══██╗██╔═══╝ ██║   ██║    ██╔══╝  ██╔══██╗██╔══██║██║        ██║   ██║   ██║██║╚██╔╝██║
╚██████╗╚██████╔╝██║  ██║██║     ╚██████╔╝    ██║     ██║  ██║██║  ██║╚██████╗   ██║   ╚██████╔╝██║ ╚═╝ ██║
 ╚═════╝ ╚═════╝ ╚═╝  ╚═╝╚═╝      ╚═════╝     ╚═╝     ╚═╝  ╚═╝╚═╝  ╚═╝ ╚═════╝   ╚═╝    ╚═════╝ ╚═╝     ╚═╝

                             ░▒▓ CORPO FRACTUM ▓▒░

                  From gods came man. From binary, came code.

        [ dissecting structure ]  [ lifting instructions ]  [ rebuilding meaning ]

-----------------------------------------------------------------------------------------------------------

## Overview

Corpo Fractum is an open-source binary decompiler targeting x86-64 ELF binaries first, with PE and Mach-O support on the roadmap. It lifts machine code to a typed SSA intermediate representation and emits readable C pseudo-code, with C++ and Rust backends also available.

The entire toolchain — loader, disassembler, IR, analysis, codegen and UI — is written in pure Rust.

---

## Current focus

| Area | Status |
|---|---|
| ELF 64-bit parsing | ✅ |
| x86-64 disassembly (Capstone, Intel syntax) | ✅ |
| CFG construction (3-pass, branch-aware) | ✅ |
| Function detection (symbols + call-site scan + jump tables) | ✅ |
| x86-64 instruction lifter → SSA IR | ✅ |
| Stack frame analysis (local variable naming) | ✅ |
| ASCII / C-string recognition | ✅ |
| C pseudo-code generation | ✅ |
| Dominator tree + natural loop detection | ✅ |
| GTK4 UI (explorer · code · graph panels) | 🚧 skeleton |

## Also supported

| Area | Status |
|---|---|
| ELF 32-bit parsing | ✅ |
| PE / PE+ parsing | ✅ |
| Mach-O / Fat binary parsing | ✅ |
| ARM64 disassembly | ✅ |
| C++ pseudo-code generation | ✅ |
| Rust pseudo-code generation | ✅ |
| Async analysis backend (Tokio) | ✅ |
| Dark theme | ✅ |

---

![image](MISC/logo.png)

---

## Quick start

### Dependencies

```bash
# Debian / Ubuntu
sudo apt install libgtk-4-dev libcairo2-dev pkg-config

# Arch Linux
sudo pacman -S gtk4 cairo pkgconf

# Fedora
sudo dnf install gtk4-devel cairo-devel
```

### Build

```bash
cargo build --release
```

### Run

```bash
./target/release/corpo_fractum

# With debug logging:
RUSTDEC_LOG=debug ./target/release/corpo_fractum

# Filter logs by crate:
RUSTDEC_LOG=rustdec_analysis=debug,info ./target/release/corpo_fractum
```

New feature, the console :

```bash                                                                                                                                                                             
cargo build --release

# With bottom console panel (below code + graph):
cargo build --release --features console-bottom

# With tabbed console panel (Console tab next to Graph):
cargo build --release --features console-tab
```

### Tests

```bash
cargo test                       # all tests
cargo test -p rustdec-loader     # loader only
cargo test -p rustdec-disasm     # disassembler only
```

---

## Usage

### Load a binary

```rust
use rustdec_loader::load_file;

let obj = load_file("/path/to/binary")?;
println!("format={:?}  arch={}  64-bit={}", obj.format, obj.arch, obj.is_64bit);
println!("{} sections, {} symbols", obj.sections.len(), obj.symbols.len());
```

### Run the full analysis pipeline

```rust
use rustdec_loader::load_file;
use rustdec_analysis::analyse;

let obj = load_file("/path/to/binary")?;
let module = analyse(&obj)?;
println!("{} functions lifted", module.functions.len());
```

### Emit pseudo-code

```rust
use rustdec_codegen::{emit_module, Language};

// module from analyse() above
let output = emit_module(&module, Language::C)?;
for (name, src) in &output {
    println!("// ── {name} ──\n{src}");
}
```

### Work with the IR directly

```rust
use rustdec_ir::{IrType, Value, Expr, BinOp};

// Inspect a specific function
let func = &module.functions[0];
println!("fn {}  @ {:#x}  frame={} bytes", func.name, func.entry_addr, func.frame_size);

for block in func.blocks_sorted() {
    println!("  bb{} [{:#x}..{:#x}]", block.id, block.start_addr, block.end_addr);
    for stmt in &block.stmts {
        println!("    {stmt:?}");
    }
    println!("    → {:?}", block.terminator);
}
```

### Log filtering

```bash
RUSTDEC_LOG=rustdec_analysis=debug,info ./target/release/corpo_fractum
```

---

## Workspace layout

```
corpo_fractum/
├── Cargo.toml                   # workspace root
├── crates/
│   ├── rustdec-loader/          # ELF / PE / Mach-O parser      (goblin)
│   ├── rustdec-disasm/          # multi-arch disassembler        (capstone-rs)
│   ├── rustdec-ir/              # SSA intermediate representation
│   ├── rustdec-analysis/        # CFG, function detection, dominance, structuration
│   ├── rustdec-lift/            # x86-64 instruction lifter + frame analysis
│   ├── rustdec-codegen/         # C / C++ / Rust code generators
│   └── rustdec-plugin/          # Lua plugin engine              (mlua — stub)
├── rustdec-gui/                 # GTK4 application               (main binary)
└── tests/                       # integration tests
```

---

## Architecture

```
Binary file
    │
    ▼
rustdec-loader          ELF / PE / Mach-O → BinaryObject + StringTable
    │
    ▼
rustdec-disasm          Capstone → Vec<Instruction>
    │
    ▼
rustdec-analysis        CFG · function detection · dominance · structuration
    │
    ▼
rustdec-lift            x86-64 → SSA IR · stack frame analysis · string annotation
    │
    ▼
rustdec-codegen         IrModule → C / C++ / Rust pseudo-code
    │
    ▼
rustdec-gui             GTK4 · Cairo · Tokio
```

---

## Roadmap

- **MVP** (current) — ELF x86-64 · SSA IR · C codegen · stack frame naming · GTK4 skeleton
- **V1** — ARM64 / RISC-V lifting · improved type inference · interactive call graph · multi-file projects
- **V2** — Lua plugin API · AI-assisted renaming · dynamic analysis (ptrace)

---

## License

GPL v3.0
