//! C99 pseudo-code backend — structured output using if/while/for.
//!
//! ## P1 improvements applied in this version
//!
//! ### Variable hoisting
//! All SSA variable declarations (`uint64_t vN`) are collected in a first pass
//! and emitted at the top of the function body, one per unique (id, type) pair.
//! This produces valid C even when the structured tree visits a variable
//! multiple times (e.g. after a loop back-edge).
//!
//! ### Copy-propagation
//! Before emitting, we build a substitution table of trivial assignments of
//! the form `vN = vM` (identity copies) or `vN = <const>`.  When emitting
//! expressions, every `vN` that has a substitution is replaced inline.
//! This eliminates the chains of temporaries the SSA lifter produces:
//!
//! ```c
//! // Before:   uint64_t v12 = v4;  uint64_t v13 = v12;  foo(v13)
//! // After:    foo(v4)
//! ```
//!
//! The substitution is intentionally conservative:
//! - Only pure `Expr::Value` RHS are propagated (no side-effects).
//! - A variable is only substituted if it is assigned exactly once in the
//!   whole function (checked via the use-count map built in the first pass).

use std::collections::{HashMap, HashSet};

use rustdec_analysis::{structure_function, CondExpr, SNode};
use rustdec_lift::frame::{is_slot_id, slot_id_to_offset};
use rustdec_ir::{BinOp, BasicBlock, CallTarget, Expr, IrFunction, IrType, Stmt, Terminator, Value};
use tracing::{debug, trace, warn};

use crate::{CodegenBackend, CodegenResult};

/// C99 code generation backend.
///
/// `string_table` is optional — when provided, `Expr::StringRef` nodes
/// are emitted as C string literals instead of hex addresses.
pub struct CBackend {
    pub string_table: HashMap<u64, String>,
}

// ── Trait implementation ──────────────────────────────────────────────────────

impl CodegenBackend for CBackend {
    fn emit_function(&self, func: &IrFunction) -> CodegenResult<String> {
        let ret    = self.emit_type(&func.ret_ty);
        let params = if func.params.is_empty() {
            "void".to_string()
        } else {
            func.params.iter().enumerate()
                .map(|(i, ty)| format!("{} a{i}", self.emit_type(ty)))
                .collect::<Vec<_>>().join(", ")
        };

        debug!(func = %func.name, ret = %ret, params = func.params.len(),
               "C: emitting function");

        let blocks: Vec<&BasicBlock> = func.blocks_sorted();

        // ── Pass 1: collect variables, copy table, and written-var set ─────────
        let mut var_decls: HashMap<u32, IrType> = HashMap::new();
        let mut assign_count: HashMap<u32, usize> = HashMap::new();
        // Maps vN → Value when vN = <pure copy>
        let mut copy_table: HashMap<u32, Value> = HashMap::new();
        // SSA IDs that were explicitly written somewhere in this function.
        // Used to filter spurious ABI placeholder arguments from calls.
        let mut written_vars: HashSet<u32> = HashSet::new();

        for bb in &blocks {
            for stmt in &bb.stmts {
                if let Stmt::Assign { lhs, ty, rhs } = stmt {
                    var_decls.entry(*lhs).or_insert_with(|| ty.clone());
                    *assign_count.entry(*lhs).or_insert(0) += 1;
                    written_vars.insert(*lhs);
                    if let Expr::Value(v) = rhs {
                        copy_table.insert(*lhs, v.clone());
                    }
                }
            }
        }

        // Only propagate variables assigned exactly once (safe in SSA).
        copy_table.retain(|id, _| assign_count.get(id).copied().unwrap_or(0) == 1);

        // Flatten the copy table transitively (vA→vB→vC becomes vA→vC).
        let copy_table = flatten_copies(copy_table);

        // Variables that are copy-propagated away don't need declarations.
        let suppressed: HashSet<u32> = copy_table.keys().copied().collect();

        // ── Emit function header ──────────────────────────────────────────────
        let mut out = String::new();
        out.push_str(&format!("// RustDec decompilation — {}\n", func.name));
        out.push_str(&format!("{ret} {}({params}) {{\n", sanitise_name(&func.name)));

        // ── Emit variable declarations (hoisted) ──────────────────────────────
        let mut decl_lines: Vec<String> = var_decls
            .iter()
            .filter(|(id, _)| !suppressed.contains(id))
            .map(|(id, ty)| format!("  {} v{id};", self.emit_type(ty)))
            .collect();
        decl_lines.sort(); // deterministic output
        if !decl_lines.is_empty() {
            for line in &decl_lines {
                out.push_str(line);
                out.push('\n');
            }
            out.push('\n');
        }

        // ── Emit structured body ──────────────────────────────────────────────
        let structured = structure_function(func);
        self.emit_node(&structured.root, &structured, &copy_table, &written_vars, &func.slot_table, &mut out, 1);

        out.push_str("}\n");
        debug!(func = %func.name, lines = out.lines().count(),
               vars = var_decls.len(), propagated = suppressed.len(),
               "C: function emitted");
        Ok(out)
    }

