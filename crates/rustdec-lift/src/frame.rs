//! Stack frame analysis pass.
//!
//! ## What this does
//!
//! After all blocks of a function have been lifted by `x86::lift_block_with_regs`,
//! this pass makes a second walk over the SSA statements to:
//!
//! 1. **Detect the frame prologue** — recognise the standard x86-64 pattern:
//!    ```asm
//!    push  rbp          ; saves caller's rbp
//!    mov   rbp, rsp     ; establishes frame pointer
//!    sub   rsp, N       ; allocates N bytes of locals
//!    ```
//!    From `N` we set `func.frame_size`.
//!
//! 2. **Discover stack slots** — scan every `Store`/`Load` whose pointer
//!    expression resolves to `rbp ± constant` or `rsp ± constant` (after
//!    adjusting for the known frame layout) and insert a `StackSlot` into
//!    `func.slot_table`.
//!
//! 3. **Rewrite pointer expressions** — replace `Expr::BinOp { Sub/Add, v_rbp, K }`
//!    patterns in `Stmt::Load`/`Stmt::Store` with a direct reference to the
//!    named slot variable, so the codegen can emit `local_0` instead of
//!    `*(v907 - 0x8)`.
//!
//! ## Limitations
//!
//! - Only handles rbp-relative and rsp-relative addressing.
//! - Does not attempt to resolve rsp-relative slots before the frame is
//!   established (pre-prologue area is left opaque).
//! - Type inference is heuristic: slot type defaults to `UInt(64)` unless the
//!   access size can be read from the `IrType` of the `Load`/`Store`.

use rustdec_ir::{BinOp, Expr, IrFunction, IrType, Stmt, Value};
use tracing::{debug, trace};

/// Run the stack frame analysis pass on `func` in-place.
pub fn analyse_frame(func: &mut IrFunction) {
    // ── Step 1: detect frame size from prologue ───────────────────────────────
    detect_frame_size(func);
    debug!(func = %func.name,
           frame_size = func.frame_size,
           "frame size detected");

    // ── Step 2: discover slots + Step 3: rewrite expressions ─────────────────
    // We need the rbp SSA id to recognise rbp-relative accesses.
    // The placeholder id for rbp (when not yet written) is 907.
    // The actual id will be whatever id was assigned to rbp in the prologue.
    let rbp_ids = collect_rbp_ids(func);
    let rsp_ids = collect_rsp_ids(func);

    debug!(func = %func.name,
           rbp_ids = rbp_ids.len(),
           rsp_ids = rsp_ids.len(),
           "frame register ids collected");

    rewrite_frame_accesses(func, &rbp_ids, &rsp_ids);

    debug!(func     = %func.name,
           slots    = func.slot_table.len(),
           "frame analysis complete");
}

// ── Step 1: prologue detection ────────────────────────────────────────────────

/// Scan the entry block for `sub rsp, N` and set `func.frame_size = N`.
fn detect_frame_size(func: &mut IrFunction) {
    let entry_block = func.blocks_sorted().into_iter().next();
    let entry_block = match entry_block {
        Some(b) => b.clone(),
        None    => return,
    };

    for stmt in &entry_block.stmts {
        // Pattern: vN = (v_rsp - Const(N))
        // This is emitted by lift_push (rsp - 8) and by `sub rsp, N`.
        if let Stmt::Assign { rhs: Expr::BinOp { op: BinOp::Sub, lhs, rhs }, .. } = stmt {
            if is_rsp_value(lhs) {
                if let Value::Const { val, .. } = rhs {
                    // The largest sub rsp is the frame allocation.
                    if *val > 8 && *val < 65536 {
                        func.frame_size = *val;
                        trace!(func = %func.name, frame_size = val, "frame size from sub rsp");
                        return;
                    }
                }
            }
        }
    }
}

// ── Step 2+3: slot discovery and expression rewriting ────────────────────────

fn rewrite_frame_accesses(
    func:    &mut IrFunction,
    rbp_ids: &[u32],
    rsp_ids: &[u32],
) {
    use petgraph::visit::NodeIndexable;

    let node_count = func.cfg.node_count();
    for ni in 0..node_count {
        let idx = func.cfg.from_index(ni);

        // We need to collect slots while iterating stmts, so we clone and
        // rewrite into a new vec.
        let stmts = std::mem::take(&mut func.cfg[idx].stmts);
        let mut new_stmts = Vec::with_capacity(stmts.len());

        for stmt in stmts {
            let rewritten = rewrite_stmt(stmt, func, rbp_ids, rsp_ids);
            new_stmts.push(rewritten);
        }

        func.cfg[idx].stmts = new_stmts;
    }
}

