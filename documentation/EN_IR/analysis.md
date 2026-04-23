# Analysis

`rustdec-analysis` orchestrates the full pipeline and provides several static analysis passes that operate on top of the raw IR.

---

## Top-level entry point

```rust
pub fn analyse(obj: &BinaryObject) -> AnalysisResult<IrModule>
```

Internal execution order:

```
rayon::join(
  disassemble all sections,
  extract_strings(obj)
)
build_symbol_map(obj, &strings)
detect_functions(obj, &instructions)
compute_function_boundaries()
rayon::par_iter: for each function {
  build_cfg(name, entry, end, insns)
  lift_function(func, insns, symbols)
}
build_call_graph(&module)
return IrModule
```

---

## Function detection

`detect_functions` performs a 4-stage scan and returns a `BTreeMap<u64, String>` (address → name):

**Stage 1 — entry point.**  
The binary's `e_entry` field is always registered as `_start`.

**Stage 2 — symbol table.**  
All symbols with `SymbolKind::Function` are added. Import stubs from the PLT are included.

**Stage 3 — call-site scanning.**  
Every `call rel32` instruction adds its target to the map if not already present. Synthetic names are generated: `sub_<addr>`.

**Stage 4 — indirect jump tables.**  
PLT-style thunk sequences and switch-table jump patterns are detected and their targets registered.

---

## CFG construction

`build_cfg` uses a 3-pass algorithm to produce a correct CFG for a single function.

**Pass 1 — leader identification.**  
A block leader is any instruction that is:
- the function entry point,
- a direct branch target,
- the instruction immediately following a terminator.

**Pass 2 — block creation.**  
Instructions are partitioned into basic blocks at leader boundaries. Each block records its start/end addresses.

**Pass 3 — edge connection.**  
For each terminator:
- `ret` → no outgoing edge (block is a sink).
- `jmp target` → one edge to the target block.
- `jcc target` → two edges: fall-through (false) and target (true).
- `call` → treated as a non-terminator; execution continues to the next instruction.
- Indirect `jmp` (e.g. switch) → edges to all known targets found by jump-table scanning.

The result is an `IrFunction` with a populated `CfgGraph` but empty statement lists.

---

## Dominator analysis

`dominance::compute(func)` runs the Cooper-Harvey-Kennedy dominator algorithm on the CFG and returns a `DomTree`.

```rust
pub struct DomTree { … }

impl DomTree {
    pub fn idom(&self, node: BlockId) -> Option<BlockId>
    pub fn strictly_dominates(&self, a: BlockId, b: BlockId) -> bool
}
```

`find_natural_loops(func)` builds on the dominator tree: any back-edge `(n → h)` where `h` dominates `n` defines a natural loop with header `h`. Returns `Vec<NaturalLoop>`.

`find_convergence(block, cond_block)` finds the post-dominator of a branch, used during structuration to determine where an if/else rejoins.

---

## CFG structuration

`structure_function(func)` converts the flat CFG into a structured `SNode` tree suitable for code generation without gotos.

**Algorithm:**

1. DFS to identify back-edges (loop headers).
2. Filter back-edges to get a DAG.
3. Topological order on the DAG.
4. For each node in order:
   - If it is a loop header → emit `Loop { cond, body }`.
   - If it ends with a conditional branch → find convergence point, emit `IfElse`.
   - Otherwise → emit `Block`.
5. Sequences of nodes with no branching → collapsed into `Seq`.

The `mnemonic` stored on `Branch` terminators is used to construct a `CondExpr` that preserves the original condition semantics (e.g. `jl` → signed less-than).

---

## String recovery

`StringRecovery` performs multi-stage string extraction from a binary.

```rust
pub struct StringRecovery<'a> {
    obj: &'a BinaryObject,
    string_table: &'a StringTable,
}
```

**Stages:**

1. **`.rodata` scan** — `apply_rodata_strings()` finds null-terminated ASCII sequences of length ≥ 4 in read-only data sections.
2. **Exhaustive scan** — `recover_strings_from_binary()` scans all data sections; tries ASCII, UTF-8, UTF-16 LE/BE, UTF-32 LE/BE.
3. **CFG-aware recovery** — `recover_strings_with_cfg()` cross-references discovered strings with the instructions that load their addresses, improving confidence scores.

Each recovered string carries:

```rust
pub struct RecoveredString {
    pub content: String,
    pub address: u64,
    pub size: usize,
    pub encoding: StringEncoding,
    pub references: Vec<u64>,   // instruction addresses that load this string
    pub confidence: f32,        // 0.0 – 1.0
}
```

`StringEncoding` variants: `Ascii`, `Utf8`, `Utf16Le`, `Utf16Be`, `Utf32Le`, `Utf32Be`, `Binary`.

---

## Call graph

```rust
pub fn build_call_graph(module: &IrModule) -> CallGraph
```

Walks every `Stmt::Assign` whose `rhs` is an `Expr::Call`. For each call:
- Direct calls (`CallTarget::Direct(addr)`) → add an edge to the function at that address.
- Named calls (`CallTarget::Named(name)`) → add an edge to an external node for `name`.
- Indirect calls → recorded but not connected (target unknown statically).

`CallGraph` is a `petgraph::DiGraph<CgFunction, CgEdge>` where `CgEdge::sites` counts how many call sites connect the same caller/callee pair.

The GUI uses this graph for the interactive call-graph visualisation in `rustdec-gui`.