    fn emit_type(&self, ty: &IrType) -> String {
        match ty {
            IrType::UInt(8)             => "uint8_t".into(),
            IrType::UInt(16)            => "uint16_t".into(),
            IrType::UInt(32)            => "uint32_t".into(),
            IrType::UInt(64)            => "uint64_t".into(),
            IrType::SInt(8)             => "int8_t".into(),
            IrType::SInt(16)            => "int16_t".into(),
            IrType::SInt(32)            => "int32_t".into(),
            IrType::SInt(64)            => "int64_t".into(),
            IrType::Float(32)           => "float".into(),
            IrType::Float(64)           => "double".into(),
            IrType::Ptr(inner)          => format!("{}*", self.emit_type(inner)),
            IrType::Array { elem, len } => format!("{}[{len}]", self.emit_type(elem)),
            IrType::Struct { name, .. } => format!("struct {name}"),
            IrType::Void                => "void".into(),
            other => {
                warn!(ty = ?other, "C: unknown type");
                "/* unknown */".into()
            }
        }
    }
}

// ── Copy-propagation helper ───────────────────────────────────────────────────

/// Flatten a copy table transitively.
///
/// If `table[A] = Var(B)` and `table[B] = Var(C)`, replace both with `Var(C)`.
/// Stops after 16 iterations to guard against cycles (shouldn't happen in SSA).
fn flatten_copies(mut table: HashMap<u32, Value>) -> HashMap<u32, Value> {
    for _ in 0..16 {
        let mut changed = false;
        let snapshot = table.clone();
        for val in table.values_mut() {
            if let Value::Var { id, .. } = val {
                if let Some(deeper) = snapshot.get(id) {
                    *val = deeper.clone();
                    changed = true;
                }
            }
        }
        if !changed { break; }
    }
    table
}

/// Resolve a `Value` through the copy-propagation table.
fn resolve<'a>(v: &'a Value, copies: &'a HashMap<u32, Value>) -> &'a Value {
    if let Value::Var { id, .. } = v {
        if let Some(subst) = copies.get(id) {
            return resolve(subst, copies); // one level of recursion is safe post-flatten
        }
    }
    v
}

/// Display a `Value`, substituting named slot variables where possible.
///
/// If the value's SSA id corresponds to a stack slot, we return the
/// slot name (`local_0`, `arg_1`, …) instead of `vN`.
fn display_value(
    v:          &Value,
    copies:     &HashMap<u32, Value>,
    slot_table: &std::collections::HashMap<i64, rustdec_ir::StackSlot>,
) -> String {
    let resolved = resolve(v, copies);
    if let Value::Var { id, .. } = resolved {
        if is_slot_id(*id) {
            let offset = slot_id_to_offset(*id);
            if let Some(slot) = slot_table.get(&offset) {
                return format!("&{}", slot.name);
            }
        }
    }
    resolved.display()
}

// ── Structured tree emitter ───────────────────────────────────────────────────

impl CBackend {
    fn indent(depth: usize) -> String {
        "  ".repeat(depth)
    }

