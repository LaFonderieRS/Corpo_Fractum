# RustDec

**Binary decompiler written in 100% Rust — ELF · PE · Mach-O → C / C++ / Rust**

> MVP status: loader + disassembler + CFG builder + C codegen + GTK4 UI skeleton.

---

## Features (MVP)

| Feature | Status |
|---------|--------|
| ELF 32/64 parsing | ✅ |
| PE / PE+ parsing | ✅ |
| Mach-O / Fat binary parsing | ✅ |
| x86-64 disassembly (Capstone) | ✅ |
| ARM64 disassembly | ✅ |
| CFG construction | ✅ |
| Function detection (symbols + call scan) | ✅ |
| C pseudo-code generation | ✅ |
| C++ pseudo-code generation | ✅ |
| Rust pseudo-code generation | ✅ |
| GTK4 UI (explorer + code + graph panels) | ✅ skeleton |
| Dark theme | ✅ |
| Async backend (Tokio) | ✅ |

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
git clone https://github.com/yourorg/rustdec
cd rustdec
cargo build --release
```

### Run

```bash
./target/release/rustdec
# or with debug logging:
RUSTDEC_LOG=debug ./target/release/rustdec
```

### Tests

```bash
cargo test                     # all unit + integration tests
cargo test -p rustdec-loader   # loader only
cargo test -p rustdec-disasm   # disassembler only
```

---

## Workspace layout

```
rustdec/
├── Cargo.toml                  # workspace root
├── crates/
│   ├── rustdec-loader/         # ELF / PE / Mach-O parser  (goblin)
│   ├── rustdec-disasm/         # multi-arch disassembler   (capstone-rs)
│   ├── rustdec-ir/             # SSA intermediate representation
│   ├── rustdec-analysis/       # CFG builder, function detection
│   └── rustdec-codegen/        # C / C++ / Rust code generators
├── rustdec-gui/                # GTK4 application (main binary)
└── tests/                      # integration tests
```

---

## Architecture

```
Binary file
    │
    ▼
rustdec-loader      (goblin)
    │  BinaryObject
    ▼
rustdec-disasm      (capstone-rs)
    │  Vec<Instruction>
    ▼
rustdec-analysis
    │  IrModule (CFG per function)
    ▼
rustdec-codegen
    │  String (C / C++ / Rust)
    ▼
rustdec-gui         (gtk4 + cairo + tokio)
```

---

## Roadmap

- **MVP** (current): loader, disasm x86-64, CFG, C codegen, GTK skeleton
- **V1**: ARM64/RISC-V, C++/Rust codegen, interactive call graph, multi-file
- **V2**: plugin API, AI-assisted renaming, dynamic debugging (ptrace)

---

## License

MIT OR Apache-2.0
