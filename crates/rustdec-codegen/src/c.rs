//! C99 pseudo-code backend — structured output using if/while/for.
//!
//! Uses [`StructuredFunc`] from `rustdec-analysis` to emit clean structured
//! code instead of flat goto-based output.

use rustdec_analysis::{structure_function, CondExpr, SNode};
use rustdec_ir::{BinOp, CallTarget, Expr, IrFunction, IrType, Stmt, Terminator, Value};
use tracing::{debug, trace, warn};

use crate::{CodegenBackend, CodegenResult};

pub struct CBackend;

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

        debug!(func = %func.name, ret = %ret, params = func.params.len(), "C: emitting function");

        let mut out = String::new();
        out.push_str(&format!("// RustDec decompilation — {}\n", func.name));
        out.push_str(&format!("{ret} {}({params}) {{\n", sanitise_name(&func.name)));

        // Build structured tree.
        let structured = structure_function(func);

        // Emit structured tree.
        self.emit_node(&structured.root, &structured, &mut out, 1);

        out.push_str("}\n");
        debug!(func = %func.name, lines = out.lines().count(), "C: function emitted");
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

// ── Structured tree emitter ───────────────────────────────────────────────────

impl CBackend {
    fn indent(depth: usize) -> String {
        "  ".repeat(depth)
    }

    fn emit_node(
        &self,
        node:       &SNode,
        sfunc:      &rustdec_analysis::StructuredFunc,
        out:        &mut String,
        depth:      usize,
    ) {
        let ind = Self::indent(depth);
        match node {
            SNode::Block(id) => {
                if let Some(bb) = sfunc.blocks.get(id) {
                    trace!(block = format_args!("{:#x}", bb.start_addr), stmts = bb.stmts.len(), "C: emit block");
                    for stmt in &bb.stmts {
                        let s = self.emit_stmt(stmt);
                        if s != "/* nop */" {
                            out.push_str(&format!("{ind}{};\n", s));
                        }
                    }
                    // Emit return if this block terminates the function.
                    if let Terminator::Return(val) = &bb.terminator {
                        if let Some(v) = val {
                            out.push_str(&format!("{ind}return {};\n", v.display()));
                        } else if !matches!(sfunc.blocks.values()
                            .filter(|b| matches!(b.terminator, Terminator::Return(_)))
                            .count(), 0)
                        {
                            // Only emit bare return if this is a void function.
                            // For non-void, the rax assignment above already carries the value.
                        }
                    }
                }
            }

            SNode::Seq(nodes) => {
                for n in nodes {
                    self.emit_node(n, sfunc, out, depth);
                }
            }

            SNode::IfElse { cond, then, else_ } => {
                let cond_str = self.emit_cond(cond, sfunc);
                out.push_str(&format!("{ind}if ({cond_str}) {{\n"));
                self.emit_node(then, sfunc, out, depth + 1);
                // Only emit else if non-empty.
                let mut else_buf = String::new();
                self.emit_node(else_, sfunc, &mut else_buf, depth + 1);
                if !else_buf.trim().is_empty() {
                    out.push_str(&format!("{ind}}} else {{\n"));
                    out.push_str(&else_buf);
                }
                out.push_str(&format!("{ind}}}\n"));
            }

            SNode::Loop { cond, body } => {
                let cond_str = self.emit_cond(cond, sfunc);
                out.push_str(&format!("{ind}while ({cond_str}) {{\n"));
                self.emit_node(body, sfunc, out, depth + 1);
                out.push_str(&format!("{ind}}}\n"));
            }

            SNode::Break    => out.push_str(&format!("{ind}break;\n")),
            SNode::Continue => out.push_str(&format!("{ind}continue;\n")),
        }
    }