fn rewrite_stmt(
    stmt:    Stmt,
    func:    &mut IrFunction,
    rbp_ids: &[u32],
    rsp_ids: &[u32],
) -> Stmt {
    match stmt {
        Stmt::Store { ptr, val } => {
            let (new_ptr, _) = resolve_frame_ptr(ptr, func, rbp_ids, rsp_ids);
            Stmt::Store { ptr: new_ptr, val }
        }
        Stmt::Assign { lhs, ty, rhs } => {
            let new_rhs = match rhs {
                Expr::Load { ptr, ty: load_ty } => {
                    let (new_ptr, slot_ty) = resolve_frame_ptr(ptr, func, rbp_ids, rsp_ids);
                    // Narrow the load type if the slot gave us better info.
                    let effective_ty = if slot_ty != IrType::Unknown { slot_ty } else { load_ty };
                    Expr::Load { ptr: new_ptr, ty: effective_ty }
                }
                other => other,
            };
            Stmt::Assign { lhs, ty, rhs: new_rhs }
        }
        other => other,
    }
}

/// If `ptr` looks like `rbp ± K` or `rsp ± K`:
/// 1. Register the slot in `func.slot_table`.
/// 2. Return `(Value::Var { id: slot_var_id, .. }, slot_ty)`.
///
/// Otherwise return the pointer unchanged.
fn resolve_frame_ptr(
    ptr:     Value,
    func:    &mut IrFunction,
    rbp_ids: &[u32],
    rsp_ids: &[u32],
) -> (Value, IrType) {
    // Pattern: Cast(BinOp(Sub|Add, base_reg, Const(K))) where base_reg is rbp/rsp.
    // After MemExpr::emit_addr the pointer is a Cast of an arithmetic result.
    // We look one level through the Cast and check the inner BinOp.

    // Direct Var match (already a pointer var — check if it's rbp/rsp itself).
    if let Value::Var { id, ref ty } = ptr {
        if rbp_ids.contains(&id) {
            // rbp + 0 → saved_rbp slot
            let slot = func.get_or_insert_slot(0, IrType::UInt(64));
            let name = slot.name.clone();
            let slot_ty = slot.ty.clone();
            trace!(func = %func.name, slot = %name, "rbp+0 slot");
            // Return a symbolic var representing the slot.
            // We use a high sentinel id based on the offset.
            return (slot_ptr_val(0), slot_ty);
        }
        if rsp_ids.contains(&id) {
            return (ptr, IrType::Unknown);
        }
    }

    // Try to unwrap Cast → BinOp(Add|Sub, base, Const(K)).
    let offset = extract_rbp_offset(&ptr, rbp_ids);
    if let Some(rbp_offset) = offset {
        let ty = access_type_from_ptr(&ptr);
        let slot = func.get_or_insert_slot(rbp_offset, ty.clone());
        let name = slot.name.clone();
        trace!(func   = %func.name,
               slot   = %name,
               offset = rbp_offset,
               "frame slot resolved");
        return (slot_ptr_val(rbp_offset), ty);
    }

    // rsp-relative: register but don't rename (rsp-relative analysis is trickier).
    let rsp_offset = extract_rsp_offset(&ptr, rsp_ids);
    if let Some(rsp_off) = rsp_offset {
        // Convert to rbp-relative using frame_size:
        // rsp = rbp - frame_size  →  [rsp + K] = [rbp - (frame_size - K)]
        if func.frame_size > 0 {
            let rbp_off = -(func.frame_size as i64) + rsp_off;
            let ty = access_type_from_ptr(&ptr);
            let slot = func.get_or_insert_slot(rbp_off, ty.clone());
            let name = slot.name.clone();
            trace!(func      = %func.name,
                   slot      = %name,
                   rsp_offset = rsp_off,
                   rbp_offset = rbp_off,
                   "rsp-relative slot resolved to rbp-relative");
            return (slot_ptr_val(rbp_off), ty);
        }
    }

    (ptr, IrType::Unknown)
}

// ── Offset extraction helpers ─────────────────────────────────────────────────

