//! # rustdec-lift
//!
//! Lifts x86-64 instructions into SSA IR statements, then runs stack frame
//! analysis to name local variables.

pub mod frame;
pub mod x86;

use petgraph::visit::NodeIndexable;
use rustdec_disasm::Instruction;
use rustdec_ir::{Expr, IrFunction, IrType, Stmt, Terminator, Value};
use std::collections::HashSet;
use rustdec_loader::StringTable;
use tracing::{debug, instrument, trace};

/// Lift all basic blocks of `func` in-place, then analyse the stack frame.
///
/// `string_table` maps virtual addresses to decoded string content — when the
/// lifter encounters a `Value::Const` pointing into that table it replaces it
/// with `Expr::StringRef` so the codegen can emit a string literal.
#[instrument(skip_all, fields(func = %func.name))]
pub fn lift_function(
    func:         &mut IrFunction,
    insns:        &[Instruction],
    string_table: &StringTable,
) {
    debug!(func = %func.name, blocks = func.cfg.node_count(), "lifting function");

    let node_count = func.cfg.node_count();
    for ni in 0..node_count {
        let idx = func.cfg.from_index(ni);
        let (start_addr, end_addr) = {
            let bb = &func.cfg[idx];
            (bb.start_addr, bb.end_addr)
        };

        let block_insns: Vec<&Instruction> = insns
            .iter()
            .skip_while(|i| i.address < start_addr)
            .take_while(|i| i.address < end_addr)
            .collect();

        trace!(func  = %func.name,
               block = format_args!("{:#x}", start_addr),
               insns = block_insns.len(),
               "lifting block");

        let (stmts, rax_id, reg_names) =
            x86::lift_block_with_regs(&block_insns, &mut func.next_var_id);

        // Merge seed names into the function table — only insert ids not
        // already present so later blocks don't overwrite earlier bindings.
        for (id, name) in reg_names {
            func.reg_names.entry(id).or_insert(name);
        }

        // Patch Return terminator with rax value.
        if matches!(func.cfg[idx].terminator, Terminator::Return(_)) {
            func.cfg[idx].terminator = if let Some(id) = rax_id {
                trace!(func    = %func.name,
                       block   = format_args!("{:#x}", start_addr),
                       rax_id  = id,
                       "patching Return with rax value");
                Terminator::Return(Some(Value::Var { id, ty: IrType::UInt(64) }))
            } else {
                Terminator::Return(None)
            };
        }

        func.cfg[idx].stmts = stmts;
    }

    infer_return_type(func);

    // Dead code elimination — remove Assign stmts whose lhs is never read.
    // This eliminates flag-computation noise (e.g. `v1 = (v0 == 0x0)` when
    // the flag is not consumed by any branch or expression).
    eliminate_dead_assigns(func);

    // Annotate constant addresses that point to known strings.
    annotate_string_refs(func, string_table);

    // Frame analysis — runs last so it sees fully annotated stmts.
    frame::analyse_frame(func);

    debug!(func    = %func.name,
           ret     = ?func.ret_ty,
           slots   = func.slot_table.len(),
           frame   = func.frame_size,
           "lift complete");
}

// ── String annotation pass ────────────────────────────────────────────────────

/// Walk every stmt in `func` and replace constant addresses with `StringRef`.
///
/// Two-pass strategy per block:
///
/// 1. Build a local copy-table mapping `SSA id → address` for every
///    `Assign { rhs: Value(Const(addr)) }` in the block.  This lets us
///    resolve `mov rdi, 0x401180` even when the constant has already been
///    folded into a Var by the lifter.
///
/// 2. Rewrite:
///    - `Assign { rhs: Expr::Value(Const(addr)) }` → `StringRef`  (direct)
///    - `Assign { rhs: Expr::Call { args } }` where an arg is `Var(id)` and
///      `copy_table[id]` is a known string address → replace the arg with a
///      synthetic `Const(addr)` so the codegen lookup works, and also inject
///      a `StringRef` wrapper around the call expression where possible.
///
/// This handles the common pattern:
/// ```asm
/// mov  rdi, 0x401180   ; lifter: vN = 0x401180
/// call puts            ; lifter: call puts(vN, …)
/// ```
/// The copy-table lets us see that `vN == 0x401180` at the call site.
fn annotate_string_refs(func: &mut IrFunction, strings: &StringTable) {
    use petgraph::visit::NodeIndexable;
    use rustdec_ir::Expr;

    if strings.is_empty() { return; }

    let node_count = func.cfg.node_count();
    for ni in 0..node_count {
        let idx = func.cfg.from_index(ni);

        // Pass 1 — build block-local const map: id → address.
        let mut const_map: std::collections::HashMap<u32, u64> =
            std::collections::HashMap::new();
        for stmt in &func.cfg[idx].stmts {
            if let Stmt::Assign { lhs, rhs: Expr::Value(Value::Const { val, .. }), .. } = stmt {
                const_map.insert(*lhs, *val);
            }
        }

        // Pass 2 — rewrite expressions.
        for stmt in &mut func.cfg[idx].stmts {
            match stmt {
                Stmt::Assign { rhs, .. } => annotate_expr(rhs, strings, &const_map),
                _ => {}
            }
        }
    }
}

