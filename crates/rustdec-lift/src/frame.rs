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
//! 2. **Eliminate ABI noise** — nop prologue stmts, epilogue stmts, and
//!    callee-saved push/pop pairs so the codegen sees clean IR.
//!    Patterns handled:
//!    - Standard prologue: `push rbp` + `mov rbp, rsp` + `sub rsp, K`
//!    - Callee-saved pushes at function entry (rbx, r12-r15)
//!    - Standard epilogue: `leave` (= `mov rsp, rbp` + `pop rbp`) or
//!      the long form `mov rsp, rbp` / `pop rbp` before `ret`
//!    - Symmetric callee-saved pops in epilogue
//!
//! 3. **Discover stack slots** — scan every `Store`/`Load` whose pointer
//!    expression resolves to `rbp ± constant` or `rsp ± constant` (after
//!    adjusting for the known frame layout) and insert a `StackSlot` into
//!    `func.slot_table`.
//!
//! 4. **Rewrite pointer expressions** — replace `Expr::BinOp { Sub/Add, v_rbp, K }`
//!    patterns in `Stmt::Load`/`Stmt::Store` with a direct reference to the
//!    named slot variable, so the codegen can emit `local_0` instead of
//!    `*(v907 - 0x8)`.
//!
//! 5. **Red zone** — leaf functions (no `sub rsp`) use the 128-byte red zone
//!    below rsp.  `Store(rsp - K, val)` patterns are treated as implicit locals.
//!
//! 6. **Dynamic alloca** — `rsp = rsp - reg` (non-constant) is flagged on
//!    `func.has_dynamic_alloca` so the codegen can emit `alloca(size)`.
//!
//! ## Limitations
//!
//! - Only handles rbp-relative and rsp-relative addressing.
//! - Type inference is heuristic: slot type defaults to `UInt(64)` unless the
//!   access size can be read from the `IrType` of the `Load`/`Store`.

use rustdec_ir::{BinOp, Expr, IrFunction, IrType, Stmt, Terminator, Value};
use std::collections::HashMap;
use tracing::{debug, trace};

// ── Public entry point ────────────────────────────────────────────────────────

/// Run the stack frame analysis pass on `func` in-place.
pub fn analyse_frame(func: &mut IrFunction) {
    // ── Step 1: detect frame size from prologue ───────────────────────────────
    let rbp_seed = reg_seed_id(func, "rbp");
    let rsp_seed = reg_seed_id(func, "rsp");

    detect_frame_size(func, rsp_seed);
    debug!(func = %func.name,
           frame_size = func.frame_size,
           rsp_seed, rbp_seed,
           "frame size detected");

    // ── Step 2: build def map and collect register id chains ──────────────────
    let def_map  = build_def_map(func);
    let rbp_ids  = collect_reg_ids_from_seed(func, rbp_seed, &def_map);
    let rsp_ids  = collect_reg_ids_from_seed(func, rsp_seed, &def_map);

    debug!(func = %func.name,
           rbp_ids = rbp_ids.len(),
           rsp_ids = rsp_ids.len(),
           "frame register ids collected");

    // ── Step 3: eliminate ABI noise (prologue / epilogue / callee-saved) ──────
    eliminate_abi_noise(func, &rbp_ids, &rsp_ids, &def_map);

    // ── Step 4: discover slots + rewrite expressions ──────────────────────────
    rewrite_frame_accesses(func, &rbp_ids, &rsp_ids, &def_map);

    debug!(func  = %func.name,
           slots = func.slot_table.len(),
           "frame analysis complete");
}

// ── Seed id helpers ───────────────────────────────────────────────────────────

/// Return the initial SSA id for a register by looking up `func.reg_names`.
/// Falls back to a legacy sentinel value when the register was never seeded.
fn reg_seed_id(func: &IrFunction, reg: &str) -> u32 {
    func.reg_names
        .iter()
        .find_map(|(&id, name)| if name == reg { Some(id) } else { None })
        .unwrap_or_else(|| match reg {
            "rsp" => 906,
            "rbp" => 907,
            _     => u32::MAX,
        })
}

// ── Step 1: prologue detection ────────────────────────────────────────────────

