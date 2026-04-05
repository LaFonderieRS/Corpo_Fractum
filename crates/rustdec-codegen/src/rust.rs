//! Rust pseudo-code backend.
//!
//! Emits idiomatic-ish Rust: `fn`, `let`, `u64`, raw pointers.
//! All pointer dereferences are wrapped in `unsafe`.

use rustdec_ir::{BinOp, CallTarget, Expr, IrFunction, IrType, Stmt, Terminator};
use tracing::{debug, trace, warn};

use crate::{CodegenBackend, CodegenResult};

pub struct RustBackend;

impl CodegenBackend for RustBackend {
    fn emit_function(&self, func: &IrFunction) -> CodegenResult<String> {
        let ret    = if func.ret_ty == IrType::Void {
            String::new()
        } else {
            format!(" -> {}", self.emit_type(&func.ret_ty))
        };
        let params = func.params.iter().enumerate()
            .map(|(i, ty)| format!("a{i}: {}", self.emit_type(ty)))
            .collect::<Vec<_>>()
            .join(", ");

        debug!(func = %func.name, ret = %ret, params = func.params.len(), "Rust: emitting function");

        let fn_name = sanitise_rust(&func.name);
        let mut out = String::new();
        out.push_str(&format!("// RustDec decompilation — {}\n", func.name));
        out.push_str("#[allow(unused_variables, unused_assignments, non_snake_case)]\n");
        out.push_str(&format!("unsafe fn {fn_name}({params}){ret} {{\n"));

        let blocks = func.blocks_sorted();

        if blocks.is_empty() {
            warn!(func = %func.name, "Rust: function has no basic blocks");
            out.push_str("    // no blocks decoded\n");
            out.push_str("    todo!()\n");
        }

        for bb in &blocks {
            trace!(
                func  = %func.name,
                block = format_args!("{:#x}", bb.start_addr),
                stmts = bb.stmts.len(),
                "Rust: emitting block"
            );
            out.push_str(&format!("    // block_{:x}\n", bb.start_addr));
            for stmt in &bb.stmts {
                let s = self.emit_stmt(stmt);
                trace!(func = %func.name, stmt = %s, "Rust: stmt");
                out.push_str(&format!("    {s};\n"));
            }
            let term = self.emit_terminator(&bb.terminator);
            out.push_str(&format!("    {term};\n\n"));
        }

        out.push_str("}\n");
        debug!(func = %func.name, lines = out.lines().count(), "Rust: function emitted");
        Ok(out)
    }

    fn emit_type(&self, ty: &IrType) -> String {
        match ty {
            IrType::UInt(8)             => "u8".into(),
            IrType::UInt(16)            => "u16".into(),
            IrType::UInt(32)            => "u32".into(),
            IrType::UInt(64)            => "u64".into(),
            IrType::SInt(8)             => "i8".into(),
            IrType::SInt(16)            => "i16".into(),
            IrType::SInt(32)            => "i32".into(),
            IrType::SInt(64)            => "i64".into(),
            IrType::Float(32)           => "f32".into(),
            IrType::Float(64)           => "f64".into(),
            IrType::Ptr(inner)          => format!("*mut {}", self.emit_type(inner)),
            IrType::Array { elem, len } => format!("[{}; {len}]", self.emit_type(elem)),
            IrType::Struct { name, .. } => sanitise_rust(name),
            IrType::Void                => "()".into(),
            other => {
                warn!(ty = ?other, "Rust: unknown type — falling back to comment");
                "/* unknown */".into()
            }
        }
    }
}

impl RustBackend {
    fn emit_stmt(&self, stmt: &Stmt) -> String {
        match stmt {
            Stmt::Assign { lhs, ty, rhs } => {
                format!("let mut v{lhs}: {} = {}", self.emit_type(ty), self.emit_expr(rhs))
            }
            Stmt::Store { ptr, val } => {
                format!("*{} = {}", ptr.display(), val.display())
            }
            Stmt::Nop => "// nop".into(),
        }
    }

    fn emit_expr(&self, expr: &Expr) -> String {
        match expr {
            Expr::Value(v) => v.display(),
            Expr::BinOp { op, lhs, rhs } => {
                format!("({}).{}({})", lhs.display(), binop_rust(op), rhs.display())
            }
            Expr::Load { ptr, ty } => {
                format!("*({} as *const {})", ptr.display(), self.emit_type(ty))
            }
            Expr::Call { target, args, .. } => {
                let tgt = match target {
                    CallTarget::Direct(a)   => format!("sub_{a:x}"),
                    CallTarget::Named(n)    => sanitise_rust(n),
                    CallTarget::Indirect(v) => {
                        warn!(ptr = %v.display(), "Rust: indirect call — emitting transmute");
                        format!("(std::mem::transmute::<_, fn()>({}))", v.display())
                    }
                };
                let args_str = args.iter().map(|a| a.display()).collect::<Vec<_>>().join(", ");
                format!("{tgt}({args_str})")
            }
            Expr::Cast { val, to } => {
                format!("{} as {}", val.display(), self.emit_type(to))
            }
            Expr::Opaque(s) => {
                warn!(expr = %s, "Rust: opaque expression");
                format!("/* {s} */ todo!()")
            }
        }
    }

    fn emit_terminator(&self, term: &Terminator) -> String {
        match term {
            Terminator::Return(Some(v)) => format!("return {}", v.display()),
            Terminator::Return(None)    => "return".into(),
            Terminator::Jump(id)        => format!("// goto block_{id:x}"),
            Terminator::Branch { cond, true_bb, false_bb } => format!(
                "if {} != 0 {{ /* goto block_{true_bb:x} */ }} else {{ /* goto block_{false_bb:x} */ }}",
                cond.display()
            ),
            Terminator::Unreachable => "unreachable!()".into(),
        }
    }
}

fn binop_rust(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add              => "wrapping_add",
        BinOp::Sub              => "wrapping_sub",
        BinOp::Mul              => "wrapping_mul",
        BinOp::UDiv | BinOp::SDiv => "wrapping_div",
        BinOp::URem | BinOp::SRem => "wrapping_rem",
        BinOp::And              => "bitand",
        BinOp::Or               => "bitor",
        BinOp::Xor              => "bitxor",
        BinOp::Shl              => "wrapping_shl",
        BinOp::LShr | BinOp::AShr => "wrapping_shr",
        BinOp::Eq               => "eq",
        BinOp::Ne               => "ne",
        BinOp::Ult | BinOp::Slt => "lt",
        BinOp::Ule | BinOp::Sle => "le",
    }
}

fn sanitise_rust(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}