fn annotate_expr(
    expr:      &mut rustdec_ir::Expr,
    strings:   &StringTable,
    const_map: &std::collections::HashMap<u32, u64>,
) {
    use rustdec_ir::Expr;

    match expr {
        // Direct: `mov reg, <string_addr>` → StringRef.
        Expr::Value(Value::Const { val, .. }) => {
            if let Some(content) = strings.get(val) {
                trace!(addr = format_args!("{:#x}", val),
                       content = %content, "string ref annotated (direct)");
                *expr = Expr::StringRef { addr: *val, content: content.clone() };
            }
        }

        // Call: resolve Var args through the block-local copy table.
        // If an arg Var(id) was assigned from a string address, replace it
        // with a Const(addr) so the codegen StringRef lookup fires.
        Expr::Call { args, .. } => {
            for arg in args.iter_mut() {
                let resolved_addr = match arg {
                    Value::Const { val, .. } => Some(*val),
                    Value::Var   { id, .. }  => const_map.get(id).copied(),
                };
                if let Some(addr) = resolved_addr {
                    if strings.contains_key(&addr) {
                        // Replace with a Const pointing to the string so that
                        // emit_expr_resolved will see it as a StringRef when
                        // it evaluates Value::Const against string_table.
                        *arg = Value::Const { val: addr, ty: rustdec_ir::IrType::Ptr(
                            Box::new(rustdec_ir::IrType::UInt(8))
                        )};
                        trace!(addr = format_args!("{:#x}", addr),
                               "call arg resolved to string addr");
                    }
                }
            }
        }

        _ => {}
    }
}

// ── Return type inference ─────────────────────────────────────────────────────

fn infer_return_type(func: &mut IrFunction) {
    let has_value = func.blocks_sorted().iter().any(|bb| {
        matches!(bb.terminator, Terminator::Return(Some(_)))
    });
    func.ret_ty = if has_value { IrType::UInt(64) } else { IrType::Void };
}

// ── Dead code elimination ─────────────────────────────────────────────────────

/// Remove `Assign` statements whose left-hand side is never read anywhere in
/// the function.
///
/// This is a simple liveness pass:
/// 1. Collect every SSA id that appears as an *operand* (right-hand side) in
///    any statement, call argument, store value, or terminator.
/// 2. Any `Assign { lhs }` where `lhs ∉ live` is dead and is dropped.
///
/// We run this repeatedly until no more stmts are removed, so that chains of
/// dead assignments (flag → flag2 → …) are fully eliminated in one call.
///
/// **Preserved**: `Store` and `Nop` are always kept — stores have side effects.
fn eliminate_dead_assigns(func: &mut IrFunction) {
    use petgraph::visit::NodeIndexable;

    loop {
        // ── Step 1: collect all ids that are READ somewhere ─────────────────
        let mut live: HashSet<u32> = HashSet::new();

        let node_count = func.cfg.node_count();
        for ni in 0..node_count {
            let idx = func.cfg.from_index(ni);
            for stmt in &func.cfg[idx].stmts {
                collect_reads_stmt(stmt, &mut live);
            }
            collect_reads_terminator(&func.cfg[idx].terminator, &mut live);
        }

        // Always keep the final rax id (it may be the return value).
        if let Some(Value::Var { id, .. }) = return_value(func) {
            live.insert(id);
        }

        // ── Step 2: drop dead Assign stmts ──────────────────────────────────
        let mut removed = 0usize;
        for ni in 0..node_count {
            let idx = func.cfg.from_index(ni);
            let before = func.cfg[idx].stmts.len();
            func.cfg[idx].stmts.retain(|stmt| match stmt {
                Stmt::Assign { lhs, .. } => {
                    // Keep if lhs is live OR if it's a seeded ABI register
                    // (ids < next_var_id at the time of seeding — we keep all
                    // Assigns unconditionally for the seed ids since they are
                    // not emitted as stmts anyway; the retain only sees stmts
                    // actually generated by the lifter).
                    live.contains(lhs)
                }
                // Stores and Nops always kept.
                _ => true,
            });
            removed += before - func.cfg[idx].stmts.len();
        }

        if removed == 0 {
            break; // Fixed point reached.
        }
    }
}

/// Collect all SSA ids *read* by a single statement.
fn collect_reads_stmt(stmt: &Stmt, live: &mut HashSet<u32>) {
    match stmt {
        Stmt::Assign { rhs, .. } => collect_reads_expr(rhs, live),
        Stmt::Store  { ptr, val } => {
            collect_reads_value(ptr, live);
            collect_reads_value(val, live);
        }
        Stmt::Nop => {}
    }
}

fn collect_reads_expr(expr: &Expr, live: &mut HashSet<u32>) {
    match expr {
        Expr::Value(v)                    => collect_reads_value(v, live),
        Expr::BinOp { lhs, rhs, .. }      => {
            collect_reads_value(lhs, live);
            collect_reads_value(rhs, live);
        }
        Expr::Load { ptr, .. }             => collect_reads_value(ptr, live),
        Expr::Cast { val, .. }             => collect_reads_value(val, live),
        Expr::Call { args, .. }            => {
            for a in args { collect_reads_value(a, live); }
        }
        Expr::StringRef { .. }             => {}
        Expr::Opaque(_)                    => {}
    }
}

fn collect_reads_value(v: &Value, live: &mut HashSet<u32>) {
    if let Value::Var { id, .. } = v {
        live.insert(*id);
    }
}

fn collect_reads_terminator(term: &Terminator, live: &mut HashSet<u32>) {
    match term {
        Terminator::Return(Some(v))        => collect_reads_value(v, live),
        Terminator::Branch { cond, .. }    => collect_reads_value(cond, live),
        _ => {}
    }
}

/// Return the return value of the function if it is a `Var`, else `None`.
fn return_value(func: &IrFunction) -> Option<Value> {
    use petgraph::visit::NodeIndexable;
    let n = func.cfg.node_count();
    for ni in 0..n {
        let idx = func.cfg.from_index(ni);
        if let Terminator::Return(Some(v)) = &func.cfg[idx].terminator {
            return Some(v.clone());
        }
    }
    None
}