    /// Emit a human-readable condition string for a Branch block.
    ///
    /// We look up the last `cmp`/`test` stmt in the block and combine it
    /// with the branch mnemonic to produce e.g. `v3 != 0` or `v1 < v2`.
    fn emit_cond(&self, cond: &CondExpr, sfunc: &rustdec_analysis::StructuredFunc) -> String {
        // Find the block by address.
        let bb = sfunc.blocks.values()
            .find(|b| b.start_addr == cond.block_addr);

        if let Some(bb) = bb {
            // Find last cmp/test assignment.
            let cmp_stmt = bb.stmts.iter().rev().find(|s| {
                matches!(s, Stmt::Assign { rhs: Expr::BinOp { op: BinOp::Sub | BinOp::And, .. }, .. })
            });

            if let Some(Stmt::Assign { lhs, rhs: Expr::BinOp { lhs: l, rhs: r, op }, .. }) = cmp_stmt {
                let lhs_s = l.display();
                let rhs_s = r.display();
                let (rel, negate) = branch_mnem_to_rel(&bb, &cond.branch_mnem);
                let negated = if cond.negate { !negate } else { negate };
                return if negated {
                    format!("!({lhs_s} {rel} {rhs_s})")
                } else {
                    format!("{lhs_s} {rel} {rhs_s}")
                };
            }
        }

        // Fallback: emit opaque condition referencing the block address.
        format!("cond_{:x}", cond.block_addr)
    }

    // ── Statement / expression emitters (unchanged from before) ──────────────

    fn emit_stmt(&self, stmt: &Stmt) -> String {
        match stmt {
            Stmt::Assign { lhs, ty, rhs } => {
                format!("{} v{lhs} = {}", self.emit_type(ty), self.emit_expr(rhs))
            }
            Stmt::Store { ptr, val } => {
                format!("*({}) = {}", ptr.display(), val.display())
            }
            Stmt::Nop => "/* nop */".into(),
        }
    }

    fn emit_expr(&self, expr: &Expr) -> String {
        match expr {
            Expr::Value(v) => v.display(),
            Expr::BinOp { op, lhs, rhs } => {
                format!("({} {} {})", lhs.display(), binop_c(op), rhs.display())
            }
            Expr::Load { ptr, ty } => {
                format!("*({}*){}", self.emit_type(ty), ptr.display())
            }
            Expr::Call { target, args, .. } => {
                let tgt = match target {
                    CallTarget::Direct(a)   => format!("sub_{a:x}"),
                    CallTarget::Named(n)    => n.clone(),
                    CallTarget::Indirect(v) => {
                        warn!(ptr = %v.display(), "C: indirect call");
                        format!("(*(void*(*)(...))({}))", v.display())
                    }
                };
                // Filter out repeated rdi/rsi/rdx placeholder args — only emit
                // non-trivial args (non-zero or non-register vars with id < 8).
                let args_str = args.iter()
                    .filter(|a| !matches!(a, Value::Const { val: 0, .. }))
                    .map(|a| a.display())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{tgt}({args_str})")
            }
            Expr::Cast { val, to } => {
                format!("({}){}", self.emit_type(to), val.display())
            }
            Expr::Opaque(s) => s.clone(),
        }
    }
}

// ── Branch mnemonic → relational operator ────────────────────────────────────

/// Return `(operator_str, negate)` for a branch mnemonic.
/// `negate=true` means the condition in the `if` should be inverted.
fn branch_mnem_to_rel(bb: &rustdec_ir::BasicBlock, mnem: &str) -> (&'static str, bool) {
    // The branch mnem is the last branch instruction in the block.
    // We peek at the terminator to recover the original mnemonic.
    match mnem {
        "je"  | "jz"  => ("==", false),
        "jne" | "jnz" => ("!=", false),
        "jl"  | "jnge"=> ("<",  false),
        "jle" | "jng" => ("<=", false),
        "jg"  | "jnle"=> (">",  false),
        "jge" | "jnl" => (">=", false),
        "jb"  | "jnae"=> ("<",  false), // unsigned
        "jbe" | "jna" => ("<=", false),
        "ja"  | "jnbe"=> (">",  false),
        "jae" | "jnb" => (">=", false),
        _             => ("!=", false), // safe default
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

fn sanitise_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}