/// Scan the entry block for `sub rsp, N` and set `func.frame_size = N`.
///
/// We look for the *largest* `rsp = rsp - Const(K)` with K in (8, 65536).
/// This filters out the `rsp -= 8` emitted by each `push` (K = 8) and
/// captures only the frame-allocation sub.
fn detect_frame_size(func: &mut IrFunction, rsp_seed: u32) {
    let entry_block = func.blocks_sorted().into_iter().next().cloned();
    let entry_block = match entry_block {
        Some(b) => b,
        None    => return,
    };

    // Track rsp ids forward through the entry block.
    let mut rsp_ids: Vec<u32> = vec![rsp_seed];

    for stmt in &entry_block.stmts {
        if let Stmt::Assign { lhs, rhs, .. } = stmt {
            match rhs {
                Expr::BinOp { op: BinOp::Sub, lhs: base, rhs: Value::Const { val, .. } } => {
                    if let Value::Var { id, .. } = base {
                        if rsp_ids.contains(id) {
                            rsp_ids.push(*lhs);
                            if *val > 8 && *val < 65536 {
                                func.frame_size = *val;
                                trace!(func = %func.name, frame_size = val,
                                       "frame size from sub rsp");
                                return;
                            }
                        }
                    }
                }
                Expr::BinOp { op: BinOp::Add, lhs: base, .. } |
                Expr::BinOp { lhs: base, .. } => {
                    if let Value::Var { id, .. } = base {
                        if rsp_ids.contains(id) {
                            rsp_ids.push(*lhs);
                        }
                    }
                }
                Expr::Value(Value::Var { id, .. }) => {
                    if rsp_ids.contains(id) { rsp_ids.push(*lhs); }
                }
                _ => {}
            }
        }
    }
}

// ── Step 2: def map + register chain collection ───────────────────────────────

/// Build a function-wide `var_id → Expr` map from all `Assign` stmts.
///
/// This is the lightweight use-def chain that lets `resolve_offset_from_id`
/// see through SSA copies and arithmetic without a full dataflow pass.
fn build_def_map(func: &IrFunction) -> HashMap<u32, Expr> {
    let mut map = HashMap::new();
    for bb in func.cfg.node_weights() {
        for stmt in &bb.stmts {
            if let Stmt::Assign { lhs, rhs, .. } = stmt {
                map.insert(*lhs, rhs.clone());
            }
        }
    }
    map
}

/// Collect all SSA ids whose value derives from `seed_id` via copies or
/// arithmetic (the full rsp / rbp "spine").
fn collect_reg_ids_from_seed(
    func:    &IrFunction,
    seed_id: u32,
    def_map: &HashMap<u32, Expr>,
) -> Vec<u32> {
    let mut ids = vec![seed_id];
    // Fixed-point: keep adding ids whose rhs base is already in the set.
    loop {
        let prev_len = ids.len();
        for bb in func.cfg.node_weights() {
            for stmt in &bb.stmts {
                if let Stmt::Assign { lhs, rhs, .. } = stmt {
                    if ids.contains(lhs) { continue; }
                    let base_id = match rhs {
                        Expr::BinOp { lhs: base, .. } => match base {
                            Value::Var { id, .. } => Some(*id),
                            _ => None,
                        },
                        Expr::Value(Value::Var { id, .. }) => Some(*id),
                        Expr::Cast  { val: Value::Var { id, .. }, .. } => Some(*id),
                        _ => None,
                    };
                    if let Some(b) = base_id {
                        if ids.contains(&b) { ids.push(*lhs); }
                    }
                }
            }
        }
        if ids.len() == prev_len { break; }
    }
    // Also sweep the def_map for any ids we might have missed (e.g. multi-block).
    // Already covered by the block walk above, but deduplicate.
    ids.dedup();
    ids
}

