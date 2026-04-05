//! x86-64 instruction lifter.
//!
//! Each instruction is mapped to zero or more IR [`Stmt`]s.
//! We model the most common patterns; unknown instructions become `Stmt::Nop`
//! with a `warn!` so the output is always complete even for unrecognised opcodes.

use rustdec_disasm::Instruction;
use rustdec_ir::{BinOp, CallTarget, Expr, IrType, Stmt, Value};
use tracing::{trace, warn};

/// Lift a slice of instructions (one basic block) into IR statements.
/// `next_id` is the SSA variable counter shared across the function.
pub fn lift_block(insns: &[&Instruction], next_id: &mut u32) -> Vec<Stmt> {
    let mut stmts = Vec::new();
    for insn in insns {
        let new = lift_insn(insn, next_id);
        trace!(
            at    = format_args!("{:#x}", insn.address),
            mnem  = %insn.mnemonic,
            ops   = %insn.operands,
            stmts = new.len(),
            "lifted"
        );
        stmts.extend(new);
    }
    stmts
}

// ── Operand helpers ───────────────────────────────────────────────────────────

/// Classify the width of a register or memory reference from its name.
fn reg_type(reg: &str) -> IrType {
    let r = reg.trim_start_matches('%');
    // 64-bit
    if r.starts_with('r') || matches!(r, "rsp" | "rbp" | "rip") {
        return IrType::UInt(64);
    }
    // 32-bit
    if r.starts_with('e') || matches!(r, "esp" | "ebp") {
        return IrType::UInt(32);
    }
    // 16-bit
    if matches!(r, "ax"|"bx"|"cx"|"dx"|"sp"|"bp"|"si"|"di") {
        return IrType::UInt(16);
    }
    // 8-bit (al, ah, bl, …)
    if r.ends_with('l') || r.ends_with('h') || r.ends_with("b") {
        return IrType::UInt(8);
    }
    IrType::UInt(64) // default
}

/// True if the operand string looks like a memory reference.
fn is_mem(op: &str) -> bool {
    op.contains('[') || op.contains("ptr")
}

/// True if the operand looks like an immediate constant.
fn is_imm(op: &str) -> bool {
    let op = op.trim();
    op.starts_with("0x") || op.starts_with('-')
        || op.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)
}

/// Parse an immediate value from an operand token.
fn parse_imm(op: &str) -> u64 {
    let op = op.trim().trim_start_matches("0x").trim_start_matches("0X");
    u64::from_str_radix(op, 16)
        .or_else(|_| op.parse::<u64>())
        .unwrap_or(0)
}

/// Build a `Value::Var` for a freshly allocated SSA variable.
fn fresh(next_id: &mut u32, ty: IrType) -> (u32, Value) {
    let id = *next_id;
    *next_id += 1;
    (id, Value::Var { id, ty })
}

/// Build a `Value::Const` from a parsed immediate.
fn imm(val: u64, ty: IrType) -> Value {
    Value::Const { val, ty }
}

/// Build an opaque `Value::Var` representing a named register.
/// We use the register name hashed to a stable id for readability.
/// In a full implementation this would use a proper register file.
fn reg_val(name: &str) -> Value {
    let ty  = reg_type(name);
    // Stable id from register name for display (not true SSA — good enough for MVP).
    let id  = reg_id(name);
    Value::Var { id, ty }
}

/// Assign a stable pseudo-SSA id to well-known x86-64 registers.
fn reg_id(name: &str) -> u32 {
    let r = name.trim_start_matches('%').trim_start_matches('r').trim_start_matches('e');
    match r {
        "ax" | "a"  => 0,
        "bx" | "b"  => 1,
        "cx" | "c"  => 2,
        "dx" | "d"  => 3,
        "sp"        => 4,
        "bp"        => 5,
        "si"        => 6,
        "di"        => 7,
        "8"         => 8,
        "9"         => 9,
        "10"        => 10,
        "11"        => 11,
        "12"        => 12,
        "13"        => 13,
        "14"        => 14,
        "15"        => 15,
        "ip"        => 16,
        _           => 31, // unknown register
    }
}

// ── Per-instruction lifter ────────────────────────────────────────────────────

