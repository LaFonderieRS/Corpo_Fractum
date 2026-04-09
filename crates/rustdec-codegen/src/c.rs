//! C99 pseudo-code backend — structured output using if/while/for.

use std::collections::{HashMap, HashSet};

use rustdec_analysis::{structure_function, CondExpr, SNode};
use rustdec_lift::frame::{is_slot_id, slot_id_to_offset};
use rustdec_ir::{BinOp, BasicBlock, CallTarget, Expr, IrFunction, IrType, Stmt, Terminator, Value};
use tracing::{debug, trace, warn};

use crate::{CodegenBackend, CodegenResult};

// ── Convenience type aliases used throughout this module ──────────────────────

type SlotMap  = std::collections::HashMap<i64, rustdec_ir::StackSlot>;
type RegNames = HashMap<u32, String>;

// ── Backend struct ────────────────────────────────────────────────────────────

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

        // ── Pass 1: collect variables, copy table, written-var set ────────────
        let mut var_decls:    HashMap<u32, IrType> = HashMap::new();
        let mut assign_count: HashMap<u32, usize>  = HashMap::new();
        let mut copy_table:   HashMap<u32, Value>  = HashMap::new();
        let mut written_vars: HashSet<u32>          = HashSet::new();

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

        copy_table.retain(|id, _| assign_count.get(id).copied().unwrap_or(0) == 1);
        let copy_table  = flatten_copies(copy_table);
        let suppressed: HashSet<u32> = copy_table.keys().copied().collect();

        // ── Emit function header ──────────────────────────────────────────────
        let mut out = String::new();
        out.push_str(&format!("// RustDec decompilation — {}\n", func.name));
        out.push_str(&format!("{ret} {}({params}) {{\n", sanitise_name(&func.name)));

        // ── Emit variable declarations (hoisted) ──────────────────────────────
        let mut decl_lines: Vec<String> = var_decls
            .iter()
            .filter(|(id, _)| {
                !suppressed.contains(id)
                    && !is_slot_id(**id)
                    && !func.reg_names.contains_key(id)
            })
            .map(|(id, ty)| format!("  {} v{id};", self.emit_type(ty)))
            .collect();
        decl_lines.sort();
        if !decl_lines.is_empty() {
            for line in &decl_lines { out.push_str(line); out.push('\n'); }
            out.push('\n');
        }

        // ── Emit structured body ──────────────────────────────────────────────
        let structured = structure_function(func);
        let slots     = &func.slot_table;
        let reg_names = &func.reg_names;
        self.emit_node(&structured.root, &structured, &copy_table,
                       &written_vars, slots, reg_names, &mut out, 1);

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
            other => { warn!(ty = ?other, "C: unknown type"); "/* unknown */".into() }
        }
    }
}

// ── Copy-propagation helpers ──────────────────────────────────────────────────

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

fn resolve<'a>(v: &'a Value, copies: &'a HashMap<u32, Value>) -> &'a Value {
    if let Value::Var { id, .. } = v {
        if let Some(subst) = copies.get(id) {
            return resolve(subst, copies);
        }
    }
    v
}

// ── Value display ─────────────────────────────────────────────────────────────

/// Display a `Value` with slot and register-name substitutions.
fn display_value(
    v:         &Value,
    copies:    &HashMap<u32, Value>,
    slots:     &SlotMap,
    reg_names: &RegNames,
) -> String {
    let resolved = resolve(v, copies);
    if let Value::Var { id, .. } = resolved {
        if is_slot_id(*id) {
            let offset = slot_id_to_offset(*id);
            if let Some(slot) = slots.get(&offset) {
                return format!("&{}", slot.name);
            }
        }
        if let Some(name) = reg_names.get(id) {
            return name.to_string();
        }
    }
    resolved.display()
}

// ── Structured tree emitter ───────────────────────────────────────────────────

impl CBackend {
    fn indent(depth: usize) -> String { "  ".repeat(depth) }

