# rustdec-analysis

Static analysis pipeline: function detection, CFG construction, IR lifting,
dominance analysis, loop detection, CFG structuration, and call graph building.

The top-level entry point is `analyse`, which takes a loaded `BinaryObject`
and returns a fully populated `IrModule` ready for code generation.

## Pipeline position

```
BinaryObject ──► rustdec-analysis::analyse ──► IrModule
                         │
                  (internally uses)
                  rustdec-disasm
                  rustdec-lift
```

## Top-level API

```rust
use rustdec_analysis::analyse;

let obj    = rustdec_loader::load_file("binary")?;
let module = analyse(&obj)?;

for func in &module.functions {
    println!("{} — {} blocks", func.name, func.cfg.node_count());
}
```

`analyse` runs the full pipeline in parallel using Rayon: disassembly and
string extraction run concurrently, then CFG building + lifting is
parallelised one task per detected function.

## Individual passes

Each pass is also exported individually for testing or custom pipelines.

### `detect_functions`

```rust
let funcs: BTreeMap<u64, String> = detect_functions(&obj, &instructions);
```

Discovers function entry points from four sources, in order:

1. Binary entry point (`e_entry`, `AddressOfEntryPoint`)
2. Symbol table (ELF, PE, Mach-O function symbols)
3. **Call-site scanning** — every `call <direct_target>` adds a new candidate
4. Indirect jump table heuristics (PLT stubs, switch tables)

### `build_cfg`

```rust
let func: IrFunction = build_cfg(name, entry_addr, end_addr, &insns, &addr_to_idx);
```

Splits a function's instructions into basic blocks (3-pass algorithm):

| Pass | Action |
|---|---|
| 1 | Identify block leaders (entry, branch targets, instruction after a terminator) |
| 2 | Build `BasicBlock` nodes and record pending edges |
| 3 | Resolve placeholder `BlockId`s and add CFG edges |

Returns an `IrFunction` with a populated CFG but **empty `stmts`** —
call `rustdec_lift::lift_function` next.

### `structure_function`

```rust
let sfunc: StructuredFunc = structure_function(&func);
```

Converts a flat CFG into a structured AST (`SNode`) for readable output:

| `SNode` | Emitted as |
|---|---|
| `Block(id)` | Flat statement sequence |
| `Seq(nodes)` | Consecutive nodes |
| `IfElse { cond, then, else_ }` | `if / else` |
| `Loop { cond, body }` | `while` / `do-while` |

Uses DFS back-edge detection to identify loops and a topological walk to
build the tree.

### `build_call_graph`

```rust
let cg: CallGraph = build_call_graph(&module);
```

Builds a directed graph (`petgraph::DiGraph`) where nodes are function names
and edges are call sites.  Useful for computing call order, finding recursive
cycles, or driving the GUI call-graph view.

### Dominance (`dominance` module)

```rust
use rustdec_analysis::{DomTree, NaturalLoop, find_natural_loops, find_convergence};

let dom   = DomTree::build(&func);
let loops = find_natural_loops(&func, &dom);
let conv  = find_convergence(&func, branch_block_idx);
```

`DomTree` wraps `petgraph`'s Cooper-Harvey-Kennedy dominator implementation
and exposes:
- `dominates(a, b)` — does A dominate B?
- `immediate_dominator(n)` — closest dominator
- `dominance_frontier(n)` — standard DF set

`find_natural_loops` returns all natural loops (header + body).
`find_convergence` finds the post-dominator of a conditional branch, used by
the structurer to identify where an `if` block rejoins the main flow.

## Error handling

```rust
pub enum AnalysisError {
    Disasm(DisasmError),     // architecture not supported
    NoCodeSection,           // binary has no executable sections
}
```

## Dependencies

- [`rustdec-loader`](../rustdec-loader)
- [`rustdec-disasm`](../rustdec-disasm)
- [`rustdec-ir`](../rustdec-ir)
- [`rustdec-lift`](../rustdec-lift)
- [`petgraph`](https://crates.io/crates/petgraph) — CFG and call graph
- [`rayon`](https://crates.io/crates/rayon) — parallel function analysis