fn lift_insn(insn: &Instruction, next_id: &mut u32) -> Vec<Stmt> {
    let mnem = insn.mnemonic.as_str();
    let ops  = insn.operands.trim();

    // Split operands on comma (Intel syntax: dst, src).
    let parts: Vec<&str> = ops.splitn(2, ',').map(str::trim).collect();
    let dst = parts.first().copied().unwrap_or("");
    let src = parts.get(1).copied().unwrap_or("");

    match mnem {
        // ── Data movement ────────────────────────────────────────────────────
        "mov" | "movabs" | "movzx" | "movsx" | "movsxd" => {
            lift_mov(dst, src, next_id)
        }

        "lea" => lift_lea(dst, src, next_id),

        "push" => {
            // push src  →  rsp -= 8; *rsp = src
            let val = operand_value(src);
            vec![Stmt::Store {
                ptr: reg_val("rsp"),
                val,
            }]
        }

        "pop" => {
            // pop dst  →  dst = *rsp; rsp += 8
            let ty  = reg_type(dst);
            let ptr = reg_val("rsp");
            let (id, _) = fresh(next_id, ty.clone());
            vec![Stmt::Assign {
                lhs: id,
                ty:  ty.clone(),
                rhs: Expr::Load { ptr, ty },
            }]
        }

        "xchg" => {
            // xchg a, b  — model as two moves via a temp
            let va = operand_value(dst);
            let vb = operand_value(src);
            let ty = IrType::UInt(64);
            let (tmp, _) = fresh(next_id, ty.clone());
            vec![
                Stmt::Assign { lhs: tmp, ty: ty.clone(), rhs: Expr::Value(va.clone()) },
                Stmt::Store  { ptr: va, val: vb.clone() },
                Stmt::Store  { ptr: vb, val: Value::Var { id: tmp, ty } },
            ]
        }

        // ── Arithmetic ───────────────────────────────────────────────────────
        "add" | "sub" | "imul" | "mul"
        | "and" | "or" | "xor"
        | "shl" | "shr" | "sar"
        | "adc" | "sbb" => {
            lift_binop(mnem, dst, src, next_id)
        }

        "inc" => {
            let lhs_val = operand_value(dst);
            let ty = value_type(&lhs_val);
            let (id, _) = fresh(next_id, ty.clone());
            vec![Stmt::Assign {
                lhs: id, ty,
                rhs: Expr::BinOp { op: BinOp::Add, lhs: lhs_val, rhs: imm(1, IrType::UInt(64)) },
            }]
        }

        "dec" => {
            let lhs_val = operand_value(dst);
            let ty = value_type(&lhs_val);
            let (id, _) = fresh(next_id, ty.clone());
            vec![Stmt::Assign {
                lhs: id, ty,
                rhs: Expr::BinOp { op: BinOp::Sub, lhs: lhs_val, rhs: imm(1, IrType::UInt(64)) },
            }]
        }

        "neg" => {
            let lhs_val = operand_value(dst);
            let ty = value_type(&lhs_val);
            let (id, _) = fresh(next_id, ty.clone());
            vec![Stmt::Assign {
                lhs: id, ty: ty.clone(),
                rhs: Expr::BinOp { op: BinOp::Sub, lhs: imm(0, ty), rhs: lhs_val },
            }]
        }

        "not" => {
            let lhs_val = operand_value(dst);
            let ty = value_type(&lhs_val);
            let (id, _) = fresh(next_id, ty.clone());
            // ~x = x XOR 0xFFFF...
            vec![Stmt::Assign {
                lhs: id, ty: ty.clone(),
                rhs: Expr::BinOp { op: BinOp::Xor, lhs: lhs_val, rhs: imm(u64::MAX, ty) },
            }]
        }

        // ── Comparisons (affect flags — we emit a comparison expr) ────────────
        "cmp" | "test" => {
            // cmp a, b  →  v_cmp = a - b  (flags implicit, used by next branch)
            let lhs_val = operand_value(dst);
            let rhs_val = operand_value(src);
            let ty = value_type(&lhs_val);
            let op = if mnem == "test" { BinOp::And } else { BinOp::Sub };
            let (id, _) = fresh(next_id, ty.clone());
            vec![Stmt::Assign {
                lhs: id, ty,
                rhs: Expr::BinOp { op, lhs: lhs_val, rhs: rhs_val },
            }]
        }

        // ── Calls ─────────────────────────────────────────────────────────────
        "call" | "lcall" => {
            lift_call(ops, next_id)
        }

        // ── String ops (simplified) ───────────────────────────────────────────
        "rep" | "repz" | "repnz" | "repe" | "repne" => {
            // Prefix — the actual op is in operands; model as opaque.
            let (id, _) = fresh(next_id, IrType::Void);
            vec![Stmt::Assign {
                lhs: id,
                ty:  IrType::Void,
                rhs: Expr::Opaque(format!("{mnem} {ops}")),
            }]
        }

        "stosb" | "stosd" | "stosq" | "movsb" | "movsd" | "movsq"
        | "scasb" | "scasq" | "lodsb" | "lodsq" => {
            let (id, _) = fresh(next_id, IrType::Void);
            vec![Stmt::Assign {
                lhs: id, ty: IrType::Void,
                rhs: Expr::Opaque(format!("{mnem} {ops}")),
            }]
        }

        // ── No-ops / bookkeeping ──────────────────────────────────────────────
        "nop" | "endbr64" | "endbr32" | "data16" => {
            vec![Stmt::Nop]
        }

        // ── Leave / enter ─────────────────────────────────────────────────────
        "leave" => {
            // leave  ≡  mov rsp, rbp; pop rbp
            vec![
                Stmt::Assign {
                    lhs: reg_id("rsp"),
                    ty:  IrType::UInt(64),
                    rhs: Expr::Value(reg_val("rbp")),
                },
                Stmt::Assign {
                    lhs: reg_id("rbp"),
                    ty:  IrType::UInt(64),
                    rhs: Expr::Load { ptr: reg_val("rsp"), ty: IrType::UInt(64) },
                },
            ]
        }

        // ── Terminators are handled by the CFG builder — no stmts needed ─────
        "ret" | "retf" | "retn" | "jmp" | "ljmp"
        | "je"  | "jne" | "jz"  | "jnz"
        | "jl"  | "jle" | "jg"  | "jge"
        | "jb"  | "jbe" | "ja"  | "jae"
        | "js"  | "jns" | "jo"  | "jno"
        | "jp"  | "jnp" | "jpe" | "jpo"
        | "jcxz"| "jecxz"| "jrcxz"
        | "hlt" | "ud2" | "int3" => {
            vec![] // terminator — no IR stmts, handled by Terminator field
        }

        // ── Unknown / unhandled ───────────────────────────────────────────────
        other => {
            warn!(mnem = %other, ops = %ops, "unhandled instruction — emitting Opaque");
            let (id, _) = fresh(next_id, IrType::Unknown);
            vec![Stmt::Assign {
                lhs: id,
                ty:  IrType::Unknown,
                rhs: Expr::Opaque(format!("{other} {ops}")),
            }]
        }
    }
}