/// Given a var id, compute its signed offset from any var in `base_ids`.
///
/// Follows the def_map chain up to `depth` levels.  Returns `Some(offset)` if
/// the chain terminates at a `base_ids` member, `None` otherwise.
fn resolve_offset_from_id(
    id:       u32,
    base_ids: &[u32],
    def_map:  &HashMap<u32, Expr>,
    depth:    u32,
) -> Option<i64> {
    if depth > 64 { return None; }
    if base_ids.contains(&id) { return Some(0); }
    match def_map.get(&id)? {
        Expr::BinOp { op: BinOp::Sub, lhs: Value::Var { id: src, .. }, rhs: Value::Const { val, .. } } => {
            let base = resolve_offset_from_id(*src, base_ids, def_map, depth + 1)?;
            Some(base - *val as i64)
        }
        Expr::BinOp { op: BinOp::Add, lhs: Value::Var { id: src, .. }, rhs: Value::Const { val, .. } } => {
            let base = resolve_offset_from_id(*src, base_ids, def_map, depth + 1)?;
            Some(base + *val as i64)
        }
        Expr::Value(Value::Var { id: src, .. }) |
        Expr::Cast  { val: Value::Var { id: src, .. }, .. } => {
            resolve_offset_from_id(*src, base_ids, def_map, depth + 1)
        }
        _ => None,
    }
}

fn resolve_offset(ptr: &Value, base_ids: &[u32], def_map: &HashMap<u32, Expr>) -> Option<i64> {
    match ptr {
        Value::Var { id, .. } => resolve_offset_from_id(*id, base_ids, def_map, 0),
        _ => None,
    }
}

// ── Step 3: ABI noise elimination ────────────────────────────────────────────

/// Callee-saved registers per System V AMD64 ABI.
fn is_callee_saved(name: &str) -> bool {
    matches!(name, "rbx" | "r12" | "r13" | "r14" | "r15")
}

/// Nop all ABI-mandated frame stmts so the codegen sees clean IR:
///
/// **Entry block** — scan forward and nop:
/// - `push reg` (rsp_dec + Store) where reg is callee-saved or rbp
/// - `mov rbp, rsp` assignment
/// - `sub rsp, K` frame allocation
///
/// **Return blocks** — scan backward from the terminator and nop:
/// - `leave` stmts (mov rsp, rbp + pop rbp)
/// - `pop reg` where reg is callee-saved
fn eliminate_abi_noise(
    func:    &mut IrFunction,
    rbp_ids: &[u32],
    rsp_ids: &[u32],
    def_map: &HashMap<u32, Expr>,
) {
    use petgraph::visit::NodeIndexable;
    let node_count = func.cfg.node_count();

    // Find entry block (lowest start_addr).
    let entry_idx = (0..node_count)
        .map(|ni| func.cfg.from_index(ni))
        .min_by_key(|&idx| func.cfg[idx].start_addr);

    if let Some(idx) = entry_idx {
        eliminate_prologue(func, idx, rbp_ids, rsp_ids, def_map);
    }

    // Epilogue: every block that ends with Return.
    for ni in 0..node_count {
        let idx = func.cfg.from_index(ni);
        if matches!(func.cfg[idx].terminator, Terminator::Return(_)) {
            eliminate_epilogue(func, idx, rbp_ids, rsp_ids, def_map);
        }
    }
}

