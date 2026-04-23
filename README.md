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

The entire toolchain — loader, disassembler, IR, analysis, codegen, CLI and UI — is written in pure Rust.

---

## Current focus

| Area | Status |
|---|---|
| ELF 64-bit parsing | ✅ |
| ELF 32-bit parsing | ✅ |
| PE / PE+ parsing | ✅ |
| Mach-O / Fat binary parsing | ✅ |
| DWARF debug info (types, locals, line info) | ✅ |
| x86-64 disassembly (Capstone, Intel syntax) | ✅ |
| ARM64 disassembly | ✅ |
| CFG construction (3-pass, branch-aware) | ✅ |
| Function detection (symbols + call-site scan + jump tables) | ✅ |
| x86-64 instruction lifter → SSA IR | ✅ |
| Stack frame analysis (local variable naming, array detection) | ✅ |
| ASCII / C-string recognition | ✅ |
| String recovery (multi-encoding, confidence scoring) | ✅ |
| Dominator tree + natural loop detection | ✅ |
| CFG structuration (if/else · while · break/continue) | ✅ |
| Call graph construction | ✅ |
| C pseudo-code generation | ✅ |
| C++ pseudo-code generation | ✅ |
| Rust pseudo-code generation | ✅ |
| Memory optimisations (Arc string interning, shared IrTypeRef) | ✅ |
| Command-line interface (CLI) | ✅ |
| Async analysis backend (Tokio) | ✅ |
| Benchmark suite | ✅ |
| GTK4 UI (explorer · code · graph panels) | 🚧 skeleton |
| Lua plugin engine | 🚧 stub |

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

### Run (GUI)

```bash
./target/release/corpo_fractum

# With debug logging:
RUSTDEC_LOG=debug ./target/release/corpo_fractum

# Filter logs by crate:
RUSTDEC_LOG=corpo-fractum=debug,info ./target/release/corpo_fractum
```

Console panel placement (GTK4 build features):

```bash
# Default — no console panel
cargo build --release

# Console below code + graph:
cargo build --release --features console-bottom

# Console as a separate tab next to Graph:
cargo build --release --features console-tab
```

### Run (CLI)

```bash
./target/release/corpo-fractum-cli [OPTIONS] <BINARY>
```

| Flag | Description |
|---|---|
| `-l, --lang <LANG>` | Output language: `c` (default), `cpp`, `rust` |
| `-o, --output <DIR>` | Write one file per function into DIR |
| `-F, --function <NAME>` | Only decompile the named function(s) |
| `--list` | List detected functions and exit (no full analysis) |
| `--emit-ir` | Dump the lifted SSA IR instead of decompiled code |
| `-v / -vv / -vvv` | Increase log verbosity (info / debug / trace) |

Examples:

```bash
# List all functions in a binary
./target/release/corpo-fractum-cli --list ./my_binary

# Decompile a specific function to C
./target/release/corpo-fractum-cli -F main ./my_binary

# Dump IR for debugging
./target/release/corpo-fractum-cli --emit-ir -F compute ./my_binary

# Write all functions as Rust pseudo-code to ./out/
./target/release/corpo-fractum-cli -l rust -o ./out ./my_binary
```

### Tests

```bash
cargo test                       # all tests
cargo test -p rustdec-loader     # loader only
cargo test -p rustdec-disasm     # disassembler only
```

---

## Usage (library)

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

### DWARF debug information

```rust
use rustdec_loader::dwarf::parse;

if let Some(info) = parse(&obj) {
    for cu in &info.units {
        for func in &cu.functions {
            println!("{} — {} params, {} locals",
                func.name, func.params.len(), func.locals.len());
        }
    }
}
```

### String recovery

```rust
use rustdec_analysis::string_recovery::StringRecovery;

let mut recovery = StringRecovery::new(&obj, &string_table);
let strings = recovery.recover_strings_from_binary();
for s in &strings {
    println!("{:#x}  {:?}  confidence={:.2}", s.address, s.content, s.confidence);
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
│   ├── rustdec-loader/          # ELF / PE / Mach-O parser + DWARF  (goblin, gimli)
│   ├── rustdec-disasm/          # multi-arch disassembler             (capstone-rs)
│   ├── rustdec-ir/              # SSA intermediate representation
│   ├── rustdec-analysis/        # CFG, function detection, dominance, structuration,
│   │                            #   call graph, string recovery
│   ├── rustdec-lift/            # x86-64 instruction lifter + frame analysis
│   ├── rustdec-codegen/         # C / C++ / Rust code generators
│   ├── rustdec-cli/             # command-line interface              (clap)
│   ├── rustdec-bench/           # benchmark suite
│   └── rustdec-lua/             # Lua plugin engine                   (mlua — stub)
├── rustdec-gui/                 # GTK4 application                    (main binary)
└── tests/                       # integration tests
```

---

## Architecture

```
Binary file
    │
    ▼
rustdec-loader          ELF / PE / Mach-O → BinaryObject + StringTable + DwarfInfo
    │
    ▼
rustdec-disasm          Capstone → Vec<Instruction>
    │
    ▼
rustdec-analysis        function detection · CFG (3-pass) · dominance · structuration
    │                   call graph · string recovery (multi-encoding, confidence scoring)
    ▼
rustdec-lift            x86-64 → SSA IR · stack frame analysis · array detection
    │                   string/symbol annotation · dead-code elimination
    ▼
rustdec-codegen         IrModule → C / C++ / Rust pseudo-code
    │
    ├──► rustdec-cli    headless CLI  (--list / --emit-ir / --lang / -F / -o)
    └──► rustdec-gui    GTK4 · Cairo · Tokio
```

---

## IR type system

```
IrType
 ├── UInt(bits)          unsigned integer (8 / 16 / 32 / 64)
 ├── SInt(bits)          signed integer
 ├── Float(bits)         32- or 64-bit float
 ├── Ptr(IrType)         typed pointer
 ├── Array { elem, len } contiguous array
 ├── Struct { name, size } opaque struct
 ├── Void
 └── Unknown

IrTypeRef = Arc<IrType>   — types are reference-counted and shared across the IR
Arc<str>                  — symbol names and call targets are interned strings
```

---

## Roadmap

- **MVP** (current) — ELF x86-64 · SSA IR · C/C++/Rust codegen · stack frame naming · CFG structuration · CLI · GTK4 skeleton
- **V1** — ARM64 / RISC-V lifting · improved type inference · interactive call graph · multi-file projects · full Lua plugin API
- **V2** — AI-assisted renaming · dynamic analysis (ptrace) · deobfuscation passes

---

## License

GPL v3.0
