# rustdec-lift

Translates x86-64 machine instructions into the SSA IR defined by
`rustdec-ir`, then cleans up the result with several analysis passes.

The main entry point is `lift_function`, which takes an `IrFunction` whose
CFG has already been built by `rustdec-analysis::build_cfg` and fills in
every `BasicBlock::stmts` in-place.

## Pipeline position

```
IrFunction (empty stmts)  ──►  rustdec-lift  ──►  IrFunction (full IR)
         ▲                                               │
  rustdec-analysis                                       ▼
  build_cfg()                               rustdec-codegen / rustdec-analysis
```

## What `lift_function` does

The function runs the following passes in order:

| Step | Pass | Effect |
|---|---|---|
| 1 | **x86 lifting** | Each instruction → one or more SSA `Stmt`s |
| 2 | **Return type inference** | Sets `func.ret_ty` from `rax` usage |
| 3 | **DCE round 1** | Removes `Assign` stmts whose LHS is never read |
| 4 | **Constant resolution** | Replaces known-address constants with `Expr::Symbol` nodes |
| 5 | **Frame analysis** | Names stack slots, strips prologue/epilogue noise |
| 6 | **Slot copy-propagation** | Eliminates `-O0` arg-spill patterns (`local_0 = rdi; v5 = local_0` → `v5 = rdi`) |
| 7 | **DCE round 2** | Removes stmts made dead by copy-prop |
| 8 | **Canary elimination** | Removes `-fstack-protector` epilogue (`__stack_chk_fail` block and its comparison chain) |
| 9 | **DCE round 3** | Removes stmts made dead by canary removal |
| 10 | **ABI arity inference** | Fills `func.params` from x86-64 SysV calling convention |

## Frame analysis (`frame` module)

Recognises the standard x86-64 frame prologue:
```asm
push  rbp
mov   rbp, rsp
sub   rsp, N       ; sets func.frame_size = N
```

and the symmetric epilogue (`leave` / `mov rsp, rbp; pop rbp`).
Both are **nop-ed out** of the IR so the code generator sees only the
user-visible logic.

Stack slots are discovered from `rbp ± K` and `rsp ± K` memory accesses,
named (`local_0`, `arg_0`, …), and inserted into `func.slot_table`.

## Slot pointer encoding

Stack slot pointers are represented in the IR as `Value::Var` with IDs in
the range `[10_000, 20_000)`:

```
slot_id = 10_000 + (rbp_offset + 4_096)
```

Use the helpers exported from `rustdec_lift::frame`:

```rust
use rustdec_lift::frame::{is_slot_id, slot_id_to_offset};

if is_slot_id(var_id) {
    let offset: i64 = slot_id_to_offset(var_id); // signed rbp offset
}
```

## Register seeding

At the start of each block, the System V x86-64 ABI registers (`rdi`, `rsi`,
`rdx`, `rcx`, `r8`, `r9`, `rax`, `rsp`, `rbp`) are seeded with fresh SSA
ids.  This means:

- A `mov rax, rdi` in a leaf function produces `v1 = v0` where `v0` is the
  seed id for `rdi` — recognisable as the first argument.
- `func.reg_names` maps seed ids to register names so the code generator can
  emit `rdi` instead of `v0` for unwritten input registers.

## Usage

```rust
use rustdec_lift::lift_function;
use rustdec_loader::build_symbol_map;

// Assume `func` has been built by `rustdec_analysis::build_cfg`.
let symbol_map = build_symbol_map(&binary_object, &string_table);
lift_function(&mut func, &all_instructions, &symbol_map);

// func.stmts, func.ret_ty, func.params, func.slot_table are now populated.
```

## Limitations

- Only x86-64 is supported; other architectures produce empty blocks.
- Flag semantics are approximated: the `cond` value in `Branch` records the
  last `cmp`/`test` result, not a precise boolean over the flag register.
- Floating-point instructions produce `Opaque` stmts (not yet lifted).
- SIMD / SSE / AVX instructions produce `Opaque` stmts.

## Dependencies

- [`rustdec-ir`](../rustdec-ir) — IR types
- [`rustdec-disasm`](../rustdec-disasm) — `Instruction` type
- [`rustdec-loader`](../rustdec-loader) — `SymbolMap`
- [`petgraph`](https://crates.io/crates/petgraph) — CFG traversal
