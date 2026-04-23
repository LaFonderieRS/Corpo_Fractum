# Code generation

`rustdec-codegen` consumes an `IrModule` and emits pseudo-code in the requested language.

---

## Entry point

```rust
pub fn emit_module(module: &IrModule, lang: Language) -> CodegenResult<Vec<(String, String)>>
```

Returns one `(function_name, source_code)` pair per function. Functions in the CRT filter list are silently skipped (see below).

```rust
pub enum Language { C, Cpp, Rust }
```

---

## Common trait

All backends implement:

```rust
pub trait CodegenBackend {
    fn emit_function(&self, func: &IrFunction, string_table: &HashMap<u64, String>) -> String;
    fn emit_type(&self, ty: &IrType) -> String;
}
```

---

## C backend

The C backend targets C99 with standard types from `<stdint.h>`.

### Type mapping

| `IrType` | C type |
|---|---|
| `UInt(8)` | `uint8_t` |
| `UInt(16)` | `uint16_t` |
| `UInt(32)` | `uint32_t` |
| `UInt(64)` | `uint64_t` |
| `SInt(8)` | `int8_t` |
| `SInt(16)` | `int16_t` |
| `SInt(32)` | `int32_t` |
| `SInt(64)` | `int64_t` |
| `Float(32)` | `float` |
| `Float(64)` | `double` |
| `Ptr(T)` | `T*` |
| `Void` | `void` |
| `Unknown` | `uint64_t` |

### Emission passes

**Pass 1 — variable collection.**  
Walks all `Stmt::Assign` nodes. Builds a copy-table (direct variable-to-variable assignments that can be propagated inline). Tracks which variables are actually written.

**Pass 2 — declarations.**  
Emits one `type name;` line per live variable at the top of the function body.

**Pass 3 — body.**  
Walks the structured `SNode` tree (from CFG structuration) and emits:
- `Seq` → statements in order.
- `IfElse` → `if (cond) { … } else { … }`.
- `Loop` → `while (cond) { … }`.
- `Break` / `Continue` → bare keywords.
- `Block` → the block's statements.

**Pass 4 — pointer fixups.**  
Inserts casts where a pointer type is passed to a function expecting a different pointer type.

### libc signature override

`libc_signatures.rs` contains a lookup table for common C library functions. When a `CallTarget::Named` matches a known function (e.g. `printf`, `strlen`, `malloc`), the backend uses the known signature rather than the inferred one. This produces:

```c
// without override:    uint64_t v0 = printf(arg_0, arg_1);
// with override:       printf("%s\n", local_0);
```

---

## C++ backend

The C++ backend follows the same four-pass structure as C with these differences:

- Types use `uint64_t` / C++ idioms where applicable.
- `nullptr` instead of `0` for null pointers.
- Member access uses `->` vs `.` based on pointer type.

---

## Rust backend

The Rust backend emits `unsafe` Rust pseudo-code:

- Variables declared with `let mut`.
- Types use `u64`, `u32`, `i32`, `*mut u8`, etc.
- Memory loads rendered as `*ptr`.
- Function bodies wrapped in `unsafe { … }`.

The output is pseudo-code only — it will not compile as-is because SSA variable names and raw pointer operations require further refinement.

---

## CRT filter

The following function names are excluded from output regardless of language, as they are CRT boilerplate with no decompilation value:

`_start`, `_init`, `_fini`, `__libc_csu_init`, `__libc_csu_fini`, `__libc_start_main`, `frame_dummy`, `register_tm_clones`, `deregister_tm_clones`, `__do_global_dtors_aux`

---

## String table

The `IrModule::string_table` (address → content, extracted from `.rodata`) is passed to every emit call. When an `Expr::Symbol { kind: SymbolKind::String, addr, name }` is encountered, the backend emits the string content as a C string literal rather than a raw address:

```c
// raw:      printf((uint8_t*)0x402010, local_0);
// resolved: printf("Hello, %s!\n", local_0);
```
