//! C99 pseudo-code backend.
//!
//! Produces readable C intended for human analysis — not guaranteed to compile.
//! Unknown types render as `/* unknown */`.
//!
//! # Error strategy
//!
//! `emit_function` is the only method that can fail (required by the
//! [`CodegenBackend`] trait).  The private helpers `emit_stmt`,
//! `emit_expr` and `emit_terminator` always succeed and return `String`
//! directly — no `Result` wrapping, no spurious `?` operators.

use rustdec_ir::{BinOp, CallTarget, Expr, IrFunction, IrType, Stmt, Terminator};
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
            func.params
                .iter()
                .enumerate()
                .map(|(i, ty)| format!("{} a{i}", self.emit_type(ty)))
                .collect::<Vec<_>>()
                .join(", ")
        };

        debug!(func = %func.name, ret = %ret, params = func.params.len(), "C: emitting function");

        let mut out = String::new();
        out.push_str(&format!("// RustDec decompilation — {}\n", func.name));
        out.push_str(&format!("{ret} {}({params}) {{\n", sanitise_name(&func.name)));

        let blocks = func.blocks_sorted();

        if blocks.is_empty() {
            warn!(func = %func.name, "C: function has no basic blocks");
            out.push_str("  /* no blocks decoded */\n");
        }

        for bb in &blocks {
            trace!(
                func  = %func.name,
                block = format_args!("{:#x}", bb.start_addr),
                stmts = bb.stmts.len(),
                term  = ?bb.terminator,
                "C: emitting block"
            );
            out.push_str(&format!("  // block_{:x}:\n", bb.start_addr));
            for stmt in &bb.stmts {
                let s = self.emit_stmt(stmt);
                trace!(func = %func.name, stmt = %s, "C: stmt");
                out.push_str(&format!("  {s};\n"));
            }
            let term = self.emit_terminator(&bb.terminator);
            out.push_str(&format!("  {term};\n\n"));
        }

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
                warn!(ty = ?other, "C: unknown type — falling back to comment");
                "/* unknown */".into()
            }
        }
    }
}

// ── Private helpers — infallible ──────────────────────────────────────────────

impl CBackend {
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
                        warn!(ptr = %v.display(), "C: indirect call — emitting cast");
                        format!("(*(void*(*)(...))({}))", v.display())
                    }
                };
                let args_str = args.iter().map(|a| a.display()).collect::<Vec<_>>().join(", ");
                format!("{tgt}({args_str})")
            }
            Expr::Cast { val, to } => {
                format!("({}){}", self.emit_type(to), val.display())
            }
            Expr::Opaque(s) => {
                warn!(expr = %s, "C: opaque expression — emitting as comment");
                format!("/* {s} */")
            }
        }
    }

    fn emit_terminator(&self, term: &Terminator) -> String {
        match term {
            Terminator::Return(Some(v)) => format!("return {}", v.display()),
            Terminator::Return(None)    => "return".into(),
            Terminator::Jump(id)        => format!("goto block_{id:x}"),
            Terminator::Branch { cond, true_bb, false_bb } => format!(
                "if ({}) {{ goto block_{true_bb:x}; }} else {{ goto block_{false_bb:x}; }}",
                cond.display()
            ),
            Terminator::Unreachable => "/* unreachable */".into(),
        }
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
