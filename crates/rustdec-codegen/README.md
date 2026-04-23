# rustdec-codegen

Code generation backends for RustDec: translates the SSA IR produced by the
analysis pipeline into readable pseudo-code in C, C++, or Rust.

## Pipeline position

```
IrModule ──► rustdec-codegen::emit_module ──► Vec<(name, source_code)>
```

## Quick start

```rust
use rustdec_codegen::{emit_module, Language};

// `module` comes from `rustdec_analysis::analyse`.
let results = emit_module(&module, Language::C)?;

for (func_name, source) in &results {
    println!("// --- {} ---\n{}", func_name, source);
}
```

## `emit_module`

Iterates over every function in the `IrModule`, skips known CRT/runtime
symbols (see below), and calls the selected backend's `emit_function` for
each remaining function.

Returns `Vec<(String, String)>` — one `(function_name, source_code)` pair
per emitted function.

## Backends

### C (`Language::C`) — `c::CBackend`

Emits C99 pseudo-code using structured constructs derived from the CFG.

**Type mapping:**

| `IrType` | C type |
|---|---|
| `UInt(8)` | `char` |
| `UInt(16)` | `uint16_t` |
| `UInt(32)` | `uint32_t` |
| `UInt(64)` | `uint64_t` |
| `SInt(8)` | `int8_t` |
| `SInt(16)` | `int16_t` |
| `SInt(32)` | `int` |
| `SInt(64)` | `int64_t` |
| `Float(32)` | `float` |
| `Float(64)` | `double` |
| `Ptr(T)` | `T*` |
| `Array { T, N }` | `T[N]` |
| `Struct { name, … }` | `struct name` |
| `Void` | `void` |

**Known libc signatures** — if the function name matches an entry in the
built-in table (`printf`, `malloc`, `fopen`, …) the declared signature
overrides the inferred arity and types, so `uint64_t main(uint64_t a0, ...)`
becomes `int main(int argc, char** argv)` automatically.

**Structured output** — the emitter calls `rustdec_analysis::structure_function`
to convert the flat CFG into `if`/`while` constructs rather than emitting
`goto`-based spaghetti.

**Copy propagation** — single-assignment variables used exactly once are
inlined at their use site so the output is free of redundant temporaries.

### C++ (`Language::Cpp`) — `cpp::CppBackend`

Same structure as the C backend; type map uses C++ integer literals
(`uint64_t`, etc.).  Class / method reconstruction is not yet implemented.

### Rust (`Language::Rust`) — `rust::RustBackend`

Emits `fn` bodies with Rust integer types (`u64`, `i32`, `f32`, …).
Ownership and borrowing annotations are not inferred; all pointers become
raw `*const T` / `*mut T`.

## CRT / runtime filter

The following symbols are silently skipped during `emit_module` because they
carry no reverse-engineering value:

```
_init  _fini  _start  __libc_start_main  __libc_csu_init  __libc_csu_fini
__do_global_dtors_aux  deregister_tm_clones  register_tm_clones
frame_dummy  _dl_relocate_static_pie
__DllMainCRTStartup  _DllMainCRTStartup  mainCRTStartup  WinMainCRTStartup
```

These functions are retained in the `IrModule` for call-graph analysis; only
code generation is suppressed.

## `CodegenBackend` trait

You can implement your own backend:

```rust
use rustdec_codegen::{CodegenBackend, CodegenResult};
use rustdec_ir::{IrFunction, IrType};

struct AsmBackend;

impl CodegenBackend for AsmBackend {
    fn emit_function(&self, func: &IrFunction) -> CodegenResult<String> {
        // ...
    }
    fn emit_type(&self, ty: &IrType) -> String {
        // ...
    }
}
```

## Dependencies

- [`rustdec-ir`](../rustdec-ir)
- [`rustdec-lift`](../rustdec-lift) — `frame::is_slot_id` (slot pointer detection)
- [`rustdec-analysis`](../rustdec-analysis) — `structure_function`
