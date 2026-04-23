# rustdec-ir

The Intermediate Representation (IR) used by RustDec.

All architecture-specific knowledge lives in `rustdec-lift`; everything
from the lifter onward — analysis, structuration, code generation — operates
exclusively on these types.  The IR is in **SSA form**: every variable is
assigned exactly once, identified by a function-scoped integer id.

## Design goals

- **Architecture-independent** — no register names, no x86 opcodes after lifting.
- **Target-independent** — no C, Rust, or C++ syntax.
- **Explicit types** — every `Value` carries an `IrType`.
- **Serialisable** — all public types derive `Serialize`/`Deserialize` (the CFG graph is skipped for portability).

## Type hierarchy

```
IrModule
 └─ Vec<IrFunction>
       ├─ CfgGraph  (petgraph DiGraph<BasicBlock, CfgEdge>)
       │    └─ BasicBlock
       │         ├─ Vec<Stmt>
       │         │    ├─ Assign { lhs: u32, ty, rhs: Expr }
       │         │    ├─ Store  { ptr: Value, val: Value }
       │         │    └─ Nop
       │         └─ Terminator
       │              ├─ Jump(BlockId)
       │              ├─ Branch { cond, true_bb, false_bb, mnemonic }
       │              ├─ Return(Option<Value>)
       │              └─ Unreachable
       ├─ HashMap<i64, StackSlot>   — named frame slots
       └─ Vec<IrType>               — parameter types
```

## Key types

### `IrType`

```rust
IrType::UInt(64)          // uint64_t
IrType::SInt(32)          // int
IrType::Float(64)         // double
IrType::Ptr(Box<IrType>)  // pointer
IrType::Array { elem, len }
IrType::Struct { name, size }
IrType::Void
IrType::Unknown           // not yet inferred
```

`IrType::byte_size()` returns the static size in bytes (`None` for `Void`/`Unknown`).

### `Value`

```rust
Value::Var   { id: u32, ty: IrType }   // SSA variable  → "v42"
Value::Const { val: u64, ty: IrType }  // integer const → "0x1234"
```

### `Expr` (right-hand side of `Assign`)

| Variant | Meaning |
|---|---|
| `Value(v)` | Copy / φ-resolution |
| `BinOp { op, lhs, rhs }` | Arithmetic or comparison |
| `Load { ptr, ty }` | Memory read (`*ptr`) |
| `Call { target, args, ret_ty }` | Function call |
| `Cast { val, to }` | Type cast / bit-cast |
| `Symbol { addr, kind, name }` | Resolved string, function, or global |
| `Opaque(String)` | Unlifted / unknown expression |

### `StackSlot`

A named stack frame slot discovered during lifting.

```rust
StackSlot {
    rbp_offset: i64,      // negative = local, positive = stack arg
    ty:         IrType,
    name:       String,   // "local_0", "arg_1", "saved_rbp"
    origin:     SlotOrigin,
}
```

Slot naming convention (x86-64 SysV ABI):

| rbp offset | Name | Origin |
|---|---|---|
| `0` | `saved_rbp` | `SavedReg` |
| `-8`, `-16`, … | `local_0`, `local_1`, … | `Local` |
| `+16`, `+24`, … | `arg_0`, `arg_1`, … | `StackArg` |

## `IrFunction` helpers

```rust
let mut func = IrFunction::new("main", 0x401000);

let id = func.fresh_var();                         // allocate next SSA id
let slot = func.get_or_insert_slot(-8, IrType::u64()); // local_0
let blocks = func.blocks_sorted();                 // sorted by start_addr
```

## Slot pointer encoding

The lifter encodes stack slot references as `Value::Var` with IDs in the
range `10_000 – 20_000` (i.e. `10_000 + rbp_offset + 4096`).  Consumers can
test for this range with `rustdec_lift::frame::is_slot_id(id)` and convert
back with `slot_id_to_offset(id)`.

## Dependencies

- [`petgraph`](https://crates.io/crates/petgraph) — directed graph for the CFG
- [`serde`](https://crates.io/crates/serde) — serialisation