/// Nop prologue stmts in the entry block.
///
/// Handles both orderings:
/// - Callee-saved pushes *before* `push rbp` (Linux/GCC style)
/// - Callee-saved pushes *after*  `sub rsp, K` (less common)
fn eliminate_prologue(
    func:    &mut IrFunction,
    idx:     petgraph::graph::NodeIndex,
    rbp_ids: &[u32],
    rsp_ids: &[u32],
    def_map: &HashMap<u32, Expr>,
) {
    // Pre-collect callee-saved seed ids (id → canonical name).
    // Done here (before any mutable borrow of func) to avoid borrow-checker issues.
    let callee_ids: HashMap<u32, String> = func.reg_names
        .iter()
        .filter(|(_, name)| is_callee_saved(name))
        .map(|(&id, name)| (id, name.clone()))
        .collect();

    // We need mutable access to stmts, so we take ownership temporarily.
    let mut stmts = std::mem::take(&mut func.cfg[idx].stmts);
    let n = stmts.len();

    // `pending_dec`: index of the latest `rsp = rsp - 8` not yet matched
    // to a Store.  The two stmts (dec + store) are always adjacent in a push.
    let mut pending_dec: Option<(usize, u32)> = None; // (stmt_idx, new_rsp_id)
    let mut saw_push_rbp    = false;
    let mut saw_mov_rbp_rsp = false;
    let mut sub_rsp_done    = false;

    let mut i = 0;
    while i < n && !sub_rsp_done {
        // ── Pattern A: rsp = rsp - 8  (first half of any push) ───────────────
        if let Some(new_rsp_id) = is_rsp_dec_by_8(&stmts[i], rsp_ids) {
            // Don't override an unmatched pending_dec — that means the last
            // rsp-8 wasn't followed by a store (unexpected), stop here.
            if pending_dec.is_some() { break; }
            pending_dec = Some((i, new_rsp_id));
            i += 1;
            continue;
        }

        // ── Pattern B: Store(rsp_new, val) — second half of a push ───────────
        if let Some((dec_idx, rsp_new_id)) = pending_dec {
            if let Stmt::Store { ptr: Value::Var { id: ptr_id, .. }, val } = &stmts[i] {
                if *ptr_id == rsp_new_id {
                    let stored_id = match val {
                        Value::Var { id, .. } => Some(*id),
                        _ => None,
                    };
                    let is_rbp    = stored_id.map_or(false, |id| rbp_ids.contains(&id));
                    let cs_name   = stored_id.and_then(|id| callee_ids.get(&id).cloned());
                    let is_callee = cs_name.is_some();

                    if is_rbp {
                        // push rbp — nop both dec and store.
                        stmts[dec_idx] = Stmt::Nop;
                        stmts[i]       = Stmt::Nop;
                        saw_push_rbp   = true;
                        pending_dec    = None;
                        i += 1;
                        continue;
                    } else if is_callee {
                        // push r12/r13/r14/r15/rbx — nop + register SavedReg slot.
                        if let Some(off) = resolve_offset_from_id(rsp_new_id, rsp_ids, def_map, 0) {
                            let rbp_off = if func.frame_size > 0 {
                                -(func.frame_size as i64) + off
                            } else {
                                off
                            };
                            let slot = func.get_or_insert_slot(rbp_off, IrType::UInt(64));
                            slot.origin = rustdec_ir::SlotOrigin::SavedReg;
                            trace!(func = %func.name,
                                   reg  = cs_name,
                                   rbp_off, "callee-saved push eliminated");
                        }
                        stmts[dec_idx] = Stmt::Nop;
                        stmts[i]       = Stmt::Nop;
                        pending_dec    = None;
                        i += 1;
                        continue;
                    } else {
                        // Unknown push — stop prologue scan.
                        break;
                    }
                }
            }
            // Expected Store but got something else — stop.
            break;
        }

        // ── Pattern C: rbp = rsp  (mov rbp, rsp) ─────────────────────────────
        // Assign(lhs, Value(Var(rsp_id))) where rsp_id in rsp_ids.
        // Only match after push rbp to avoid false positives.
        if saw_push_rbp && !saw_mov_rbp_rsp {
            if let Stmt::Assign { rhs: Expr::Value(Value::Var { id, .. }), .. } = &stmts[i] {
                if rsp_ids.contains(id) {
                    stmts[i]        = Stmt::Nop;
                    saw_mov_rbp_rsp = true;
                    i += 1;
                    continue;
                }
            }
        }

        // ── Pattern D: rsp = rsp - K  (sub rsp, frame_size) ──────────────────
        if saw_mov_rbp_rsp && func.frame_size > 0 {
            if let Stmt::Assign { rhs: Expr::BinOp { op: BinOp::Sub, lhs: base, rhs: Value::Const { val, .. } }, .. } = &stmts[i] {
                if let Value::Var { id, .. } = base {
                    if rsp_ids.contains(id) && *val == func.frame_size {
                        stmts[i]    = Stmt::Nop;
                        sub_rsp_done = true;
                        i += 1;
                        continue;
                    }
                }
            }
        }

        // Anything else → stop prologue scan.
        break;
    }

    // Callee-saved pushes *after* sub rsp, K — less common but real (MSVC style).
    // We continue scanning only if sub_rsp_done.
    if sub_rsp_done {
        while i < n {
            if let Some(new_rsp_id) = is_rsp_dec_by_8(&stmts[i], rsp_ids) {
                let next = i + 1;
                if next < n {
                    if let Stmt::Store { ptr: Value::Var { id: ptr_id, .. }, val } = &stmts[next] {
                        if *ptr_id == new_rsp_id {
                            let stored_id = match val {
                                Value::Var { id, .. } => Some(*id),
                                _ => None,
                            };
                            let is_callee = stored_id.map_or(false, |id| {
                                callee_ids.contains_key(&id)
                            });
                            if is_callee {
                                stmts[i]    = Stmt::Nop;
                                stmts[next] = Stmt::Nop;
                                i += 2;
                                continue;
                            }
                        }
                    }
                }
            }
            break;
        }
    }

    func.cfg[idx].stmts = stmts;
}

