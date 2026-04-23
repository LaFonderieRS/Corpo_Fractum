# Corpo Fractum — Documentation

Binary decompiler written in Rust: ELF/PE/Mach-O → SSA IR → C / C++ / Rust pseudo-code.

---

## Contents

| Document | Description |
|---|---|
| [Architecture](architecture.md) | Pipeline overview, crate graph, design principles |
| [IR](ir.md) | SSA intermediate representation: types, values, statements, control flow |
| [Lifter](lift.md) | x86-64 → SSA IR: register table, flags, frame analysis, array detection |
| [Analysis](analysis.md) | CFG construction, dominance, structuration, string recovery, call graph |
| [Code generation](codegen.md) | C / C++ / Rust backends, libc signatures, CRT filtering |
| [CLI](cli.md) | Command-line interface reference |
| [GUI](gui.md) | Graphical interface user guide |

---

## Quick orientation

```
Binary file  ──►  rustdec-loader   parse format + DWARF
             ──►  rustdec-disasm   disassemble to instructions
             ──►  rustdec-analysis build CFG, detect functions, structure flow
             ──►  rustdec-lift     lift x86-64 → SSA IR, name stack slots
             ──►  rustdec-codegen  emit C / C++ / Rust source
             ──►  rustdec-cli      headless CLI
             ──►  rustdec-gui      GTK4 desktop application
```

All crates live under the workspace root. There are no sub-directories — each crate is a direct child of the repository root.