// ── Sub-lifters ───────────────────────────────────────────────────────────────

fn lift_mov(dst: &str, src: &str, next_id: &mut u32) -> Vec<Stmt> {
    let src_val = operand_value(src);
    let src_ty  = value_type(&src_val);

    if is_mem(dst) {
        // Store to memory.
        let ptr = operand_ptr(dst);
        return vec![Stmt::Store { ptr, val: src_val }];
    }

    let dst_ty = reg_type(dst);
    let rhs = if is_mem(src) {
        Expr::Load { ptr: operand_ptr(src), ty: dst_ty.clone() }
    } else {
        // Cast if sizes differ (e.g. movzx eax, byte ptr [rsp]).
        if src_ty != dst_ty {
            Expr::Cast { val: src_val, to: dst_ty.clone() }
        } else {
            Expr::Value(src_val)
        }
    };

    let id = reg_id(dst);
    vec![Stmt::Assign { lhs: id, ty: dst_ty, rhs }]
}

fn lift_lea(dst: &str, src: &str, next_id: &mut u32) -> Vec<Stmt> {
    // lea dst, [base + offset]  →  dst = &expr
    // We model the address computation as an opaque u64 for now.
    let id = reg_id(dst);
    vec![Stmt::Assign {
        lhs: id,
        ty:  IrType::UInt(64),
        rhs: Expr::Opaque(format!("&{src}")),
    }]
}

