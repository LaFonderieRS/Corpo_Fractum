# Intermediate Representation

The IR is the pivot of the entire toolchain. It sits between the lifter (which produces it) and the code generators (which consume it). It is defined entirely in `rustdec-ir`.

---

## Goals

- **Architecture-independent** — no registers after lifting, no x86-specific opcodes.
- **Target-independent** — no C/Rust syntax baked in; backends decide how to render.
- **Explicit types on every value** — every SSA variable and constant carries an `IrType`.
- **Confidence-annotated** — basic blocks carry a confidence score for uncertain deductions.

---

## Type system

```rust
pub enum IrType {
    UInt(u8),                        // unsigned integer, bit-width 8/16/32/64
    SInt(u8),                        // signed integer
    Float(u8),                       // 32 or 64-bit IEEE float
    Ptr(Box<IrType>),                // typed pointer (64-bit target assumed)
    Array { elem: Box<IrType>, len: u64 },
    Struct { name: String, size: u64 }, // opaque struct (name from DWARF or synthetic)
    Void,
    Unknown,
}

pub type IrTypeRef = Arc<IrType>;   // shared, reference-counted type node
```

`IrTypeRef` is used everywhere a type appears. The same `Arc` node is reused across all variables of the same type in a function, avoiding repeated allocations.

Common types are constructed with helpers:
```rust
IrType::u64()   // UInt(64)
IrType::u32()   // UInt(32)
IrType::u8()    // UInt(8)
IrType::ptr()   // Ptr(UInt(8))  — void* equivalent
```

---

## Values

```rust
pub enum Value {
    Var { id: u32, ty: IrTypeRef },       // SSA variable
    Const { val: u64, ty: IrTypeRef },    // integer / pointer constant
}
```

Variables are identified by a `u32` id. Every variable is assigned exactly once (SSA invariant). The `IrFunction` assigns fresh ids via `fresh_var()`.

---

## Expressions

```rust
pub enum Expr {
    Value(Value),                          // copy / phi source
    BinOp { op: BinOp, lhs: Value, rhs: Value },
    Load { ptr: Value, ty: IrTypeRef },    // memory dereference
    Call { target: CallTarget, args: Vec<Value>, ret_ty: IrTypeRef },
    Cast { val: Value, to: IrTypeRef },    // type cast / truncation / extension
    Symbol { addr: u64, kind: SymbolKind, name: Arc<str> },
    ArrayAccess { name: String, index: Value, elem_ty: IrTypeRef },
    Opaque(String),                        // unresolved / unlifted expression
}
```

`Arc<str>` is used for `Symbol::name` and `CallTarget::Named` so that the same string literal is shared across all uses in the module.

### Binary operators

| Variant | Meaning |
|---|---|
| `Add`, `Sub`, `Mul` | arithmetic |
| `UDiv`, `SDiv` | unsigned / signed division |
| `URem`, `SRem` | unsigned / signed remainder |
| `And`, `Or`, `Xor` | bitwise |
| `Shl`, `LShr`, `AShr` | shifts |
| `Eq`, `Ne` | equality comparison |
| `Ult`, `Ule` | unsigned less-than / less-or-equal |
| `Slt`, `Sle` | signed less-than / less-or-equal |

### Call targets

```rust
pub enum CallTarget {
    Direct(u64),        // statically known address
    Indirect(Value),    // computed target (function pointer)
    Named(Arc<str>),    // imported symbol (e.g. "printf")
}
```

### Symbol kinds

```rust
pub enum SymbolKind {
    String,     // points to a string literal in .rodata
    Function,   // call target resolved to a known function
    Global,     // global variable reference
}
```

---

## Statements

```rust
pub enum Stmt {
    Assign { lhs: u32, ty: IrTypeRef, rhs: Expr }, // SSA assignment  %lhs = rhs
    Store { ptr: Value, val: Value },               // *ptr = val
    ArrayStore { name: String, index: Value, val: Value }, // arr[i] = val
    Nop,                                            // placeholder (dead code)
}
```

Dead-code elimination replaces unused assignments with `Nop` rather than removing them, preserving statement indices for later passes.

---

## Terminators

Every basic block ends with exactly one terminator:

```rust
pub enum Terminator {
    Jump(BlockId),                                       // unconditional
    Branch { cond: Value, true_bb: BlockId,
             false_bb: BlockId, mnemonic: String },      // conditional
    Return(Option<Value>),                               // function return
    Unreachable,                                         // after ud2 / hlt
}
```

The `mnemonic` field on `Branch` preserves the original x86 condition code (e.g. `"jne"`, `"jl"`) so code generators can emit idiomatic comparisons.

---

## Basic blocks

```rust
pub struct BasicBlock {
    pub id: BlockId,                 // u32
    pub start_addr: u64,
    pub end_addr: u64,
    pub stmts: Vec<Stmt>,
    pub terminator: Terminator,
    pub confidence: f32,             // 0.0 – 1.0; < 1.0 means uncertain lift
}
```

---

## Stack frame

Frame analysis populates `IrFunction::slot_table` with a `StackSlot` for every addressed stack location:

```rust
pub struct StackSlot {
    pub rbp_offset: i64,        // signed offset from RBP
    pub ty: IrTypeRef,
    pub name: String,           // e.g. "local_0", "arg_1", "saved_rbx"
    pub origin: SlotOrigin,
    pub array_info: Option<ArrayInfo>,
    pub provenance: Provenance,
}

pub enum SlotOrigin {
    Local,       // local variable
    StackArg,    // argument passed on the stack (beyond the register window)
    SavedReg,    // callee-saved register spill
    Unknown,
}

pub struct ArrayInfo {
    pub count: u32,   // number of elements
    pub stride: u32,  // element size in bytes
}
```

`Provenance` records how the type was determined:

| Variant | Source |
|---|---|
| `Auto` | synthetic default |
| `Inferred` | derived from usage patterns |
| `Dwarf` | read from DWARF debug info |
| `User` | set interactively (future) |

---

## Functions and modules

```rust
pub struct IrFunction {
    pub name: String,
    pub entry_addr: u64,
    pub end_addr: u64,
    pub cfg: CfgGraph,              // petgraph DiGraph<BasicBlock, CfgEdge>
    pub params: Vec<IrTypeRef>,
    pub param_names: Vec<String>,
    pub ret_ty: IrTypeRef,
    pub next_var_id: u32,
    pub slot_table: Vec<StackSlot>,
    pub frame_size: u64,
    pub reg_names: HashMap<u32, String>, // var_id → register name (debug)
}

pub struct IrModule {
    pub functions: Vec<IrFunction>,
    pub string_table: HashMap<u64, String>, // address → string content
}
```

`IrFunction::blocks_sorted()` returns basic blocks in start-address order, which is the canonical iteration order used by code generators.

---

## Structured IR

After CFG structuration (in `rustdec-analysis`), an `IrFunction` can be converted to a tree of `SNode`:

```rust
pub enum SNode {
    Block(BlockId),
    Seq(Vec<SNode>),
    IfElse { cond: CondExpr, then: Box<SNode>, else_: Option<Box<SNode>> },
    Loop { cond: Option<CondExpr>, body: Box<SNode> },
    Break,
    Continue,
}
```

Code generators consume `SNode` trees to emit structured output (`if`, `while`, `for`) without gotos.
