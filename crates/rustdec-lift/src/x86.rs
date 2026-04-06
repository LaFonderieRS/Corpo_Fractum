//! x86-64 instruction lifter with register mapping and flag tracking.

use rustdec_disasm::Instruction;
use rustdec_ir::{BinOp, Expr, IrType, Stmt, Value};
use std::collections::HashMap;
use tracing::{trace, warn};

// ── Register table ────────────────────────────────────────────────────────────

/// Tracks the current SSA variable ID for each physical register.
#[derive(Default, Clone, Debug)]
pub struct RegisterTable {
    /// Maps register names (e.g. "rax") to their current SSA variable ID.
    mapping: HashMap<String, u32>,
}

impl RegisterTable {
    /// Return the current SSA ID for a register, if one has been assigned.
    pub fn get(&self, reg: &str) -> Option<u32> {
        self.mapping.get(&normalize_reg(reg)).copied()
    }

    /// Record that this register is now represented by `id`.
    pub fn set(&mut self, reg: &str, id: u32) {
        self.mapping.insert(normalize_reg(reg), id);
    }
}

// ── Flag tracker ──────────────────────────────────────────────────────────────

/// Tracks the SSA variable IDs that hold the current CPU flag values.
#[derive(Default, Clone, Debug)]
pub struct FlagTracker {
    /// Zero flag — set when the result of the last arithmetic op is zero.
    pub zf: Option<u32>,
    /// Sign flag.
    pub sf: Option<u32>,
    /// Carry flag.
    pub cf: Option<u32>,
    /// Overflow flag.
    pub of: Option<u32>,
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Lift a slice of instructions (one basic block) into IR statements.
///
/// `next_id` is the SSA variable counter shared across the whole function.
pub fn lift_block(insns: &[&Instruction], next_id: &mut u32) -> Vec<Stmt> {
    let mut stmts = Vec::new();
    let mut regs  = RegisterTable::default();
    let mut flags = FlagTracker::default();

    for insn in insns {
        let new = lift_insn(insn, next_id, &mut regs, &mut flags);
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

// ── Per-instruction lifter ────────────────────────────────────────────────────

fn lift_insn(
    insn:  &Instruction,
    next_id: &mut u32,
    regs:  &mut RegisterTable,
    flags: &mut FlagTracker,
) -> Vec<Stmt> {
    let mnem = insn.mnemonic.as_str();
    let ops  = insn.operands.trim();

    // Intel syntax: operands are "dst, src".
    let parts: Vec<&str> = ops.splitn(2, ',').map(str::trim).collect();
    let dst = parts.first().copied().unwrap_or("");
    let src = parts.get(1).copied().unwrap_or("");

    match mnem {
        // ── Data movement ────────────────────────────────────────────────────
        "mov" | "movabs" | "movzx" | "movsx" | "movsxd" => {
            lift_mov(dst, src, next_id, regs)
        }

        "lea" => lift_lea(dst, src, next_id, regs),

        "push" => lift_push(dst, next_id, regs),

        "pop" => lift_pop(dst, next_id, regs),

        "leave" => lift_leave(next_id, regs),

        // ── Arithmetic / logic ───────────────────────────────────────────────
        "add" | "sub" | "and" | "or" | "xor"
        | "cmp" | "test"
        | "adc" | "sbb"
        | "shl" | "shr" | "sar"
        | "imul" | "mul" => {
            lift_binop(mnem, dst, src, next_id, regs, flags)
        }

        "inc" => {
            let lhs = operand_to_val(dst, regs);
            let ty  = value_type(&lhs);
            let (id, _) = fresh(next_id, ty.clone());
            regs.set(dst, id);
            vec![Stmt::Assign {
                lhs: id, ty,
                rhs: Expr::BinOp {
                    op:  BinOp::Add,
                    lhs,
                    rhs: Value::Const { val: 1, ty: IrType::UInt(64) },
                },
            }]
        }

        "dec" => {
            let lhs = operand_to_val(dst, regs);
            let ty  = value_type(&lhs);
            let (id, _) = fresh(next_id, ty.clone());
            regs.set(dst, id);
            vec![Stmt::Assign {
                lhs: id, ty,
                rhs: Expr::BinOp {
                    op:  BinOp::Sub,
                    lhs,
                    rhs: Value::Const { val: 1, ty: IrType::UInt(64) },
                },
            }]
        }

        "neg" => {
            let lhs = operand_to_val(dst, regs);
            let ty  = value_type(&lhs);
            let (id, _) = fresh(next_id, ty.clone());
            regs.set(dst, id);
            vec![Stmt::Assign {
                lhs: id, ty: ty.clone(),
                rhs: Expr::BinOp {
                    op:  BinOp::Sub,
                    lhs: Value::Const { val: 0, ty },
                    rhs: lhs,
                },
            }]
        }

        "not" => {
            let lhs = operand_to_val(dst, regs);
            let ty  = value_type(&lhs);
            let (id, _) = fresh(next_id, ty.clone());
            regs.set(dst, id);
            vec![Stmt::Assign {
                lhs: id, ty: ty.clone(),
                rhs: Expr::BinOp {
                    op:  BinOp::Xor,
                    lhs,
                    rhs: Value::Const { val: u64::MAX, ty },
                },
            }]
        }

        // ── Calls ────────────────────────────────────────────────────────────
        "call" | "lcall" => lift_call(ops, next_id, regs),

        // ── String / rep ops (opaque) ─────────────────────────────────────────
        "rep"  | "repz"  | "repnz" | "repe"  | "repne"
        | "stosb" | "stosd" | "stosq"
        | "movsb" | "movsd" | "movsq"
        | "scasb" | "scasq" | "lodsb" | "lodsq" => {
            let (id, _) = fresh(next_id, IrType::Void);
            vec![Stmt::Assign {
                lhs: id,
                ty:  IrType::Void,
                rhs: Expr::Opaque(format!("{mnem} {ops}")),
            }]
        }

        // ── No-ops ────────────────────────────────────────────────────────────
        "nop" | "endbr64" | "endbr32" | "data16" => vec![Stmt::Nop],

        // ── Terminators (handled by CFG builder — no stmts needed) ───────────
        "ret" | "retf" | "retn"
        | "jmp" | "ljmp"
        | "je"  | "jne" | "jz"   | "jnz"
        | "jl"  | "jle" | "jg"   | "jge"
        | "jb"  | "jbe" | "ja"   | "jae"
        | "js"  | "jns" | "jo"   | "jno"
        | "jp"  | "jnp" | "jpe"  | "jpo"
        | "jcxz"| "jecxz" | "jrcxz"
        | "hlt" | "ud2" | "int3" => vec![],

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

fn lift_mov(
    dst:     &str,
    src:     &str,
    next_id: &mut u32,
    regs:    &mut RegisterTable,
) -> Vec<Stmt> {
    let src_val = operand_to_val(src, regs);
    let dst_ty  = reg_type(dst);

    if is_mem(dst) {
        return vec![Stmt::Store {
            ptr: operand_ptr(dst, regs),
            val: src_val,
        }];
    }

    let (new_id, _) = fresh(next_id, dst_ty.clone());
    regs.set(dst, new_id);

    let mut stmts = vec![Stmt::Assign {
        lhs: new_id,
        ty:  dst_ty.clone(),
        rhs: if is_mem(src) {
            Expr::Load { ptr: operand_ptr(src, regs), ty: dst_ty.clone() }
        } else if value_type(&src_val) != dst_ty {
            Expr::Cast { val: src_val, to: dst_ty.clone() }
        } else {
            Expr::Value(src_val)
        },
    }];

    // x86-64: writing a 32-bit register zero-extends the full 64-bit register.
    if dst_ty == IrType::UInt(32) && is_gp_reg(dst) {
        let ext_id = alloc(next_id, IrType::UInt(64));
        regs.set(&to_64bit_name(dst), ext_id);
        stmts.push(Stmt::Assign {
            lhs: ext_id,
            ty:  IrType::UInt(64),
            rhs: Expr::Cast {
                val: Value::Var { id: new_id, ty: dst_ty },
                to:  IrType::UInt(64),
            },
        });
    }

    stmts
}

fn lift_push(
    src:     &str,
    next_id: &mut u32,
    regs:    &mut RegisterTable,
) -> Vec<Stmt> {
    // push src  ≡  rsp -= 8 ; *rsp = src
    //
    // Emitting the decrement explicitly lets downstream passes treat the
    // stack as ordinary memory and reason about rsp as a normal pointer.
    let val = operand_to_val(src, regs);
    let mut stmts = Vec::new();

    // 1. rsp_new = rsp - 8
    let rsp_old = reg_val("rsp", regs);
    let (rsp_new, rsp_new_val) = fresh(next_id, IrType::UInt(64));
    regs.set("rsp", rsp_new);
    stmts.push(Stmt::Assign {
        lhs: rsp_new,
        ty:  IrType::UInt(64),
        rhs: Expr::BinOp {
            op:  BinOp::Sub,
            lhs: rsp_old,
            rhs: Value::Const { val: 8, ty: IrType::UInt(64) },
        },
    });

    // 2. *rsp_new = val
    stmts.push(Stmt::Store {
        ptr: rsp_new_val,
        val,
    });

    stmts
}

fn lift_pop(
    dst:     &str,
    next_id: &mut u32,
    regs:    &mut RegisterTable,
) -> Vec<Stmt> {
    // pop dst  ≡  dst = *rsp ; rsp += 8
    let ty = reg_type(dst);
    let mut stmts = Vec::new();

    // 1. dst = *rsp
    let rsp_val = reg_val("rsp", regs);
    let (dst_id, _) = fresh(next_id, ty.clone());
    regs.set(dst, dst_id);
    stmts.push(Stmt::Assign {
        lhs: dst_id,
        ty:  ty,
        rhs: Expr::Load {
            ptr: rsp_val,
            ty:  IrType::UInt(64),
        },
    });

    // 2. rsp += 8
    let rsp_old = reg_val("rsp", regs);
    let (rsp_new, _) = fresh(next_id, IrType::UInt(64));
    regs.set("rsp", rsp_new);
    stmts.push(Stmt::Assign {
        lhs: rsp_new,
        ty:  IrType::UInt(64),
        rhs: Expr::BinOp {
            op:  BinOp::Add,
            lhs: rsp_old,
            rhs: Value::Const { val: 8, ty: IrType::UInt(64) },
        },
    });

    stmts
}

fn lift_leave(
    next_id: &mut u32,
    regs:    &mut RegisterTable,
) -> Vec<Stmt> {
    // leave ≡ mov rsp, rbp ; pop rbp
    // We delegate to lift_pop which handles rsp arithmetic explicitly.
    let mut stmts = Vec::new();

    // mov rsp, rbp
    let rbp_val = reg_val("rbp", regs);
    let (rsp_id, _) = fresh(next_id, IrType::UInt(64));
    regs.set("rsp", rsp_id);
    stmts.push(Stmt::Assign {
        lhs: rsp_id,
        ty:  IrType::UInt(64),
        rhs: Expr::Value(rbp_val),
    });

    // pop rbp  (uses the updated rsp from above)
    stmts.extend(lift_pop("rbp", next_id, regs));

    stmts
}

// ── Memory address expression parser ─────────────────────────────────────────

/// A parsed x86-64 memory addressing expression.
///
/// Intel syntax: `[base + index*scale + disp]`
/// All fields are optional; the only constraint is that at least one
/// of `base`, `index`, or `disp` is present.
#[derive(Debug, Default)]
struct MemExpr<'a> {
    base:  Option<&'a str>,
    index: Option<&'a str>,
    scale: u64,               // 1 if no explicit scale
    disp:  Option<i64>,       // signed: may be negative
}

impl<'a> MemExpr<'a> {
    /// Parse `[base + index*scale + disp]` from a bracket-enclosed string.
    ///
    /// Handles all common Intel-syntax forms:
    /// - `[rsp]`
    /// - `[rsp + 0x10]`
    /// - `[rip + 0x1234]`
    /// - `[rbx + rcx*4]`
    /// - `[rbx + rcx*4 + 0x10]`
    /// - `[rbx - 8]`
    fn parse(s: &'a str) -> Self {
        // Strip outer brackets and size prefix ("qword ptr", etc.)
        let s = strip_mem_prefix(s);
        let inner = s
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim();

        let mut expr = MemExpr { scale: 1, ..Default::default() };

        // Tokenise on `+` first, then handle `-` as negative displacement.
        // We split carefully to avoid breaking `r8` into `r` and `8`.
        let mut tokens: Vec<&str> = Vec::new();
        let mut start = 0;
        let bytes = inner.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if (bytes[i] == b'+' || bytes[i] == b'-') && i > start {
                // Keep the sign attached to the following token.
                tokens.push(inner[start..i].trim());
                start = i; // include the sign character
            }
            i += 1;
        }
        tokens.push(inner[start..].trim());

        for tok in tokens {
            let tok = tok.trim();
            if tok.is_empty() { continue; }

            if tok.contains('*') {
                // index*scale pair
                let mut parts = tok.splitn(2, '*');
                let idx   = parts.next().unwrap_or("").trim();
                let scale = parts.next().unwrap_or("1").trim();
                expr.index = Some(idx);
                expr.scale = parse_int(scale).unwrap_or(1) as u64;
            } else if let Some(v) = parse_int_signed(tok) {
                // Pure numeric token → displacement (accumulate).
                expr.disp = Some(expr.disp.unwrap_or(0).wrapping_add(v));
            } else if is_register(tok) {
                // Register token → base (first) or index (second).
                if expr.base.is_none() {
                    expr.base = Some(tok);
                } else if expr.index.is_none() {
                    expr.index = Some(tok);
                }
            }
        }

        expr
    }

    /// Emit IR statements that compute the final address into a fresh variable.
    /// Returns `(stmts, addr_value)`.
    fn emit_addr(
        &self,
        next_id: &mut u32,
        regs:    &RegisterTable,
    ) -> (Vec<Stmt>, Value) {
        let mut stmts = Vec::new();
        let ptr_ty = IrType::ptr(IrType::UInt(64));

        // Start from base, or 0 if no base.
        let base_val: Value = match self.base {
            Some(r) => {
                let id = regs.get(r).unwrap_or(0);
                Value::Var { id, ty: IrType::UInt(64) }
            }
            None => Value::Const { val: 0, ty: IrType::UInt(64) },
        };

        let mut acc: Value = base_val;

        // Add index*scale if present.
        if let Some(idx_reg) = self.index {
            let idx_id = regs.get(idx_reg).unwrap_or(0);
            let idx_val = Value::Var { id: idx_id, ty: IrType::UInt(64) };

            // scaled = index * scale
            let scaled: Value = if self.scale == 1 {
                idx_val
            } else {
                let (s_id, s_val) = fresh(next_id, IrType::UInt(64));
                stmts.push(Stmt::Assign {
                    lhs: s_id,
                    ty:  IrType::UInt(64),
                    rhs: Expr::BinOp {
                        op:  BinOp::Mul,
                        lhs: idx_val,
                        rhs: Value::Const { val: self.scale, ty: IrType::UInt(64) },
                    },
                });
                s_val
            };

            // acc = acc + scaled
            let (a_id, a_val) = fresh(next_id, IrType::UInt(64));
            stmts.push(Stmt::Assign {
                lhs: a_id,
                ty:  IrType::UInt(64),
                rhs: Expr::BinOp {
                    op:  BinOp::Add,
                    lhs: acc,
                    rhs: scaled,
                },
            });
            acc = a_val;
        }

        // Add displacement if non-zero.
        if let Some(d) = self.disp {
            if d != 0 {
                let (op, disp_abs) = if d < 0 {
                    (BinOp::Sub, (-d) as u64)
                } else {
                    (BinOp::Add, d as u64)
                };
                let (a_id, a_val) = fresh(next_id, IrType::UInt(64));
                stmts.push(Stmt::Assign {
                    lhs: a_id,
                    ty:  IrType::UInt(64),
                    rhs: Expr::BinOp {
                        op,
                        lhs: acc,
                        rhs: Value::Const { val: disp_abs, ty: IrType::UInt(64) },
                    },
                });
                acc = a_val;
            }
        }

        // Cast final value to pointer type.
        let (ptr_id, ptr_val) = fresh(next_id, ptr_ty.clone());
        stmts.push(Stmt::Assign {
            lhs: ptr_id,
            ty:  ptr_ty,
            rhs: Expr::Cast { val: acc, to: IrType::ptr(IrType::UInt(64)) },
        });

        (stmts, ptr_val)
    }
}

fn lift_lea(
    dst:     &str,
    src:     &str,
    next_id: &mut u32,
    regs:    &mut RegisterTable,
) -> Vec<Stmt> {
    // LEA dst, [base + index*scale + disp]
    //
    // Unlike a memory load, LEA only computes the *address* — it does not
    // dereference the pointer.  We parse the bracket expression and emit
    // the arithmetic directly into IR, giving downstream passes full
    // visibility into pointer arithmetic.
    let expr = MemExpr::parse(src);
    let (mut stmts, addr_val) = expr.emit_addr(next_id, regs);

    // Assign the computed address to the destination register.
    let (dst_id, _) = fresh(next_id, IrType::UInt(64));
    regs.set(dst, dst_id);
    stmts.push(Stmt::Assign {
        lhs: dst_id,
        ty:  IrType::UInt(64),
        rhs: Expr::Cast {
            val: addr_val,
            to:  IrType::UInt(64),
        },
    });

    stmts
}

fn lift_binop(
    mnem:    &str,
    dst:     &str,
    src:     &str,
    next_id: &mut u32,
    regs:    &mut RegisterTable,
    flags:   &mut FlagTracker,
) -> Vec<Stmt> {
    let mut stmts = Vec::new();

    let lhs_val = operand_to_val(dst, regs);
    let rhs_val = operand_to_val(src, regs);
    let ty = value_type(&lhs_val);

    let op = match mnem {
        "add" | "adc"        => BinOp::Add,
        "sub" | "sbb" | "cmp" => BinOp::Sub,
        "and" | "test"       => BinOp::And,
        "or"                 => BinOp::Or,
        "xor"                => BinOp::Xor,
        "shl"                => BinOp::Shl,
        "shr"                => BinOp::LShr,
        "sar"                => BinOp::AShr,
        "imul" | "mul"       => BinOp::Mul,
        _                    => BinOp::Add,
    };

    // xor rax, rax idiom → rax = 0
    if op == BinOp::Xor {
        if let (Value::Var { id: la, .. }, Value::Var { id: ra, .. }) = (&lhs_val, &rhs_val) {
            if la == ra {
                let id = alloc(next_id, IrType::UInt(64));
                regs.set(dst, id);
                return vec![Stmt::Assign {
                    lhs: id,
                    ty:  IrType::UInt(64),
                    rhs: Expr::Value(Value::Const { val: 0, ty: IrType::UInt(64) }),
                }];
            }
        }
    }

    // Core computation.
    let (res_id, res_val) = fresh(next_id, ty.clone());
    stmts.push(Stmt::Assign {
        lhs: res_id,
        ty:  ty.clone(),
        rhs: Expr::BinOp { op, lhs: lhs_val, rhs: rhs_val },
    });

    // Write result back to destination (cmp/test only set flags, no write-back).
    if mnem != "cmp" && mnem != "test" {
        let (final_id, _) = fresh(next_id, ty.clone());
        regs.set(dst, final_id);
        stmts.push(Stmt::Assign {
            lhs: final_id,
            ty:  ty.clone(),
            rhs: Expr::Value(res_val.clone()),
        });
    }

    // Zero flag: ZF = (result == 0).
    let (zf_id, _) = fresh(next_id, IrType::UInt(1));
    flags.zf = Some(zf_id);
    stmts.push(Stmt::Assign {
        lhs: zf_id,
        ty:  IrType::UInt(1),
        rhs: Expr::BinOp {
            op:  BinOp::Eq,
            lhs: res_val,
            rhs: Value::Const { val: 0, ty },
        },
    });

    stmts
}

fn lift_call(
    ops:     &str,
    next_id: &mut u32,
    regs:    &mut RegisterTable,
) -> Vec<Stmt> {
    use rustdec_ir::CallTarget;

    // System V AMD64 ABI: integer arguments in rdi, rsi, rdx, rcx, r8, r9.
    let arg_regs = ["rdi", "rsi", "rdx", "rcx", "r8", "r9"];
    let args: Vec<Value> = arg_regs.iter().map(|r| reg_val(r, regs)).collect();

    let target = if let Some(addr) = parse_hex(ops) {
        CallTarget::Direct(addr)
    } else if is_mem(ops) || ops.contains('%') {
        CallTarget::Indirect(operand_to_val(ops, regs))
    } else {
        CallTarget::Named(ops.to_string())
    };

    // Return value lands in rax.
    let (ret_id, _) = fresh(next_id, IrType::UInt(64));
    regs.set("rax", ret_id);

    vec![Stmt::Assign {
        lhs: ret_id,
        ty:  IrType::UInt(64),
        rhs: Expr::Call { target, args, ret_ty: IrType::UInt(64) },
    }]
}

// ── Operand helpers ───────────────────────────────────────────────────────────

fn operand_to_val(op: &str, regs: &RegisterTable) -> Value {
    let op = op.trim();
    if let Some(addr) = parse_hex(op) {
        return Value::Const { val: addr, ty: IrType::UInt(64) };
    }
    if let Ok(v) = op.parse::<u64>() {
        return Value::Const { val: v, ty: IrType::UInt(64) };
    }
    if is_mem(op) {
        // Memory operand used as a source value — placeholder until full
        // address-expression parsing is implemented.
        return Value::Var { id: 9999, ty: infer_mem_type(op) };
    }
    // Register operand: use the current SSA ID from the table, or 0 if unseen.
    let id = regs.get(op).unwrap_or(0);
    Value::Var { id, ty: reg_type(op) }
}

/// Resolve a memory operand to its pointer value, emitting address-computation
/// statements when the addressing mode requires arithmetic.
/// Returns `(stmts, ptr_value)`.
/*fn operand_ptr_full(
    op:      &str,
    next_id: &mut u32,
    regs:    &RegisterTable,
) -> (Vec<Stmt>, Value) {
    MemExpr::parse(op).emit_addr(next_id, regs)
}*/

/// Simplified wrapper: extract just the base pointer without emitting extra
/// stmts.  Used in paths that cannot thread `next_id` easily; full arithmetic
/// is handled by callers that use `operand_ptr_full`.
fn operand_ptr(op: &str, regs: &RegisterTable) -> Value {
    let inner = strip_mem_prefix(op)
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim();

    if let Some(addr) = parse_hex(inner) {
        return Value::Const { val: addr, ty: IrType::ptr(IrType::UInt(64)) };
    }

    let base = inner
        .split(|c: char| c == '+' || c == '-' || c == '*')
        .next()
        .unwrap_or(inner)
        .trim();

    let id = regs.get(base).unwrap_or(0);
    Value::Var { id, ty: IrType::ptr(IrType::UInt(64)) }
}

fn reg_val(reg: &str, regs: &RegisterTable) -> Value {
    let id = regs.get(reg).unwrap_or(0);
    Value::Var { id, ty: reg_type(reg) }
}

fn fresh(next_id: &mut u32, ty: IrType) -> (u32, Value) {
    let id = *next_id;
    *next_id += 1;
    (id, Value::Var { id, ty })
}

/// Allocate a fresh ID without returning the Value.
fn alloc(next_id: &mut u32, _ty: IrType) -> u32 {
    let id = *next_id;
    *next_id += 1;
    id
}

// ── Type helpers ──────────────────────────────────────────────────────────────

fn reg_type(reg: &str) -> IrType {
    let r = normalize_reg(reg);
    if r.starts_with('r') || matches!(r.as_str(), "rsp" | "rbp" | "rip") {
        IrType::UInt(64)
    } else if r.starts_with('e') || matches!(r.as_str(), "esp" | "ebp") {
        IrType::UInt(32)
    } else if matches!(r.as_str(), "ax"|"bx"|"cx"|"dx"|"sp"|"bp"|"si"|"di") {
        IrType::UInt(16)
    } else if r.ends_with('l') || r.ends_with('h') || r.ends_with('b') {
        IrType::UInt(8)
    } else {
        IrType::UInt(64)
    }
}

fn infer_mem_type(op: &str) -> IrType {
    if op.contains("byte")  { IrType::UInt(8)  }
    else if op.contains("word")  { IrType::UInt(16) }
    else if op.contains("dword") { IrType::UInt(32) }
    else                         { IrType::UInt(64) }
}

fn value_type(v: &Value) -> IrType {
    v.ty().clone()
}

fn is_mem(op: &str) -> bool {
    op.contains('[') || op.contains("ptr")
}

fn is_gp_reg(reg: &str) -> bool {
    let r = normalize_reg(reg);
    r.starts_with('e') || r.starts_with('r')
}

fn normalize_reg(reg: &str) -> String {
    reg.trim_start_matches('%').to_lowercase()
}

fn to_64bit_name(reg: &str) -> String {
    let r = normalize_reg(reg);
    if r.starts_with('e') { format!("r{}", &r[1..]) } else { r }
}

fn strip_mem_prefix(op: &str) -> &str {
    for prefix in &["qword ptr ", "dword ptr ", "word ptr ", "byte ptr ", "xmmword ptr "] {
        if let Some(rest) = op.strip_prefix(prefix) {
            return rest;
        }
    }
    op
}

fn parse_hex(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        let hex = hex.trim_end_matches(|c: char| !c.is_ascii_hexdigit());
        return u64::from_str_radix(hex, 16).ok();
    }
    None
}

/// Parse a signed integer token — handles `0x1a`, `-8`, `16`, `+4`.
fn parse_int_signed(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() { return None; }

    let (neg, s) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest.trim())
    } else {
        (false, s.trim_start_matches('+').trim())
    };