    fn emit_node(
        &self,
        node:    &SNode,
        sfunc:   &rustdec_analysis::StructuredFunc,
        copies:  &HashMap<u32, Value>,
        written: &HashSet<u32>,
        _slots:  &std::collections::HashMap<i64, rustdec_ir::StackSlot>,
        out:     &mut String,
        depth:   usize,
    ) {
        let ind = Self::indent(depth);
        match node {
            SNode::Block(id) => {
                if let Some(bb) = sfunc.blocks.get(id) {
                    trace!(block = format_args!("{:#x}", bb.start_addr),
                           stmts = bb.stmts.len(),
                           "C: emit block");
                    for stmt in &bb.stmts {
                        if let Some(line) = self.emit_stmt_opt(stmt, copies, written, _slots) {
                            out.push_str(&format!("{ind}{line};\n"));
                        }
                    }
                    // Return statement — value already patched by lifter.
                    match &bb.terminator {
                        Terminator::Return(Some(v)) => {
                            let resolved = resolve(v, copies);
                            out.push_str(&format!("{ind}return {};\n",
                                                  resolved.display()));
                        }
                        Terminator::Return(None) => {
                            out.push_str(&format!("{ind}return;\n"));
                        }
                        _ => {}
                    }
                }
            }

            SNode::Seq(nodes) => {
                for n in nodes {
                    self.emit_node(n, sfunc, copies, written, _slots, out, depth);
                }
            }

            SNode::IfElse { cond, then, else_ } => {
                let cond_str = self.emit_cond(cond, sfunc, copies, written, _slots);
                out.push_str(&format!("{ind}if ({cond_str}) {{\n"));
                self.emit_node(then, sfunc, copies, written, _slots, out, depth + 1);
                let mut else_buf = String::new();
                self.emit_node(else_, sfunc, copies, written, _slots, &mut else_buf, depth + 1);
                if !else_buf.trim().is_empty() {
                    out.push_str(&format!("{ind}}} else {{\n"));
                    out.push_str(&else_buf);
                }
                out.push_str(&format!("{ind}}}\n"));
            }

            SNode::Loop { cond, body } => {
                let cond_str = self.emit_cond(cond, sfunc, copies, written, _slots);
                out.push_str(&format!("{ind}while ({cond_str}) {{\n"));
                self.emit_node(body, sfunc, copies, written, _slots, out, depth + 1);
                out.push_str(&format!("{ind}}}\n"));
            }

            SNode::Break    => out.push_str(&format!("{ind}break;\n")),
            SNode::Continue => out.push_str(&format!("{ind}continue;\n")),
        }
    }

    /// Emit one statement, or `None` if it should be suppressed.
    ///
    /// Assignments of the form `vN = vM` where `vN` is in the copy table
    /// are suppressed entirely — the substitution happens at use-sites.
    /// `Stmt::Nop` is always suppressed.
    fn emit_stmt_opt(
        &self,
        stmt:   &Stmt,
        copies: &HashMap<u32, Value>,
        written: &HashSet<u32>,
        _slots:  &std::collections::HashMap<i64, rustdec_ir::StackSlot>,
    ) -> Option<String> {
        match stmt {
            Stmt::Nop => None,

            Stmt::Assign { lhs, ty: _, rhs } => {
                // Suppress if this variable is copy-propagated away.
                if copies.contains_key(lhs) {
                    return None;
                }
                let rhs_str = self.emit_expr_resolved(rhs, copies, written, _slots);
                // Emit as assignment (no type — variables are declared at top).
                // Slot vars are never assigned to directly (they are addressed
                // via their pointer), so we suppress them here.
                if is_slot_id(*lhs) {
                    return None;
                }
                Some(format!("v{lhs} = {rhs_str}"))
            }

            Stmt::Store { ptr, val } => {
                let ptr_r = resolve(ptr, copies);
                let val_r = resolve(val, copies);
                // Slot store → named assignment.
                if let Value::Var { id, .. } = ptr_r {
                    if is_slot_id(*id) {
                        let offset = slot_id_to_offset(*id);
                        if let Some(slot) = _slots.get(&offset) {
                            return Some(format!("{} = {}", slot.name, val_r.display()));
                        }
                    }
                }
                Some(format!("*({}) = {}", ptr_r.display(), val_r.display()))
            }
        }
    }