/// Try to extract the constant rbp offset from a pointer value.
///
/// Recognises:
/// - `Cast(BinOp(Sub, Var(rbp_id), Const(K)))` → `-K`
/// - `Cast(BinOp(Add, Var(rbp_id), Const(K)))` → `+K`
/// - `Cast(Var(rbp_id))`                        → `0`
fn extract_rbp_offset(ptr: &Value, rbp_ids: &[u32]) -> Option<i64> {
    extract_base_offset(ptr, rbp_ids)
}

fn extract_rsp_offset(ptr: &Value, rsp_ids: &[u32]) -> Option<i64> {
    extract_base_offset(ptr, rsp_ids)
}

fn extract_base_offset(ptr: &Value, base_ids: &[u32]) -> Option<i64> {
    // The ptr value coming from MemExpr::emit_addr is always
    // Var { id = ptr_id } whose rhs is Cast(arithmetic).
    // We can't see through SSA here easily, so we look at the Value directly.
    // In practice after emit_addr the final var is a Cast — but we see the
    // Var reference, not the Expr.  We handle this via the Stmt rewriting:
    // we store a map from ptr_var_id → offset in a side table.
    // For now, handle the simpler case where the pointer IS the rbp/rsp var.
    if let Value::Var { id, .. } = ptr {
        if base_ids.contains(id) {
            return Some(0);
        }
    }
    None
}

/// Try to read the access type from a pointer value's type annotation.
fn access_type_from_ptr(ptr: &Value) -> IrType {
    match ptr.ty() {
        IrType::Ptr(inner) => *inner.clone(),
        _ => IrType::UInt(64),
    }
}

/// Build a stable symbolic `Value` for a slot at `rbp_offset`.
///
/// We encode the offset into a high-range var id so the codegen can
/// look up `func.slot_table` by id.  Ids 10_000..20_000 are reserved.
/// offset=-8 → id=10_008, offset=-16 → id=10_016, offset=16 → id=10_016_positive...
/// We use: id = 10_000 + (offset + 4096) to keep positives in range.
pub fn slot_ptr_val(rbp_offset: i64) -> Value {
    let id = (10_000i64 + rbp_offset + 4096) as u32;
    Value::Var { id, ty: IrType::ptr(IrType::UInt(64)) }
}

/// Inverse: given a slot var id, recover the rbp_offset.
pub fn slot_id_to_offset(id: u32) -> i64 {
    id as i64 - 10_000 - 4096
}

/// Return true if this id is a slot var.
pub fn is_slot_id(id: u32) -> bool {
    id >= 10_000 && id < 20_000
}

// ── Register id collection ────────────────────────────────────────────────────

/// Collect all SSA ids ever assigned to `rbp` in this function.
fn collect_rbp_ids(func: &IrFunction) -> Vec<u32> {
    collect_reg_ids(func, 907) // 907 = placeholder id for rbp
}

fn collect_rsp_ids(func: &IrFunction) -> Vec<u32> {
    collect_reg_ids(func, 906) // 906 = placeholder id for rsp
}

fn collect_reg_ids(func: &IrFunction, placeholder: u32) -> Vec<u32> {
    // Start with the placeholder id (used when the register has never been
    // written — e.g. rsp at function entry before any push).
    let mut ids = vec![placeholder];

    // Walk all stmts looking for patterns that write to the register.
    // We look for:
    //   vN = v_rsp - 8          (push: rsp update)
    //   vN = v_rsp + 8          (pop: rsp update)
    //   vN = v_rsp - frame_size (sub rsp, N)
    //   vN = v_rbp              (mov rbp, rsp in prologue)
    for bb in func.blocks_sorted() {
        for stmt in &bb.stmts {
            if let Stmt::Assign { lhs, rhs, .. } = stmt {
                match rhs {
                    Expr::BinOp { lhs: base, .. } => {
                        if let Value::Var { id, .. } = base {
                            if ids.contains(id) {
                                ids.push(*lhs);
                            }
                        }
                    }
                    Expr::Value(Value::Var { id, .. }) => {
                        if ids.contains(id) {
                            ids.push(*lhs);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    ids.dedup();
    ids
}

/// Return true if this value references an rsp-family var.
fn is_rsp_value(v: &Value) -> bool {
    if let Value::Var { id, .. } = v {
        // 906 = rsp placeholder; in practice rsp id changes every push/pop.
        // We check if it's exactly the placeholder here — full tracking
        // happens via collect_rsp_ids above.
        *id == 906
    } else {
        false
    }
}
