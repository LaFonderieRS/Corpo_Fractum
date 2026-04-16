//! # rustdec-lift
//!
//! Lifts x86-64 instructions into SSA IR statements, then runs stack frame
//! analysis to name local variables.

pub mod frame;
pub mod x86;

use petgraph::visit::NodeIndexable;
use rustdec_disasm::Instruction;
use rustdec_ir::{BinOp, Expr, IrFunction, IrType, SymbolKind, Stmt, Terminator, Value};
use rustdec_loader::{SymbolMap, SymbolMapKind};
use std::collections::{HashMap, HashSet};
use tracing::{debug, instrument, trace};

/// Lift all basic blocks of `func` in-place, then analyse the stack frame.
///
/// `symbols` is the unified symbol map produced by
/// [`rustdec_loader::build_symbol_map`].  The lifter uses it to annotate any
/// SSA variable whose value chain resolves to a known symbol address as an
/// [`Expr::Symbol`] node, covering strings, functions, and global variables.
#[instrument(skip_all, fields(func = %func.name))]
pub fn lift_function(
    func:    &mut IrFunction,
    insns:   &[Instruction],
    symbols: &SymbolMap,
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
    eliminate_dead_assigns(func);

    // Resolve all constant addresses to symbolic IR nodes in a single pass.
    resolve_constants(func, symbols);

    // Frame analysis — runs last so it sees fully annotated stmts.
    frame::analyse_frame(func);

    // ABI arity inference — determines func.params from register usage.
    infer_abi_args(func);

    debug!(func    = %func.name,
           ret     = ?func.ret_ty,
           params  = func.params.len(),
           slots   = func.slot_table.len(),
           frame   = func.frame_size,
           "lift complete");
}

// ── Constant resolution pass ──────────────────────────────────────────────────