    let val = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).ok()?
    } else {
        s.parse::<i64>().ok()?
    };

    Some(if neg { -val } else { val })
}

/// Parse an unsigned integer token — handles `0x4` and `4`.
fn parse_int(s: &str) -> Option<u64> {
    parse_int_signed(s).map(|v| v as u64)
}

/// Return true if `tok` looks like an x86-64 register name.
fn is_register(tok: &str) -> bool {
    let t = tok.trim_start_matches(['+', '-', ' ']);
    let r = t.trim_start_matches('%').to_lowercase();
    // General-purpose 64/32/16/8-bit registers + rip.
    matches!(r.as_str(),
        "rax"|"rbx"|"rcx"|"rdx"|"rsi"|"rdi"|"rsp"|"rbp"|"rip"
        |"r8" |"r9" |"r10"|"r11"|"r12"|"r13"|"r14"|"r15"
        |"eax"|"ebx"|"ecx"|"edx"|"esi"|"edi"|"esp"|"ebp"
        |"r8d"|"r9d"|"r10d"|"r11d"|"r12d"|"r13d"|"r14d"|"r15d"
        |"ax" |"bx" |"cx" |"dx" |"si" |"di" |"sp" |"bp"
        |"al" |"bl" |"cl" |"dl" |"sil"|"dil"|"spl"|"bpl"
        |"ah" |"bh" |"ch" |"dh"
    )
}