/// Nop epilogue stmts in a block ending with `Terminator::Return`.
///
/// Scans backward from the last stmt and nops:
/// - `rsp = rbp`          — `mov rsp, rbp` (first half of `leave`)
/// - `x  = Load(rsp)`     — `pop rbp` load (often already DCE'd)
/// - `rsp = rsp + 8`      — `pop rbp` rsp-adjust (symmetric to prologue push)
///
/// Multiple such groups are consumed (callee-saved pops + pop rbp).
/// The scan stops at the first stmt that doesn't fit these patterns.
fn eliminate_epilogue(
    func:    &mut IrFunction,
    idx:     petgraph::graph::NodeIndex,
    rbp_ids: &[u32],
    rsp_ids: &[u32],
    _def_map: &HashMap<u32, Expr>,
) {
    let mut stmts = std::mem::take(&mut func.cfg[idx].stmts);
    let mut to_nop: Vec<usize> = Vec::new();
    let mut i = stmts.len();

    while i > 0 {
        i -= 1;
        let stmt = &stmts[i];

        // rsp = rbp  (mov rsp, rbp — first half of leave)
        if let Stmt::Assign { rhs: Expr::Value(Value::Var { id, .. }), lhs, .. } = stmt {
            if rbp_ids.contains(id) && rsp_ids.contains(lhs) {
                to_nop.push(i);
                continue;
            }
        }

        // rsp = rsp + 8  (pop rsp-adjust, symmetric to prologue push rsp-8)
        if let Stmt::Assign { rhs: Expr::BinOp { op: BinOp::Add, lhs: base, rhs: Value::Const { val, .. } }, .. } = stmt {
            if *val == 8 {
                if let Value::Var { id, .. } = base {
                    if rsp_ids.contains(id) {
                        to_nop.push(i);
                        continue;
                    }
                }
            }
        }

        // x = Load(rsp)  (pop load — may survive DCE if result is live)
        // Nop only if the result is rbp or a callee-saved id (seed id check).
        if let Stmt::Assign { lhs, rhs: Expr::Load { ptr: Value::Var { id: ptr_id, .. }, .. }, .. } = stmt {
            if rsp_ids.contains(ptr_id) && rbp_ids.contains(lhs) {
                to_nop.push(i);
                continue;
            }
        }

        break;
    }

    for j in to_nop {
        stmts[j] = Stmt::Nop;
    }

    func.cfg[idx].stmts = stmts;
}

// ── Pattern predicates ────────────────────────────────────────────────────────

/// If `stmt` is `lhs = rsp_prev - 8`, return `Some(lhs)`.
fn is_rsp_dec_by_8(stmt: &Stmt, rsp_ids: &[u32]) -> Option<u32> {
    if let Stmt::Assign { lhs, rhs: Expr::BinOp { op: BinOp::Sub, lhs: base, rhs: Value::Const { val: 8, .. } }, .. } = stmt {
        if let Value::Var { id, .. } = base {
            if rsp_ids.contains(id) {
                return Some(*lhs);
            }
        }
    }
    None
}

// ── Step 4+5: slot discovery and expression rewriting ─────────────────────────

