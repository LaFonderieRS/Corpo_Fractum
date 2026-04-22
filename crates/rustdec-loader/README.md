# rustdec-loader

Parses ELF, PE, and Mach-O binaries into a normalised, format-agnostic
`BinaryObject`.  Everything downstream — the disassembler, lifter, and
code generators — works exclusively against this type and never needs to
re-parse raw bytes.

## Pipeline position

```
raw bytes  ──►  rustdec-loader  ──►  BinaryObject
                                          │
                                          ▼
                                   rustdec-disasm / rustdec-analysis
```

## Key types

| Type | Description |
|---|---|
| `BinaryObject` | Normalised binary: format, arch, sections, symbols, DWARF, entry point |
| `Arch` | CPU architecture (`X86_64`, `Arm64`, `RiscV64`, …) |
| `Format` | Container format (`Elf`, `Pe`, `MachO`) |
| `Section` | A contiguous memory region with VA, raw data, and kind (`Code`, `Data`, …) |
| `Symbol` | Named address with kind (`Function`, `Object`, `Import`, …) |
| `DwarfInfo` | Parsed DWARF debug info (compilation units, function signatures, types, line numbers) |
| `SymbolMap` | Unified VA → symbol table built from strings + DWARF + ELF/PE symbols |

## Public API

```rust
// Load from disk
let obj = rustdec_loader::load_file("binary")?;

// Load from memory (useful in tests or GUI drag-and-drop)
let obj = rustdec_loader::load_bytes(&bytes)?;

// Iterate executable sections
for sec in obj.code_sections() {
    println!("{} @ {:#x} ({} bytes)", sec.name, sec.virtual_addr, sec.size);
}

// Build the unified symbol map (pass to the lifter)
let symbol_map = rustdec_loader::build_symbol_map(&obj, &string_table);

// Extract string literals from read-only sections
let strings = rustdec_loader::extract_strings(&obj);
```

## Symbol map priority

When multiple sources describe the same address the first match wins:

1. String literals (`.rodata` / `.rdata` / `__cstring`)
2. DWARF function / variable names (highest-quality debug info)
3. ELF / PE / Mach-O symbol table entries

## Supported formats

| Format | 32-bit | 64-bit | Notes |
|---|---|---|---|
| ELF | ✓ | ✓ | Linux, BSDs, embedded |
| PE | ✓ | ✓ | Windows executables and DLLs |
| Mach-O | ✓ | ✓ | macOS/iOS; fat binaries supported |
| DWARF | — | — | Embedded in any of the above; optional |

## Dependencies

- [`goblin`](https://crates.io/crates/goblin) — raw ELF / PE / Mach-O parsing
- [`gimli`](https://crates.io/crates/gimli) — DWARF debug info parsing
