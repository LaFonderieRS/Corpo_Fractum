//! # rustdec-lift
//!
//! Lifts x86-64 instructions into SSA IR statements, then runs stack frame
//! analysis to name local variables.

pub mod frame;
pub mod x86;

use petgraph::visit::NodeIndexable;
use rustdec_disasm::Instruction;
use rustdec_ir::{IrFunction, IrType, Stmt, Terminator, Value};
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

        let (stmts, rax_id) =
            x86::lift_block_with_regs(&block_insns, &mut func.next_var_id);

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

/// Walk every stmt in `func` and replace `Value::Const { val: addr }` with
/// `Expr::StringRef` when `addr` is a known string address.
///
/// We look specifically at:
/// - `Stmt::Assign { rhs: Expr::Value(Const(addr)) }` — e.g. `mov rdi, 0x401180`
/// - `Stmt::Assign { rhs: Expr::Call { args } }` — const args to calls
fn annotate_string_refs(func: &mut IrFunction, strings: &StringTable) {
    use petgraph::visit::NodeIndexable;
    use rustdec_ir::Expr;

    if strings.is_empty() { return; }

    let node_count = func.cfg.node_count();
    for ni in 0..node_count {
        let idx = func.cfg.from_index(ni);
        for stmt in &mut func.cfg[idx].stmts {
            match stmt {
                Stmt::Assign { rhs, .. } => {
                    annotate_expr(rhs, strings);
                }
                _ => {}
            }
        }
    }
}

fn annotate_expr(expr: &mut rustdec_ir::Expr, strings: &StringTable) {
    use rustdec_ir::Expr;

    match expr {
        // mov reg, <addr>  →  StringRef if addr is a known string.
        Expr::Value(Value::Const { val, .. }) => {
            if let Some(content) = strings.get(val) {
                trace!(addr = format_args!("{:#x}", val), content = %content, "string ref annotated");
                *expr = Expr::StringRef { addr: *val, content: content.clone() };
            }
        }

        // Call args — annotate const args that point to strings.
        Expr::Call { args, .. } => {
            for arg in args.iter_mut() {
                if let Value::Const { val, .. } = arg {
                    if let Some(content) = strings.get(val) {
                        // We can't replace a Value with an Expr here, but we
                        // can mark the arg with a special sentinel type so the
                        // codegen can look it up.  Instead, we store the info
                        // in the surrounding Assign's rhs — the codegen's
                        // emit_expr_resolved already handles StringRef at the
                        // top level; for call args we emit them inline below.
                        // For now the annotation is best-effort: the codegen
                        // will see Const(addr) and can look up the string table
                        // itself via the StringRef lookup path.
                        trace!(addr = format_args!("{:#x}", val),
                               content = %content,
                               "call arg points to string");
                    }
                }
            }
        }

        // LEA result — the addr is in the Opaque string, not a Const.
        // String resolution for LEA is done in the codegen via address lookup.
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