    /// Emit a condition expression for `if`/`while` headers.
    fn emit_cond(
        &self,
        cond:    &CondExpr,
        sfunc:   &rustdec_analysis::StructuredFunc,
        copies:  &HashMap<u32, Value>,
        _written: &HashSet<u32>,
        _slots:  &std::collections::HashMap<i64, rustdec_ir::StackSlot>,
    ) -> String {
        let bb = sfunc.blocks.values()
            .find(|b| b.start_addr == cond.block_addr);

        if let Some(bb) = bb {
            // Find the last cmp/test — it produced the flag variable.
            let cmp = bb.stmts.iter().rev().find(|s| {
                matches!(s, Stmt::Assign {
                    rhs: Expr::BinOp { op: BinOp::Sub | BinOp::And, .. }, ..
                })
            });

            if let Some(Stmt::Assign { rhs: Expr::BinOp { lhs: l, rhs: r, .. }, .. }) = cmp {
                let lhs_r = resolve(l, copies);
                let rhs_r = resolve(r, copies);
                let (rel, is_signed) = branch_mnem_to_rel(&cond.branch_mnem);
                // For signed branches, cast operands to their signed equivalents
                // so C performs a signed comparison even if the declared type is
                // unsigned.  When the lifter has already stamped SInt (e.g. after
                // movsx/imul), signed_cast is a no-op.
                let lhs_str = if is_signed { signed_cast(lhs_r) } else { lhs_r.display() };
                let rhs_str = if is_signed { signed_cast(rhs_r) } else { rhs_r.display() };
                return if cond.negate {
                    format!("!({lhs_str} {rel} {rhs_str})")
                } else {
                    format!("{lhs_str} {rel} {rhs_str}")
                };
            }
        }

        format!("cond_{:x}", cond.block_addr)
    }

    // ── Expression emitters ───────────────────────────────────────────────────

    fn emit_expr_resolved(
        &self,
        expr:    &Expr,
        copies:  &HashMap<u32, Value>,
        written: &HashSet<u32>,
        _slots:  &std::collections::HashMap<i64, rustdec_ir::StackSlot>,
    ) -> String {
        match expr {
            Expr::Value(v) => resolve(v, copies).display(),

            Expr::BinOp { op, lhs, rhs } => {
                let l = resolve(lhs, copies);
                let r = resolve(rhs, copies);
                format!("({} {} {})", l.display(), binop_c(op), r.display())
            }

            Expr::Load { ptr, ty } => {
                let p = display_value(ptr, copies, _slots);
                // If the ptr is a slot reference, emit as named variable access.
                if let Value::Var { id, .. } = resolve(ptr, copies) {
                    if is_slot_id(*id) {
                        let offset = slot_id_to_offset(*id);
                        if let Some(slot) = _slots.get(&offset) {
                            return slot.name.clone();
                        }
                    }
                }
                format!("*({}*){}", self.emit_type(ty), p)
            }

            Expr::Call { target, args, .. } => {
                let tgt = match target {
                    CallTarget::Direct(a)   => format!("sub_{a:x}"),
                    CallTarget::Named(n)    => n.clone(),
                    CallTarget::Indirect(v) => {
                        let r = resolve(v, copies);
                        warn!(ptr = %r.display(), "C: indirect call");
                        format!("(*(void*(*)(...))({}))", r.display())
                    }
                };
                // P2 — use-def argument filtering.
                //
                // The lifter always passes the 6 SysV ABI registers (rdi, rsi,
                // rdx, rcx, r8, r9) as arguments to every call, regardless of
                // whether the callee actually uses them.  After copy-propagation
                // these resolve to their SSA IDs at the point of the call.
                //
                // An argument is considered "real" (i.e. intentionally set before
                // this call) if, after resolution, it refers to a variable that
                // was written somewhere in the current function.  Arguments that
                // resolve to a Const are always real.  Arguments that resolve to
                // a Var whose ID was never written in this function are ABI
                // placeholders and are suppressed.
                let filtered: Vec<String> = args
                    .iter()
                    .filter_map(|a| {
                        let r = resolve(a, copies);
                        match r {
                            Value::Const { .. } => Some(r.display()),
                            Value::Var { id, .. } => {
                                if written.contains(id) {
                                    Some(r.display())
                                } else {
                                    // Unwritten ABI register — suppress.
                                    None
                                }
                            }
                        }
                    })
                    .collect();
                format!("{tgt}({})", filtered.join(", "))
            }

            Expr::Cast { val, to } => {
                let v = resolve(val, copies);
                format!("({}){}", self.emit_type(to), v.display())
            }

            Expr::Opaque(s) => {
                // Opaque expressions come from unmodelled instructions or
                // address computations that could not be reduced to IR.
                // Emit as a line comment so the C remains syntactically valid
                // (the surrounding emit_stmt_opt wraps this in a statement).
                format!("/* {s} */")
            }

            Expr::StringRef { addr, content } => {
                // Emit as a C string literal with proper escaping.
                // If the string_table has a higher-fidelity version, use it.
                let text = self.string_table.get(addr).unwrap_or(content);
                format!("\"{}\"", escape_c_string(text))
            }
        }
    }
}