/// Single-entry-point pass that resolves every SSA chain reachable from a
/// known symbol address to an [`Expr::Symbol`] node.
///
/// This pass is **architecture-agnostic**: it operates on IR expressions, not
/// on machine-code operands.  It covers:
///
/// * Direct constant assignment — `vN = 0x401180` (MOV absolute)
/// * Arithmetic chains — `vN = vRIP + 0xfff` (RIP-relative LEA)
/// * Multi-block copy chains — `vM = vN` after any of the above
///
/// The algorithm:
/// 1. Build a function-wide **const-eval map** (`SSA id → u64`) by folding
///    every expression that can be fully reduced to a constant through a
///    fixed-point iteration.
/// 2. Walk all `Assign` statements once:
///    - If the RHS evaluates to a symbol address → replace with `Expr::Symbol`.
///    - If a call argument `Var(id)` evaluates to a string address → replace
///      with a typed `Const(addr)` so the codegen's fallback path emits the
///      string literal (since `Value` cannot carry an `Expr` directly).
fn resolve_constants(func: &mut IrFunction, symbols: &SymbolMap) {
    if symbols.is_empty() { return; }

    let const_eval = build_const_eval(func);
    let node_count = func.cfg.node_count();

    for ni in 0..node_count {
        let idx = func.cfg.from_index(ni);
        for stmt in &mut func.cfg[idx].stmts {
            if let Stmt::Assign { rhs, .. } = stmt {
                match rhs {
                    // ── Direct constant or copy-of-constant → Symbol ──────────
                    Expr::Value(Value::Const { val, .. }) => {
                        if let Some(entry) = symbols.get(val) {
                            trace!(addr  = format_args!("{:#x}", val),
                                   name  = %entry.name,
                                   "resolve_constants: Const → Symbol");
                            *rhs = symbol_expr(*val, entry);
                        }
                    }
                    Expr::Value(Value::Var { id, .. }) => {
                        if let Some(&addr) = const_eval.get(id) {
                            if let Some(entry) = symbols.get(&addr) {
                                trace!(id,
                                       addr  = format_args!("{:#x}", addr),
                                       name  = %entry.name,
                                       "resolve_constants: Var copy → Symbol");
                                *rhs = symbol_expr(addr, entry);
                            }
                        }
                    }
                    // ── Arithmetic / cast that folds to a symbol addr → Symbol ─
                    Expr::BinOp { .. } | Expr::Cast { .. } => {
                        if let Some(addr) = try_eval_expr(rhs, &const_eval) {
                            if let Some(entry) = symbols.get(&addr) {
                                trace!(addr  = format_args!("{:#x}", addr),
                                       name  = %entry.name,
                                       "resolve_constants: arithmetic → Symbol");
                                *rhs = symbol_expr(addr, entry);
                            }
                        }
                    }
                    // ── Call site: resolve Var args that trace to a symbol ─────
                    //
                    // `Value` cannot carry an `Expr::Symbol` so for string args
                    // we replace with `Const(addr)` — the codegen checks the
                    // string_table for every `Const` call arg.
                    Expr::Call { args, .. } => {
                        for arg in args.iter_mut() {
                            if let Value::Var { id, .. } = arg {
                                if let Some(&addr) = const_eval.get(id) {
                                    if let Some(entry) = symbols.get(&addr) {
                                        if entry.kind == SymbolMapKind::String {
                                            trace!(id,
                                                   addr = format_args!("{:#x}", addr),
                                                   "resolve_constants: call arg → Const(addr)");
                                            *arg = Value::Const {
                                                val: addr,
                                                ty:  IrType::Ptr(Box::new(IrType::UInt(8))),
                                            };
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Build an [`Expr::Symbol`] from a symbol map entry.
fn symbol_expr(addr: u64, entry: &rustdec_loader::SymbolEntry) -> Expr {
    let kind = match entry.kind {
        SymbolMapKind::String   => SymbolKind::String,
        SymbolMapKind::Function => SymbolKind::Function,
        SymbolMapKind::Global   => SymbolKind::Global,
    };
    Expr::Symbol { addr, kind, name: entry.name.clone() }
}

// ── Constant folding helpers ──────────────────────────────────────────────────

/// Build a map of `SSA id → u64` for every variable whose full expression
/// chain can be reduced to a constant.
///
/// A fixed-point loop (≤ 16 iterations) handles forward references: if `vA`
/// is defined before `vB` in block order but `vA = vB + 1`, the second pass
/// will resolve `vA` once `vB` is known.  In practice, straight-line SSA
/// converges in 1–2 iterations.
fn build_const_eval(func: &IrFunction) -> HashMap<u32, u64> {
    let mut eval: HashMap<u32, u64> = HashMap::new();
    let node_count = func.cfg.node_count();

    for _ in 0..16 {
        let mut changed = false;
        for ni in 0..node_count {
            let idx = func.cfg.from_index(ni);
            for stmt in &func.cfg[idx].stmts {
                if let Stmt::Assign { lhs, rhs, .. } = stmt {
                    if eval.contains_key(lhs) { continue; }
                    if let Some(v) = try_eval_expr(rhs, &eval) {
                        eval.insert(*lhs, v);
                        changed = true;
                    }
                }
            }
        }
        if !changed { break; }
    }
    eval
}

/// Try to reduce `expr` to a `u64` constant given the already-known map.
fn try_eval_expr(expr: &Expr, eval: &HashMap<u32, u64>) -> Option<u64> {
    match expr {
        Expr::Value(v)               => try_eval_value(v, eval),
        Expr::Cast  { val, .. }      => try_eval_value(val, eval),
        Expr::BinOp { op, lhs, rhs } => {
            let l = try_eval_value(lhs, eval)?;
            let r = try_eval_value(rhs, eval)?;
            Some(match op {
                BinOp::Add  => l.wrapping_add(r),
                BinOp::Sub  => l.wrapping_sub(r),
                BinOp::Mul  => l.wrapping_mul(r),
                BinOp::And  => l & r,
                BinOp::Or   => l | r,
                BinOp::Xor  => l ^ r,
                BinOp::Shl  => l.wrapping_shl(r as u32),
                BinOp::LShr => l.wrapping_shr(r as u32),
                BinOp::AShr => ((l as i64).wrapping_shr(r as u32)) as u64,
                _           => return None,
            })
        }
        _ => None,
    }
}

fn try_eval_value(v: &Value, eval: &HashMap<u32, u64>) -> Option<u64> {
    match v {
        Value::Const { val, .. } => Some(*val),
        Value::Var   { id,  .. } => eval.get(id).copied(),
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
                Stmt::Assign { lhs, .. } => live.contains(lhs),
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
        Expr::Symbol { .. }                => {}
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
    let n = func.cfg.node_count();
    for ni in 0..n {
        let idx = func.cfg.from_index(ni);
        if let Terminator::Return(Some(v)) = &func.cfg[idx].terminator {
            return Some(v.clone());
        }
    }
    None
}

// ── ABI arity inference ───────────────────────────────────────────────────────

/// Infer function arity by scanning which System V x86-64 argument registers
/// are *read* anywhere in the function body.
///
/// For each register in (rdi, rsi, rdx, rcx, r8, r9), check whether its seed
/// id appears as an operand.  The longest contiguous prefix of read registers
/// defines the parameter list — arity stops at the first unread register.
///
/// If `func.params` is already non-empty (e.g. populated from DWARF), the
/// pass is skipped.
///
/// Type inference heuristic (in priority order):
/// 1. `Cast(seed, to)` — movzx / sign-extension → use the target type
/// 2. `Load { ptr: seed }` — seed used as pointer → `Ptr(load_ty)`
/// 3. Default: `UInt(64)`
fn infer_abi_args(func: &mut IrFunction) {
    if !func.params.is_empty() { return; }

    const ARG_ORDER: &[&str] = &["rdi", "rsi", "rdx", "rcx", "r8", "r9"];

    // Build reg_name → seed_id map restricted to the arg registers.
    let mut reg_to_seed: HashMap<&str, u32> = HashMap::new();
    for (&id, name) in &func.reg_names {
        for &reg in ARG_ORDER {
            if name == reg { reg_to_seed.insert(reg, id); }
        }
    }

    // Collect all SSA ids that appear as read operands anywhere in the function.
    let mut live: HashSet<u32> = HashSet::new();
    let n = func.cfg.node_count();
    for ni in 0..n {
        let idx = func.cfg.from_index(ni);
        for stmt in &func.cfg[idx].stmts {
            collect_reads_stmt(stmt, &mut live);
        }
        collect_reads_terminator(&func.cfg[idx].terminator, &mut live);
    }

    // Determine the arity and infer types for each arg register in ABI order.
    let mut params: Vec<IrType> = Vec::new();
    for &reg in ARG_ORDER {
        let seed = match reg_to_seed.get(reg) {
            Some(&id) => id,
            None      => break,
        };
        if !live.contains(&seed) { break; } // first unread reg → stop
        params.push(infer_seed_type(func, seed));
    }

    if !params.is_empty() {
        debug!(func = %func.name, arity = params.len(), "ABI arity inferred");
        func.params = params;
    }
}

/// Heuristic type for an argument seed id, based on how it is used.
fn infer_seed_type(func: &IrFunction, seed: u32) -> IrType {
    let n = func.cfg.node_count();
    for ni in 0..n {
        let idx = func.cfg.from_index(ni);
        for stmt in &func.cfg[idx].stmts {
            match stmt {
                // movzx-style cast: seed is being narrowed → use the target type.
                Stmt::Assign {
                    rhs: Expr::Cast { val: Value::Var { id, .. }, to }, ..
                } if *id == seed => return to.clone(),

                // Load through seed: seed is a pointer → Ptr(pointee_ty).
                Stmt::Assign {
                    rhs: Expr::Load { ptr: Value::Var { id, .. }, ty }, ..
                } if *id == seed => return IrType::Ptr(Box::new(ty.clone())),

                _ => {}
            }
        }
    }
    IrType::UInt(64)
}
