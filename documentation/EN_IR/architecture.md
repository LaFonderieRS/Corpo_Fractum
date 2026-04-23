# Architecture

## Pipeline

Every analysis run follows this linear pipeline. Each stage is an independent crate with a well-defined input/output contract.

```
┌──────────────────────────────────────────────────────────────────┐
│  Binary file (.elf / .exe / .dylib / …)                          │
└───────────────────────────┬──────────────────────────────────────┘
                            │
              ┌─────────────▼──────────────┐
              │       rustdec-loader        │
              │  ELF · PE · Mach-O · DWARF  │
              │  → BinaryObject             │
              │  → StringTable              │
              │  → DwarfInfo                │
              └─────────────┬──────────────┘
                            │
              ┌─────────────▼──────────────┐
              │       rustdec-disasm        │
              │  Capstone (multi-arch)      │
              │  → Vec<Instruction>         │
              └─────────────┬──────────────┘
                            │
              ┌─────────────▼──────────────┐
              │      rustdec-analysis       │
              │  function detection         │
              │  CFG construction (3-pass)  │
              │  dominance + loops          │
              │  CFG structuration          │
              │  string recovery            │
              │  call graph                 │
              │  → IrModule (CFG + stmts)   │
              └─────────────┬──────────────┘
                            │  (per function, parallel via rayon)
              ┌─────────────▼──────────────┐
              │        rustdec-lift         │
              │  x86-64 → SSA IR            │
              │  frame analysis             │
              │  dead-code elimination      │
              │  symbol annotation          │
              │  → IrFunction (SSA)         │
              └─────────────┬──────────────┘
                            │
              ┌─────────────▼──────────────┐
              │       rustdec-codegen       │
              │  C / C++ / Rust emission    │
              │  libc signature lookup      │
              │  → Vec<(name, source)>      │
              └─────────────┬──────────────┘
                            │
                  ┌─────────┴─────────┐
                  │                   │
        ┌─────────▼──────┐   ┌────────▼────────┐
        │  rustdec-cli   │   │  rustdec-gui     │
        │  headless CLI  │   │  GTK4 desktop    │
        └────────────────┘   └─────────────────┘
```

---

## Crate responsibilities

| Crate | Responsibility | Key dependencies |
|---|---|---|
| `rustdec-loader` | Parse ELF/PE/Mach-O into `BinaryObject`; DWARF debug info | goblin, gimli |
| `rustdec-disasm` | Disassemble raw bytes into `Vec<Instruction>` | capstone-rs |
| `rustdec-ir` | Define all IR types: `IrType`, `Stmt`, `Expr`, `IrFunction`, `IrModule` | petgraph |
| `rustdec-lift` | Lift x86-64 instructions to SSA; analyse stack frames | rustdec-ir, rustdec-disasm |
| `rustdec-analysis` | Orchestrate the full pipeline; CFG, structuration, string recovery | rayon, petgraph |
| `rustdec-codegen` | Emit pseudo-code from `IrModule` | rustdec-ir, rustdec-lift |
| `rustdec-cli` | Parse CLI arguments; drive the pipeline headlessly | clap, anyhow |
| `rustdec-gui` | GTK4 desktop application; async bridge to Tokio backend | gtk4, cairo-rs, tokio |
| `rustdec-bench` | Benchmark harness for the analysis pipeline | clap, serde |
| `rustdec-lua` | Lua plugin engine (stub) | mlua |

---

## Parallelism

Two levels of parallelism are used:

**Stage 1 — data parallelism (rayon::join):**  
Disassembly and string extraction run concurrently on the same binary object.

**Stage 2 — function parallelism (rayon::par_iter):**  
CFG construction and lifting run in parallel across all detected functions. Each function is independent; results are collected and assembled into `IrModule`.

The GUI adds a third level: the entire analysis pipeline runs inside a `tokio::task::spawn_blocking` call so the GTK main thread is never stalled.

---

## Error handling

Every crate defines its own error enum via `thiserror`. Errors propagate up as `Result<T, E>` — there are no panics in the hot path. The CLI and GUI catch top-level errors and display them to the user.

---

## Logging

All crates use `tracing` macros (`trace!`, `debug!`, `info!`, `warn!`, `error!`). The subscriber is configured by the entry point (CLI or GUI). The filter is driven by the `RUSTDEC_LOG` environment variable (same syntax as `RUST_LOG`).

---

## Design principles

**Architecture-independent IR.**  
After lifting, no registers or architecture-specific concepts remain in the IR. All subsequent passes — structuration, codegen — are pure IR operations.

**Shared immutable data via `Arc`.**  
`IrTypeRef = Arc<IrType>` lets the same type node be referenced from thousands of SSA variables without cloning. `Arc<str>` interns symbol names and import names across the whole module.

**No unsafe in the hot path.**  
FFI to Capstone and GTK4 is encapsulated in the respective wrapper crates. The analysis and IR crates contain no `unsafe`.

**Incremental output.**  
The GUI receives `AnalysisFunctionReady` events as each function completes, so the user can start reading results before the full analysis finishes.