// ── Branch mnemonic → relational operator ────────────────────────────────────

/// Map an x86 branch mnemonic to `(operator, is_signed)`.
///
/// `is_signed` drives the cast in `emit_cond`: signed branches (`jl`, `jle`,
/// `jg`, `jge`) require `(intN_t)` casts on their operands so that C
/// performs a signed comparison even when the variable type is `uintN_t`.
fn branch_mnem_to_rel(mnem: &str) -> (&'static str, bool) {
    match mnem {
        "je"  | "jz"   => ("==", false),
        "jne" | "jnz"  => ("!=", false),
        "jl"  | "jnge" => ("<",  true),  // signed
        "jle" | "jng"  => ("<=", true),  // signed
        "jg"  | "jnle" => (">",  true),  // signed
        "jge" | "jnl"  => (">=", true),  // signed
        "jb"  | "jnae" => ("<",  false), // unsigned
        "jbe" | "jna"  => ("<=", false), // unsigned
        "ja"  | "jnbe" => (">",  false), // unsigned
        "jae" | "jnb"  => (">=", false), // unsigned
        _              => ("!=", false),
    }
}

/// Wrap a value display in a signed cast when the value's own type is unsigned.
///
/// If the value already has a signed type (`SInt`), no cast is emitted —
/// this happens when the lifter has already stamped `SInt` (e.g. after
/// `movsx` or `imul`).
fn signed_cast(v: &Value) -> String {
    let s = v.display();
    match v.ty() {
        IrType::SInt(_)  => s,
        IrType::UInt(8)  => format!("(int8_t){s}"),
        IrType::UInt(16) => format!("(int16_t){s}"),
        IrType::UInt(32) => format!("(int32_t){s}"),
        IrType::UInt(64) => format!("(int64_t){s}"),
        _                => format!("(int64_t){s}"),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn binop_c(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add              => "+",
        BinOp::Sub              => "-",
        BinOp::Mul              => "*",
        BinOp::UDiv | BinOp::SDiv => "/",
        BinOp::URem | BinOp::SRem => "%",
        BinOp::And              => "&",
        BinOp::Or               => "|",
        BinOp::Xor              => "^",
        BinOp::Shl              => "<<",
        BinOp::LShr | BinOp::AShr => ">>",
        BinOp::Eq               => "==",
        BinOp::Ne               => "!=",
        BinOp::Ult | BinOp::Slt => "<",
        BinOp::Ule | BinOp::Sle => "<=",
    }
}

/// Escape a string for use as a C string literal.
fn escape_c_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"'  => out.push_str("\\\""),
            '\\'  => out.push_str("\\\\"),
            '\n'  => out.push_str("\\n"),
            '\r'  => out.push_str("\\r"),
            '\t'  => out.push_str("\\t"),
            '\0'  => out.push_str("\\0"),
            c if c.is_ascii_graphic() || c == ' ' => out.push(c),
            c    => { let _ = c; out.push_str("?"); }
        }
    }
    out
}

fn sanitise_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}