fn lift_binop(mnem: &str, dst: &str, src: &str, next_id: &mut u32) -> Vec<Stmt> {
    let op = match mnem {
        "add" | "adc" => BinOp::Add,
        "sub" | "sbb" => BinOp::Sub,
        "imul"| "mul" => BinOp::Mul,
        "and"         => BinOp::And,
        "or"          => BinOp::Or,
        "xor"         => BinOp::Xor,
        "shl"         => BinOp::Shl,
        "shr"         => BinOp::LShr,
        "sar"         => BinOp::AShr,
        _             => BinOp::Add,
    };

    let lhs_val = operand_value(dst);
    let rhs_val = operand_value(src);
    let ty = value_type(&lhs_val);
    let id = reg_id(dst);

    // xor rax, rax  →  rax = 0  (very common idiom — simplify directly)
    if op == BinOp::Xor {
        if let (Value::Var { id: la, .. }, Value::Var { id: ra, .. }) = (&lhs_val, &rhs_val) {
            if la == ra {
                return vec![Stmt::Assign {
                    lhs: id,
                    ty:  IrType::UInt(64),
                    rhs: Expr::Value(imm(0, IrType::UInt(64))),
                }];
            }
        }
    }

    vec![Stmt::Assign {
        lhs: id, ty,
        rhs: Expr::BinOp { op, lhs: lhs_val, rhs: rhs_val },
    }]
}

fn lift_call(ops: &str, next_id: &mut u32) -> Vec<Stmt> {
    let ops = ops.trim();

    // System V AMD64 ABI argument registers.
    let arg_regs = ["rdi", "rsi", "rdx", "rcx", "r8", "r9"];
    let args: Vec<Value> = arg_regs.iter().map(|r| reg_val(r)).collect();

    let target = if let Some(addr) = parse_call_target(ops) {
        CallTarget::Direct(addr)
    } else if ops.starts_with("0x") {
        CallTarget::Direct(parse_imm(ops))
    } else if is_mem(ops) || ops.starts_with('%') || !ops.is_empty() {
        CallTarget::Indirect(operand_value(ops))
    } else {
        CallTarget::Named(ops.to_string())
    };

    let (id, _) = fresh(next_id, IrType::UInt(64));
    vec![Stmt::Assign {
        lhs: id,
        ty:  IrType::UInt(64),
        rhs: Expr::Call { target, args, ret_ty: IrType::UInt(64) },
    }]
}

fn parse_call_target(ops: &str) -> Option<u64> {
    let ops = ops.trim();
    if let Some(hex) = ops.strip_prefix("0x").or_else(|| ops.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).ok();
    }
    None
}

// ── Operand → Value helpers ───────────────────────────────────────────────────

/// Turn an operand string into a Value (register var or immediate).
fn operand_value(op: &str) -> Value {
    let op = op.trim();
    if is_imm(op) {
        let v = if op.starts_with("0x") || op.starts_with("0X") {
            parse_imm(op)
        } else {
            op.parse::<u64>().unwrap_or(0)
        };
        imm(v, IrType::UInt(64))
    } else if is_mem(op) {
        // Memory operand used as value → load.
        let ty = infer_mem_type(op);
        Value::Var { id: 30, ty } // placeholder: mem operand
    } else {
        reg_val(op)
    }
}

/// Turn an operand string into a pointer Value (for loads/stores).
fn operand_ptr(op: &str) -> Value {
    // Strip "qword ptr", "dword ptr", etc.
    let op = strip_mem_prefix(op);
    // Strip brackets.
    let inner = op.trim_start_matches('[').trim_end_matches(']').trim();
    // Try to parse as address.
    if inner.starts_with("0x") {
        imm(parse_imm(inner), IrType::ptr(IrType::UInt(64)))
    } else {
        // Base register or expression.
        let base_reg = inner.split(|c| c == '+' || c == '-' || c == '*').next().unwrap_or(inner).trim();
        let mut v = reg_val(base_reg);
        // Upgrade type to pointer.
        match &mut v {
            Value::Var { ty, .. } => *ty = IrType::ptr(IrType::UInt(64)),
            _ => {}
        }
        v
    }
}

fn strip_mem_prefix(op: &str) -> &str {
    for prefix in &["qword ptr ", "dword ptr ", "word ptr ", "byte ptr ", "xmmword ptr "] {
        if let Some(rest) = op.strip_prefix(prefix) {
            return rest;
        }
    }
    op
}

fn infer_mem_type(op: &str) -> IrType {
    if op.contains("byte")  { return IrType::UInt(8); }
    if op.contains("word")  { return IrType::UInt(16); }
    if op.contains("dword") { return IrType::UInt(32); }
    IrType::UInt(64)
}

fn value_type(v: &Value) -> IrType {
    v.ty().clone()
}