fn rewrite_frame_accesses(
    func:    &mut IrFunction,
    rbp_ids: &[u32],
    rsp_ids: &[u32],
    def_map: &HashMap<u32, Expr>,
) {
    use petgraph::visit::NodeIndexable;
    let node_count = func.cfg.node_count();
    for ni in 0..node_count {
        let idx = func.cfg.from_index(ni);
        let stmts = std::mem::take(&mut func.cfg[idx].stmts);
        let mut new_stmts = Vec::with_capacity(stmts.len());
        for stmt in stmts {
            let rewritten = rewrite_stmt(stmt, func, rbp_ids, rsp_ids, def_map);
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
    def_map: &HashMap<u32, Expr>,
) -> Stmt {
    match stmt {
        Stmt::Store { ptr, val } => {
            let (new_ptr, _) = resolve_frame_ptr(ptr, func, rbp_ids, rsp_ids, def_map);
            Stmt::Store { ptr: new_ptr, val }
        }
        Stmt::Assign { lhs, ty, rhs } => {
            let new_rhs = match rhs {
                Expr::Load { ptr, ty: load_ty } => {
                    let (new_ptr, slot_ty) = resolve_frame_ptr(ptr, func, rbp_ids, rsp_ids, def_map);
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

/// Try to resolve `ptr` to a named frame slot.
///
/// Resolution order:
/// 1. rbp ± K → local / arg slot
/// 2. rsp ± K → converted to rbp-relative using `frame_size`
/// 3. rsp ± K with `frame_size == 0` → red-zone local (leaf function)
fn resolve_frame_ptr(
    ptr:     Value,
    func:    &mut IrFunction,
    rbp_ids: &[u32],
    rsp_ids: &[u32],
    def_map: &HashMap<u32, Expr>,
) -> (Value, IrType) {
    // ── rbp-relative ─────────────────────────────────────────────────────────
    if let Some(rbp_off) = resolve_offset(&ptr, rbp_ids, def_map) {
        let ty   = access_type_from_ptr(&ptr);
        let slot = func.get_or_insert_slot(rbp_off, ty.clone());
        let name = slot.name.clone();
        trace!(func = %func.name, slot = %name, offset = rbp_off,
               "frame slot resolved (rbp-relative)");
        return (slot_ptr_val(rbp_off), ty);
    }

    // ── rsp-relative ─────────────────────────────────────────────────────────
    if let Some(rsp_off) = resolve_offset(&ptr, rsp_ids, def_map) {
        let ty = access_type_from_ptr(&ptr);

        if func.frame_size > 0 {
            // Standard frame: rsp = rbp - frame_size → [rsp + K] = [rbp - (frame_size - K)]
            let rbp_off = -(func.frame_size as i64) + rsp_off;
            let slot = func.get_or_insert_slot(rbp_off, ty.clone());
            let name = slot.name.clone();
            trace!(func = %func.name, slot = %name,
                   rsp_off, rbp_off, "rsp-relative slot → rbp-relative");
            return (slot_ptr_val(rbp_off), ty);
        } else if rsp_off < 0 {
            // Red zone: leaf function, no sub rsp.  rsp is the frame base.
            // [rsp - K] is equivalent to [entry_rsp - K] = local slot.
            let slot = func.get_or_insert_slot(rsp_off, ty.clone());
            let name = slot.name.clone();
            trace!(func = %func.name, slot = %name,
                   rsp_off, "red-zone slot (leaf function)");
            return (slot_ptr_val(rsp_off), ty);
        }
    }

    (ptr, IrType::Unknown)
}

// ── Offset helpers ────────────────────────────────────────────────────────────

/// Try to read the pointee type from a pointer value's type annotation.
fn access_type_from_ptr(ptr: &Value) -> IrType {
    match ptr.ty() {
        IrType::Ptr(inner) => *inner.clone(),
        _ => IrType::UInt(64),
    }
}

/// Build a stable symbolic `Value` for a slot at `rbp_offset`.
///
/// Ids 10_000..20_000 are reserved for slot pointers.
/// Encoding: id = 10_000 + (offset + 4096) keeps the range positive.
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

// ── Legacy public helpers (kept for codegen compatibility) ────────────────────

/// Return true if this value references an rsp-family var.
///
/// Used by the legacy codegen path — prefer `resolve_offset` for new code.
pub fn is_rsp_value(v: &Value) -> bool {
    if let Value::Var { id, .. } = v { *id == 906 } else { false }
}