    fn emit_node(
        &self,
        node:      &SNode,
        sfunc:     &rustdec_analysis::StructuredFunc,
        copies:    &HashMap<u32, Value>,
        written:   &HashSet<u32>,
        slots:     &SlotMap,
        reg_names: &RegNames,
        out:       &mut String,
        depth:     usize,
    ) {
        let ind = Self::indent(depth);
        match node {
            SNode::Block(id) => {
                if let Some(bb) = sfunc.blocks.get(id) {
                    trace!(block = format_args!("{:#x}", bb.start_addr),
                           stmts = bb.stmts.len(), "C: emit block");
                    for stmt in &bb.stmts {
                        if let Some(line) = self.emit_stmt_opt(
                            stmt, copies, written, slots, reg_names)
                        {
                            out.push_str(&format!("{ind}{line};\n"));
                        }
                    }
                    match &bb.terminator {
                        Terminator::Return(Some(v)) => {
                            let r = resolve(v, copies);
                            let s = display_value(r, copies, slots, reg_names);
                            out.push_str(&format!("{ind}return {s};\n"));
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
                    self.emit_node(n, sfunc, copies, written, slots, reg_names, out, depth);
                }
            }

            SNode::IfElse { cond, then, else_ } => {
                let cond_str = self.emit_cond(cond, sfunc, copies, written, slots, reg_names);
                out.push_str(&format!("{ind}if ({cond_str}) {{\n"));
                self.emit_node(then, sfunc, copies, written, slots, reg_names, out, depth + 1);
                let mut else_buf = String::new();
                self.emit_node(else_, sfunc, copies, written, slots, reg_names,
                               &mut else_buf, depth + 1);
                if !else_buf.trim().is_empty() {
                    out.push_str(&format!("{ind}}} else {{\n"));
                    out.push_str(&else_buf);
                }
                out.push_str(&format!("{ind}}}\n"));
            }

            SNode::Loop { cond, body } => {
                let cond_str = self.emit_cond(cond, sfunc, copies, written, slots, reg_names);
                out.push_str(&format!("{ind}while ({cond_str}) {{\n"));
                self.emit_node(body, sfunc, copies, written, slots, reg_names, out, depth + 1);
                out.push_str(&format!("{ind}}}\n"));
            }

            SNode::Break    => out.push_str(&format!("{ind}break;\n")),
            SNode::Continue => out.push_str(&format!("{ind}continue;\n")),
        }
    }

    fn emit_stmt_opt(
        &self,
        stmt:      &Stmt,
        copies:    &HashMap<u32, Value>,
        written:   &HashSet<u32>,
        slots:     &SlotMap,
        reg_names: &RegNames,
    ) -> Option<String> {
        match stmt {
            Stmt::Nop => None,

            Stmt::Assign { lhs, ty: _, rhs } => {
                if copies.contains_key(lhs) { return None; }
                if is_slot_id(*lhs)          { return None; }
                // Suppress assignments to ABI seed registers that were never
                // written by the function body (they are implicit inputs).
                if reg_names.contains_key(lhs) { return None; }

                let rhs_str = self.emit_expr_resolved(rhs, copies, written, slots, reg_names);
                Some(format!("v{lhs} = {rhs_str}"))
            }

            Stmt::Store { ptr, val } => {
                let ptr_r = resolve(ptr, copies);
                let val_r = resolve(val, copies);

                // Suppress stores through a known-null pointer.
                if matches!(ptr_r, Value::Const { val: 0, .. }) {
                    return Some("/* NULL store suppressed */".to_string());
                }

                // Slot store → named assignment.
                if let Value::Var { id, .. } = ptr_r {
                    if is_slot_id(*id) {
                        let offset = slot_id_to_offset(*id);
                        if let Some(slot) = slots.get(&offset) {
                            return Some(format!("{} = {}", slot.name, val_r.display()));
                        }
                    }
                }
                let ptr_s = display_value(ptr_r, copies, slots, reg_names);
                let val_s = display_value(val_r, copies, slots, reg_names);
                Some(format!("*({ptr_s}) = {val_s}"))
            }
        }
    }

    fn emit_cond(
        &self,
        cond:      &CondExpr,
        sfunc:     &rustdec_analysis::StructuredFunc,
        copies:    &HashMap<u32, Value>,
        _written:  &HashSet<u32>,
        slots:     &SlotMap,
        reg_names: &RegNames,
    ) -> String {
        let bb = sfunc.blocks.values()
            .find(|b| b.start_addr == cond.block_addr);

        if let Some(bb) = bb {
            let cmp = bb.stmts.iter().rev().find(|s| {
                matches!(s, Stmt::Assign {
                    rhs: Expr::BinOp { op: BinOp::Sub | BinOp::And, .. }, ..
                })
            });

            if let Some(Stmt::Assign { rhs: Expr::BinOp { lhs: l, rhs: r, .. }, .. }) = cmp {
                let lhs_r = resolve(l, copies);
                let rhs_r = resolve(r, copies);
                let (rel, is_signed) = branch_mnem_to_rel(&cond.branch_mnem);
                let lhs_s = if is_signed { signed_cast(lhs_r) }
                            else { display_value(lhs_r, copies, slots, reg_names) };
                let rhs_s = if is_signed { signed_cast(rhs_r) }
                            else { display_value(rhs_r, copies, slots, reg_names) };
                return if cond.negate {
                    format!("!({lhs_s} {rel} {rhs_s})")
                } else {
                    format!("{lhs_s} {rel} {rhs_s}")
                };
            }
        }

        format!("cond_{:x}", cond.block_addr)
    }

    fn emit_expr_resolved(
        &self,
        expr:      &Expr,
        copies:    &HashMap<u32, Value>,
        written:   &HashSet<u32>,
        slots:     &SlotMap,
        reg_names: &RegNames,
    ) -> String {
        match expr {
            Expr::Value(v) => display_value(v, copies, slots, reg_names),

            Expr::BinOp { op, lhs, rhs } => {
                let l = display_value(lhs, copies, slots, reg_names);
                let r = display_value(rhs, copies, slots, reg_names);
                format!("({l} {} {r})", binop_c(op))
            }

            Expr::Load { ptr, ty } => {
                if let Value::Var { id, .. } = resolve(ptr, copies) {
                    if is_slot_id(*id) {
                        let offset = slot_id_to_offset(*id);
                        if let Some(slot) = slots.get(&offset) {
                            return slot.name.clone();
                        }
                    }
                }
                let p = display_value(ptr, copies, slots, reg_names);
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
                let filtered: Vec<String> = args
                    .iter()
                    .filter_map(|a| {
                        let r = resolve(a, copies);
                        match r {
                            Value::Const { val, .. } => {
                                // Check if this constant is a string address.
                                if let Some(text) = self.string_table.get(val) {
                                    Some(format!("\"{}\"", escape_c_string(text)))
                                } else {
                                    Some(display_value(r, copies, slots, reg_names))
                                }
                            }
                            Value::Var { id, .. } => {
                                if written.contains(id) {
                                    Some(display_value(r, copies, slots, reg_names))
                                } else {
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
                format!("({}){}", self.emit_type(to), display_value(v, copies, slots, reg_names))
            }

            Expr::Opaque(s) => format!("/* {s} */"),

            Expr::StringRef { addr, content } => {
                let text = self.string_table.get(addr).unwrap_or(content);
                format!("\"{}\"", escape_c_string(text))
            }
        }
    }
}

// ── Branch mnemonic → relational operator ────────────────────────────────────

fn branch_mnem_to_rel(mnem: &str) -> (&'static str, bool) {
    match mnem {
        "je"  | "jz"   => ("==", false),
        "jne" | "jnz"  => ("!=", false),
        "jl"  | "jnge" => ("<",  true),
        "jle" | "jng"  => ("<=", true),
        "jg"  | "jnle" => (">",  true),
        "jge" | "jnl"  => (">=", true),
        "jb"  | "jnae" => ("<",  false),
        "jbe" | "jna"  => ("<=", false),
        "ja"  | "jnbe" => (">",  false),
        "jae" | "jnb"  => (">=", false),
        _              => ("!=", false),
    }
}

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

fn escape_c_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"'  => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if c.is_ascii_graphic() || c == ' ' => out.push(c),
            _    => out.push('?'),
        }
    }
    out
}

fn sanitise_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}
