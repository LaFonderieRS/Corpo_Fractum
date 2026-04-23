//! x86-64 instruction lifter with register mapping and flag tracking.

use rustdec_disasm::Instruction;
use rustdec_ir::{BinOp, CallTarget, Expr, IrType, IrTypeRef, Stmt, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{trace, warn};

// ── Register table ────────────────────────────────────────────────────────────

#[derive(Default, Clone, Debug)]
pub struct RegisterTable {
    mapping: HashMap<String, u32>,
}

impl RegisterTable {
    pub fn get(&self, reg: &str) -> Option<u32> {
        self.mapping.get(&normalize_reg(reg)).copied()
    }
    pub fn set(&mut self, reg: &str, id: u32) {
        self.mapping.insert(normalize_reg(reg), id);
    }
}

// ── Flag tracker ──────────────────────────────────────────────────────────────

#[derive(Default, Clone, Debug)]
pub struct FlagTracker {
    pub zf: Option<u32>,
    pub sf: Option<u32>,
    pub cf: Option<u32>,
    pub of: Option<u32>,
}

// ── Public entry points ───────────────────────────────────────────────────────

/// Lift a block and return `(stmts, rax_id_at_end)`.
///
/// The `RegisterTable` is pre-seeded with the System V x86-64 ABI calling
/// convention registers so that reads of `rsp`, `rdi`, `rsi`, etc. before
/// any assignment produce real SSA ids instead of placeholder sentinels.
///
/// Each ABI register is allocated a fresh SSA id at the start so that:
/// - The codegen sees proper variable names rather than `v906`.
/// - Copy-propagation can eliminate trivial copies.
/// - Frame analysis correctly identifies rbp/rsp chains.
pub fn lift_block_with_regs(
    insns:   &[&Instruction],
    next_id: &mut u32,
) -> (Vec<Stmt>, Option<u32>, HashMap<u32, String>) {
    let mut stmts = Vec::new();
    let mut regs  = RegisterTable::default();
    let mut flags = FlagTracker::default();

    // Seed ABI registers and capture the id → name table so the codegen
    // can substitute "rdi" for "v14" when the register is used but never
    // written (i.e. it enters the block as an implicit input).
    let reg_names = seed_abi_regs(&mut regs, next_id);

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
    let rax_id = regs.get("rax");
    (stmts, rax_id, reg_names)
}


/// Pre-allocate SSA ids for the System V x86-64 ABI registers.
///
/// We seed both caller-saved (rdi…r9, rax) and callee-saved (rbx, r12…r15)
/// registers plus the stack and frame pointers.  This prevents placeholder
/// sentinel ids (900-931) from leaking into the codegen output.
///
/// The allocated ids are intentionally *not* emitted as IR statements —
/// they represent the implicit "input" values of the block.
fn seed_abi_regs(regs: &mut RegisterTable, next_id: &mut u32) -> HashMap<u32, String> {
    let mut names: HashMap<u32, String> = HashMap::new();

    // Seed list: (canonical 64-bit name, all aliases).
    // We register the canonical name for every id so the codegen can
    // print "rdi" regardless of which alias appeared in the assembly.
    // Each tuple: (canonical 64-bit name, list of aliases that map to the same id).
    // The id is allocated once; every alias points to the same canonical name.
    const ABI_REGS: &[(&str, &[&str])] = &[
        ("rdi", &["edi",  "di",  "dil"]),
        ("rsi", &["esi",  "si",  "sil"]),
        ("rdx", &["edx",  "dx",  "dl",  "dh"]),
        ("rcx", &["ecx",  "cx",  "cl",  "ch"]),
        ("r8",  &["r8d"]),
        ("r9",  &["r9d"]),
        ("rax", &["eax",  "ax",  "al",  "ah"]),
        ("r10", &["r10d"]),
        ("r11", &["r11d"]),
        ("rbx", &["ebx",  "bx",  "bl",  "bh"]),
        ("r12", &["r12d"]),
        ("r13", &["r13d"]),
        ("r14", &["r14d"]),
        ("r15", &["r15d"]),
        ("rsp", &["esp",  "sp",  "spl"]),
        ("rbp", &["ebp",  "bp",  "bpl"]),
    ];

    for &(canon, aliases) in ABI_REGS {
        let ty = reg_type(canon);
        let (id, _) = fresh(next_id, ty);
        // Register the canonical name.
        regs.set(canon, id);
        names.insert(id, canon.to_string());
        // Register all aliases to the same id.
        for &alias in aliases {
            regs.set(alias, id);
        }
    }

    names
}

/// Convenience wrapper — drops the rax id.
pub fn lift_block(insns: &[&Instruction], next_id: &mut u32) -> Vec<Stmt> {
    lift_block_with_regs(insns, next_id).0
}

// ── Mnemonic normalisation ────────────────────────────────────────────────────

/// Strip the Capstone size suffix from a mnemonic.
///
/// Capstone on x86-64 appends `b/w/l/q` to many mnemonics (`movq`, `pushq`,
/// `subq`, `callq`, `retq`…).  We strip it so the match arms below work
/// regardless of which form Capstone emits.
///
/// Only strips when the resulting base is a known mnemonic root — this
/// prevents accidentally mangling names like `rol`, `ror`, `rcl`, `rcr`.
fn strip_size_suffix(mnem: &str) -> &str {
    if mnem.len() >= 3 {
        let stripped = mnem.strip_suffix('q')
            .or_else(|| mnem.strip_suffix('l'))
            .or_else(|| mnem.strip_suffix('w'))
            .or_else(|| mnem.strip_suffix('b'));

        if let Some(base) = stripped {
            if is_known_mnemonic_root(base) {
                return base;
            }
        }
    }
    mnem
}

fn is_known_mnemonic_root(s: &str) -> bool {
    matches!(s,
        "mov" | "movabs" | "movz" | "movs" | "movsd"
        | "push" | "pop"
        | "sub"  | "add"  | "and"  | "or"   | "xor"
        | "cmp"  | "test" | "adc"  | "sbb"
        | "shl"  | "shr"  | "sar"  | "imul" | "mul"
        | "idiv" | "div"
        | "inc"  | "dec"  | "neg"  | "not"
        | "lea"  | "call" | "ret"  | "jmp"
        | "nop"  | "leave"| "xchg"
        | "sto"  | "lod"  | "sca"
        | "rep"  | "repz" | "repnz"
        | "nopl" | "nopw"
    )
}

// ── Per-instruction lifter ────────────────────────────────────────────────────

fn lift_insn(
    insn:    &Instruction,
    next_id: &mut u32,
    regs:    &mut RegisterTable,
    flags:   &mut FlagTracker,
) -> Vec<Stmt> {
    let raw  = insn.mnemonic.to_lowercase();
    let mnem = strip_size_suffix(&raw);
    let ops  = insn.operands.trim();

    // Capstone emits AT&T syntax: "src, dst" (source first, destination second).
    // We also handle `%`-prefixed operands — normalize_reg strips the `%`.
    let parts: Vec<&str> = ops.splitn(2, ',').map(str::trim).collect();
    let op0 = parts.first().copied().unwrap_or(""); // AT&T src (single-operand: the operand)
    let op1 = parts.get(1).copied().unwrap_or("");  // AT&T dst (two-operand instructions)

    match mnem {
        // ── Data movement ────────────────────────────────────────────────────
        // Two-operand AT&T: src=op0, dst=op1 — swap relative to Intel convention.
        "movsx" | "movsxd" => lift_movsx(op1, op0, next_id, regs),
        "mov" | "movabs" | "movzx" | "movz" | "movs" => lift_mov(op1, op0, next_id, regs),

        "lea"   => lift_lea(op1, op0, next_id, regs, insn.address, insn.size),
        "push"  => lift_push(op0, next_id, regs),
        "pop"   => lift_pop(op0, next_id, regs),
        "leave" => lift_leave(next_id, regs),

        "xchg" => {
            let va = operand_to_val(op0, regs);
            let vb = operand_to_val(op1, regs);
            let ty = value_type(&va);
            let (tmp, tmp_val) = fresh(next_id, ty.clone());
            // tmp = va ; *va_ptr = vb ; *vb_ptr = tmp
            // (simplified — we don't emit true pointer semantics for xchg)
            vec![
                Stmt::Assign { lhs: tmp, ty: (*ty).clone(), rhs: Expr::Value(va.clone()) },
                Stmt::Store  { ptr: va, val: vb.clone() },
                Stmt::Store  { ptr: vb, val: tmp_val },
            ]
        }

        // ── Arithmetic / logic ───────────────────────────────────────────────
        // Two-operand AT&T: src=op0, dst=op1.
        "add" | "sub" | "and" | "or" | "xor"
        | "cmp" | "test"
        | "adc" | "sbb"
        | "shl" | "shr" | "sar"
        | "imul" | "mul" => lift_binop(mnem, op1, op0, next_id, regs, flags),

        "div"  => lift_div(op0, next_id, regs, false),
        "idiv" => lift_div(op0, next_id, regs, true),

        "inc" => lift_incdec(op0, BinOp::Add, next_id, regs),
        "dec" => lift_incdec(op0, BinOp::Sub, next_id, regs),

        "neg" => {
            let lhs = operand_to_val(op0, regs);
            let ty  = value_type(&lhs);
            let (id, _) = fresh(next_id, ty.clone());
            regs.set(op0, id);
            vec![Stmt::Assign { lhs: id, ty: (*ty).clone(),
                rhs: Expr::BinOp { op: BinOp::Sub,
                    lhs: Value::Const { val: 0, ty }, rhs: lhs } }]
        }

        "not" => {
            let lhs = operand_to_val(op0, regs);
            let ty  = value_type(&lhs);
            let (id, _) = fresh(next_id, ty.clone());
            regs.set(op0, id);
            vec![Stmt::Assign { lhs: id, ty: (*ty).clone(),
                rhs: Expr::BinOp { op: BinOp::Xor, lhs,
                    rhs: Value::Const { val: u64::MAX, ty } } }]
        }

        // ── Calls ────────────────────────────────────────────────────────────
        "call" | "lcall" => lift_call(ops, next_id, regs),

        // ── String / rep ops (opaque) ─────────────────────────────────────────
        "rep" | "repz" | "repnz" | "repe" | "repne"
        | "sto" | "lod" | "sca"
        | "stosb" | "stosd" | "stosq"
        | "movsb" | "movsd" | "movsq"
        | "scasb" | "scasq" | "lodsb" | "lodsq" => {
            let (id, _) = fresh(next_id, IrType::Void);
            vec![Stmt::Assign { lhs: id, ty: IrType::Void,
                rhs: Expr::Opaque(format!("{} {}", insn.mnemonic, ops)) }]
        }

        // ── Syscall ───────────────────────────────────────────────────────────
        // Linux x86-64: nr in rax, args in rdi/rsi/rdx/r10/r8/r9 (r10 ≠ rcx!).
        // We represent this as a special Call to "__syscall" so the codegen can
        // look up the syscall name from the number and emit `syscall(SYS_xxx, ...)`.
        "syscall" => lift_syscall(next_id, regs),

        // ── No-ops ────────────────────────────────────────────────────────────
        "nop" | "nopl" | "nopw"
        | "endbr64" | "endbr32" | "data16" => vec![Stmt::Nop],

        // ── Terminators — handled by CFG builder ──────────────────────────────
        "ret"  | "retf" | "retn"
        | "jmp"| "ljmp"
        | "je" | "jne"  | "jz"  | "jnz"
        | "jl" | "jle"  | "jg"  | "jge"
        | "jb" | "jbe"  | "ja"  | "jae"
        | "js" | "jns"  | "jo"  | "jno"
        | "jp" | "jnp"  | "jpe" | "jpo"
        | "jcxz" | "jecxz" | "jrcxz"
        | "hlt" | "ud2" | "int3" => vec![],

        other => {
            warn!(mnem = %other, ops = %ops, "unhandled instruction — emitting Opaque");
            let (id, _) = fresh(next_id, IrType::Unknown);
            vec![Stmt::Assign { lhs: id, ty: IrType::Unknown,
                rhs: Expr::Opaque(format!("{} {}", insn.mnemonic, ops)) }]
        }
    }
}

// ── Sub-lifters ───────────────────────────────────────────────────────────────

fn lift_mov(dst: &str, src: &str, next_id: &mut u32, regs: &mut RegisterTable) -> Vec<Stmt> {
    let dst_ty = reg_type(dst);

    if is_mem(dst) {
        let src_val = operand_to_val(src, regs);
        let (ptr_stmts, ptr_val) = operand_ptr_full(dst, next_id, regs);
        let mut stmts = ptr_stmts;
        stmts.push(Stmt::Store { ptr: ptr_val, val: src_val });
        return stmts;
    }

    // Register destination.
    let (rhs_expr, extra_stmts) = if is_mem(src) {
        // Load from memory — emit full address computation.
        let (ptr_stmts, ptr_val) = operand_ptr_full(src, next_id, regs);
        let expr = Expr::Load { ptr: ptr_val, ty: dst_ty.clone() };
        (expr, ptr_stmts)
    } else {
        let src_val = operand_to_val(src, regs);
        let expr = if *value_type(&src_val) != dst_ty {
            Expr::Cast { val: src_val, to: dst_ty.clone() }
        } else {
            Expr::Value(src_val)
        };
        (expr, vec![])
    };

    let (new_id, _) = fresh(next_id, dst_ty.clone());
    regs.set(dst, new_id);

    let mut stmts = extra_stmts;
    stmts.push(Stmt::Assign { lhs: new_id, ty: dst_ty.clone(), rhs: rhs_expr });

    // x86-64: writing eXX zero-extends into rXX.
    if dst_ty == IrType::UInt(32) && is_gp_reg(dst) {
        let ext_id = alloc(next_id, IrType::UInt(64));
        regs.set(&to_64bit_name(dst), ext_id);
        stmts.push(Stmt::Assign {
            lhs: ext_id, ty: IrType::UInt(64),
            rhs: Expr::Cast { val: Value::Var { id: new_id, ty: Arc::new(dst_ty) }, to: IrType::UInt(64) },
        });
    }
    stmts
}

fn lift_push(src: &str, next_id: &mut u32, regs: &mut RegisterTable) -> Vec<Stmt> {
    // push src  ≡  rsp -= 8 ; *rsp = src
    let val = operand_to_val(src, regs);
    let rsp_old = reg_val("rsp", regs);
    let (rsp_new, rsp_new_val) = fresh(next_id, IrType::UInt(64));
    regs.set("rsp", rsp_new);
    vec![
        Stmt::Assign { lhs: rsp_new, ty: IrType::UInt(64),
            rhs: Expr::BinOp { op: BinOp::Sub, lhs: rsp_old,
                rhs: Value::Const { val: 8, ty: Arc::new(IrType::UInt(64)) } } },
        Stmt::Store { ptr: rsp_new_val, val },
    ]
}

fn lift_pop(dst: &str, next_id: &mut u32, regs: &mut RegisterTable) -> Vec<Stmt> {
    // pop dst  ≡  dst = *rsp ; rsp += 8
    let ty      = reg_type(dst);
    let rsp_ptr = reg_val("rsp", regs);
    let (dst_id, _) = fresh(next_id, ty.clone());
    regs.set(dst, dst_id);
    let rsp_old = reg_val("rsp", regs);
    let (rsp_new, _) = fresh(next_id, IrType::UInt(64));
    regs.set("rsp", rsp_new);
    vec![
        Stmt::Assign { lhs: dst_id, ty,
            rhs: Expr::Load { ptr: rsp_ptr, ty: IrType::UInt(64) } },
        Stmt::Assign { lhs: rsp_new, ty: IrType::UInt(64),
            rhs: Expr::BinOp { op: BinOp::Add, lhs: rsp_old,
                rhs: Value::Const { val: 8, ty: Arc::new(IrType::UInt(64)) } } },
    ]
}

fn lift_syscall(next_id: &mut u32, regs: &mut RegisterTable) -> Vec<Stmt> {
    // Arg order: nr=rax, then rdi, rsi, rdx, r10 (NOT rcx), r8, r9.
    let nr  = reg_val("rax", regs);
    let a0  = reg_val("rdi", regs);
    let a1  = reg_val("rsi", regs);
    let a2  = reg_val("rdx", regs);
    let a3  = reg_val("r10", regs);
    let a4  = reg_val("r8",  regs);
    let a5  = reg_val("r9",  regs);
    let (ret_id, _) = fresh(next_id, IrType::SInt(64));
    regs.set("rax", ret_id);
    vec![Stmt::Assign {
        lhs: ret_id,
        ty:  IrType::SInt(64),
        rhs: Expr::Call {
            target: CallTarget::Named("__syscall".to_string()),
            args:   vec![nr, a0, a1, a2, a3, a4, a5],
            ret_ty: IrType::SInt(64),
        },
    }]
}

fn lift_leave(next_id: &mut u32, regs: &mut RegisterTable) -> Vec<Stmt> {
    // leave ≡ mov rsp, rbp ; pop rbp
    let rbp_val = reg_val("rbp", regs);
    let (rsp_id, _) = fresh(next_id, IrType::UInt(64));
    regs.set("rsp", rsp_id);
    let mut stmts = vec![Stmt::Assign { lhs: rsp_id, ty: IrType::UInt(64),
        rhs: Expr::Value(rbp_val) }];
    stmts.extend(lift_pop("rbp", next_id, regs));
    stmts
}

/// Shared implementation for `inc` and `dec`.
fn lift_incdec(dst: &str, op: BinOp, next_id: &mut u32, regs: &mut RegisterTable) -> Vec<Stmt> {
    let lhs = operand_to_val(dst, regs);
    let ty  = value_type(&lhs);
    let (id, _) = fresh(next_id, ty.clone());
    regs.set(dst, id);
    vec![Stmt::Assign { lhs: id, ty: (*ty).clone(),
        rhs: Expr::BinOp { op, lhs,
            rhs: Value::Const { val: 1, ty: Arc::new(IrType::UInt(64)) } } }]
}

// ── Memory address expression parser ─────────────────────────────────────────

#[derive(Debug, Default)]
struct MemExpr<'a> {
    base:  Option<&'a str>,
    index: Option<&'a str>,
    scale: u64,
    disp:  Option<i64>,
}

impl<'a> MemExpr<'a> {
    fn parse(s: &'a str) -> Self {
        let s = s.trim().trim_start_matches('*'); // strip AT&T indirect-call marker
        let s = strip_mem_prefix(s);

        // AT&T form: optional_disp(%base, %index, scale) — e.g. "0xff9(%rip)", "-8(%rbp)"
        if s.contains('(') && !s.contains('[') {
            let mut expr = MemExpr { scale: 1, ..Default::default() };
            if let Some(paren) = s.find('(') {
                let disp_str = s[..paren].trim();
                if !disp_str.is_empty() {
                    expr.disp = parse_int_signed(disp_str);
                }
                let inner = s[paren + 1..].trim_end_matches(')');
                let components: Vec<&str> = inner.split(',').map(str::trim).collect();
                if let Some(base) = components.first() {
                    let b = base.trim_start_matches('%');
                    if !b.is_empty() && is_register(b) {
                        expr.base = Some(b);
                    }
                }
                if let Some(idx) = components.get(1) {
                    let i = idx.trim_start_matches('%');
                    if !i.is_empty() && is_register(i) {
                        expr.index = Some(i);
                    }
                }
                if let Some(sc) = components.get(2) {
                    expr.scale = parse_int(sc).unwrap_or(1);
                }
            }
            return expr;
        }

        // Intel form: [base + index*scale + disp]
        let inner = s.trim_start_matches('[').trim_end_matches(']').trim();
        let mut expr = MemExpr { scale: 1, ..Default::default() };

        // Tokenise on `+`/`-`, keeping the sign attached to the next token.
        let mut tokens: Vec<&str> = Vec::new();
        let mut start = 0;
        let bytes = inner.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if (bytes[i] == b'+' || bytes[i] == b'-') && i > start {
                tokens.push(inner[start..i].trim());
                start = i;
            }
            i += 1;
        }
        tokens.push(inner[start..].trim());

        for tok in tokens {
            let tok = tok.trim();
            if tok.is_empty() { continue; }

            if tok.contains('*') {
                let mut parts = tok.splitn(2, '*');
                let idx_raw = parts.next().unwrap_or("").trim();
                let scale   = parts.next().unwrap_or("1").trim();
                // Strip sign/% from the index register token.
                let idx = idx_raw.trim_start_matches(['+', '-']).trim()
                                 .trim_start_matches('%');
                expr.index = Some(idx);
                expr.scale = parse_int(scale).unwrap_or(1);
            } else if let Some(v) = parse_int_signed(tok) {
                expr.disp = Some(expr.disp.unwrap_or(0).wrapping_add(v));
            } else {
                // Register token — strip sign prefix and AT&T `%`.
                let reg = tok.trim_start_matches(['+', '-']).trim()
                             .trim_start_matches('%');
                if is_register(reg) {
                    if expr.base.is_none() { expr.base = Some(reg); }
                    else if expr.index.is_none() { expr.index = Some(reg); }
                }
            }
        }
        expr
    }

    /// Emit IR stmts computing the effective address, return `(stmts, ptr_val)`.
    fn emit_addr(&self, next_id: &mut u32, regs: &RegisterTable) -> (Vec<Stmt>, Value) {
        let mut stmts = Vec::new();

        // Base register, or 0 if absent.
        let mut acc: Value = match self.base {
            Some(r) => {
                let id = regs.get(r)
                    .or_else(|| { let r64 = to_64bit_name(r); regs.get(&r64) })
                    .unwrap_or_else(|| reg_name_to_placeholder_id(r));
                Value::Var { id, ty: Arc::new(IrType::UInt(64)) }
            }
            None => Value::Const { val: 0, ty: Arc::new(IrType::UInt(64)) },
        };

        // index * scale
        if let Some(idx) = self.index {
            let idx_id = regs.get(idx)
                .or_else(|| { let r64 = to_64bit_name(idx); regs.get(&r64) })
                .unwrap_or_else(|| reg_name_to_placeholder_id(idx));
            let idx_val = Value::Var { id: idx_id, ty: Arc::new(IrType::UInt(64)) };

            let scaled = if self.scale == 1 {
                idx_val
            } else {
                let (s_id, s_val) = fresh(next_id, IrType::UInt(64));
                stmts.push(Stmt::Assign { lhs: s_id, ty: IrType::UInt(64),
                    rhs: Expr::BinOp { op: BinOp::Mul, lhs: idx_val,
                        rhs: Value::Const { val: self.scale, ty: Arc::new(IrType::UInt(64)) } } });
                s_val
            };
            let (a_id, a_val) = fresh(next_id, IrType::UInt(64));
            stmts.push(Stmt::Assign { lhs: a_id, ty: IrType::UInt(64),
                rhs: Expr::BinOp { op: BinOp::Add, lhs: acc, rhs: scaled } });
            acc = a_val;
        }

        // + displacement
        if let Some(d) = self.disp {
            if d != 0 {
                let (op, abs) = if d < 0 {
                    (BinOp::Sub, (-d) as u64)
                } else {
                    (BinOp::Add, d as u64)
                };
                let (a_id, a_val) = fresh(next_id, IrType::UInt(64));
                stmts.push(Stmt::Assign { lhs: a_id, ty: IrType::UInt(64),
                    rhs: Expr::BinOp { op, lhs: acc,
                        rhs: Value::Const { val: abs, ty: Arc::new(IrType::UInt(64)) } } });
                acc = a_val;
            }
        }

        // Cast to pointer.
        let (ptr_id, ptr_val) = fresh(next_id, IrType::ptr(IrType::UInt(64)));
        stmts.push(Stmt::Assign { lhs: ptr_id, ty: IrType::ptr(IrType::UInt(64)),
            rhs: Expr::Cast { val: acc, to: IrType::ptr(IrType::UInt(64)) } });
        (stmts, ptr_val)
    }
}

fn lift_lea(
    dst: &str, src: &str,
    next_id: &mut u32, regs: &mut RegisterTable,
    insn_address: u64, insn_size: usize,
) -> Vec<Stmt> {
    let expr = MemExpr::parse(src);

    // RIP-relative: resolve to an absolute address at lift time so downstream
    // passes see a plain constant rather than an opaque RIP arithmetic chain.
    if expr.base == Some("rip") {
        let rip = insn_address + insn_size as u64;
        let effective = rip.wrapping_add(expr.disp.unwrap_or(0) as u64);
        let addr_val  = Value::Const { val: effective, ty: Arc::new(IrType::UInt(64)) };
        let (dst_id, _) = fresh(next_id, IrType::UInt(64));
        regs.set(dst, dst_id);
        return vec![Stmt::Assign { lhs: dst_id, ty: IrType::UInt(64),
            rhs: Expr::Value(addr_val) }];
    }

    let (mut stmts, addr_val) = expr.emit_addr(next_id, regs);
    let (dst_id, _) = fresh(next_id, IrType::UInt(64));
    regs.set(dst, dst_id);
    stmts.push(Stmt::Assign { lhs: dst_id, ty: IrType::UInt(64),
        rhs: Expr::Cast { val: addr_val, to: IrType::UInt(64) } });
    stmts
}

fn lift_binop(
    mnem: &str, dst: &str, src: &str,
    next_id: &mut u32, regs: &mut RegisterTable, flags: &mut FlagTracker,
) -> Vec<Stmt> {
    let mut stmts = Vec::new();
    let lhs_val = operand_to_val(dst, regs);
    let rhs_val = operand_to_val(src, regs);
    let ty = value_type(&lhs_val);

    // Determine the BinOp and whether the result type should be signed.
    // `imul` and `sar` produce signed results — stamp SInt so downstream
    // comparisons and codegen can distinguish signed from unsigned arithmetic.
    let (op, result_ty): (BinOp, IrTypeRef) = match mnem {
        "add" | "adc"          => (BinOp::Add,  ty.clone()),
        "sub" | "sbb" | "cmp"  => (BinOp::Sub,  ty.clone()),
        "and" | "test"         => (BinOp::And,  ty.clone()),
        "or"                   => (BinOp::Or,   ty.clone()),
        "xor"                  => (BinOp::Xor,  ty.clone()),
        "shl"                  => (BinOp::Shl,  ty.clone()),
        "shr"                  => (BinOp::LShr, ty.clone()),
        "sar"                  => (BinOp::AShr, Arc::new(to_signed(&ty))), // signed result
        "imul"                 => (BinOp::Mul,  Arc::new(to_signed(&ty))), // signed result
        "mul"                  => (BinOp::Mul,  ty.clone()),
        _                      => (BinOp::Add,  ty.clone()),
    };

    // xor reg, reg idiom → 0  (very common zeroing pattern)
    if op == BinOp::Xor {
        if let (Value::Var { id: la, .. }, Value::Var { id: ra, .. }) = (&lhs_val, &rhs_val) {
            if la == ra {
                let id = alloc(next_id, IrType::UInt(64));
                regs.set(dst, id);
                return vec![Stmt::Assign { lhs: id, ty: IrType::UInt(64),
                    rhs: Expr::Value(Value::Const { val: 0, ty: Arc::new(IrType::UInt(64)) }) }];
            }
        }
    }

    // Core computation.
    let (res_id, _) = fresh(next_id, result_ty.clone());
    stmts.push(Stmt::Assign { lhs: res_id, ty: (*result_ty).clone(),
        rhs: Expr::BinOp { op, lhs: lhs_val, rhs: rhs_val } });

    // Write-back (cmp / test are flag-only — no destination update).
    if mnem != "cmp" && mnem != "test" {
        regs.set(dst, res_id);
    }

    // Zero flag: ZF = (result == 0).  Use UInt(8) so codegen emits uint8_t.
    let res_ref = Value::Var { id: res_id, ty: result_ty.clone() };
    let (zf_id, _) = fresh(next_id, IrType::UInt(8));
    flags.zf = Some(zf_id);
    stmts.push(Stmt::Assign { lhs: zf_id, ty: IrType::UInt(8),
        rhs: Expr::BinOp { op: BinOp::Eq, lhs: res_ref,
            rhs: Value::Const { val: 0, ty: result_ty } } });

    stmts
}

/// `movsx`/`movsxd` — sign-extend src into dst, stamp SInt on the result.
///
/// Unlike plain `mov`, these indicate the value is signed by intent.
/// Propagating `SInt` here means that any subsequent `cmp`/`jl` using
/// this register will find a signed type and emit the correct cast.
fn lift_movsx(dst: &str, src: &str, next_id: &mut u32, regs: &mut RegisterTable) -> Vec<Stmt> {
    let src_val = operand_to_val(src, regs);
    let dst_bits = match reg_type(dst) {
        IrType::UInt(b) | IrType::SInt(b) => b,
        _ => 64,
    };
    let dst_ty = IrType::SInt(dst_bits);
    let (new_id, _) = fresh(next_id, dst_ty.clone());
    regs.set(dst, new_id);
    let mut stmts = vec![Stmt::Assign {
        lhs: new_id, ty: dst_ty.clone(),
        rhs: Expr::Cast { val: src_val, to: dst_ty.clone() },
    }];
    // x86-64: writing a 32-bit reg zero-extends into the 64-bit counterpart.
    // For sign-extension, keep the signed type all the way to 64 bits so that
    // the 64-bit register also carries SInt — this prevents spurious unsigned
    // casts in codegen when the full-width register is later compared.
    if dst_bits == 32 && is_gp_reg(dst) {
        let ext_id = alloc(next_id, IrType::SInt(64));
        regs.set(&to_64bit_name(dst), ext_id);
        stmts.push(Stmt::Assign {
            lhs: ext_id, ty: IrType::SInt(64),
            rhs: Expr::Cast { val: Value::Var { id: new_id, ty: Arc::new(dst_ty) }, to: IrType::SInt(64) },
        });
    }
    stmts
}

/// `div`/`idiv` — unsigned or signed division of (rdx:rax) by src.
///
/// Full x86 semantics require the 128-bit dividend rdx:rax; we approximate
/// with rax only (sufficient for the common case where rdx was zeroed via
/// `xor rdx, rdx` or sign-extended via `cqo`).
/// Quotient → rax, remainder → rdx.
fn lift_div(src: &str, next_id: &mut u32, regs: &mut RegisterTable, signed: bool) -> Vec<Stmt> {
    let dividend = reg_val("rax", regs);
    let divisor  = operand_to_val(src, regs);
    let (quot_op, rem_op, result_ty) = if signed {
        (BinOp::SDiv, BinOp::SRem, IrType::SInt(64))
    } else {
        (BinOp::UDiv, BinOp::URem, IrType::UInt(64))
    };
    let (quot_id, _) = fresh(next_id, result_ty.clone());
    let (rem_id,  _) = fresh(next_id, result_ty.clone());
    regs.set("rax", quot_id);
    regs.set("rdx", rem_id);
    vec![
        Stmt::Assign { lhs: quot_id, ty: result_ty.clone(),
            rhs: Expr::BinOp { op: quot_op, lhs: dividend.clone(), rhs: divisor.clone() } },
        Stmt::Assign { lhs: rem_id, ty: result_ty,
            rhs: Expr::BinOp { op: rem_op,  lhs: dividend,         rhs: divisor } },
    ]
}

fn lift_call(ops: &str, next_id: &mut u32, regs: &mut RegisterTable) -> Vec<Stmt> {
    use rustdec_ir::CallTarget;

    // Strip the AT&T indirect-call `*` prefix if present.
    let ops_clean = ops.trim().trim_start_matches('*');

    let arg_regs = ["rdi", "rsi", "rdx", "rcx", "r8", "r9"];
    let args: Vec<Value> = arg_regs.iter().map(|r| reg_val(r, regs)).collect();

    let target = if let Some(addr) = parse_hex(ops_clean) {
        CallTarget::Direct(addr)
    } else if is_mem(ops_clean) {
        // Indirect through memory: emit address computation stmts inline.
        // For now we use the simplified ptr extraction; full MemExpr would
        // require threading next_id through here, which we do below.
        CallTarget::Indirect(operand_to_val(ops_clean, regs))
    } else {
        // Register indirect or named symbol.
        let reg_clean = ops_clean.trim_start_matches('%');
        if is_register(reg_clean) {
            CallTarget::Indirect(reg_val(reg_clean, regs))
        } else {
            CallTarget::Named(ops_clean.to_string())
        }
    };

    // Return value lands in rax.
    let (ret_id, _) = fresh(next_id, IrType::UInt(64));
    regs.set("rax", ret_id);
    vec![Stmt::Assign { lhs: ret_id, ty: IrType::UInt(64),
        rhs: Expr::Call { target, args, ret_ty: IrType::UInt(64) } }]
}

// ── Operand helpers ───────────────────────────────────────────────────────────

/// Resolve an operand string to a `Value`.
///
/// - Hex / decimal literal → `Const`
/// - Memory reference       → placeholder `Var { id: 9999 }` (use
///   `operand_ptr_full` when you need the actual load stmts)
/// - Register               → `Var` with the current SSA id from the table
fn operand_to_val(op: &str, regs: &RegisterTable) -> Value {
    let op = op.trim().trim_start_matches('*'); // strip AT&T indirect marker
    let op = op.trim_start_matches('$');        // strip AT&T immediate prefix
    let op = op.trim();

    if let Some(addr) = parse_hex(op) {
        return Value::Const { val: addr, ty: Arc::new(IrType::UInt(64)) };
    }
    if let Ok(v) = op.parse::<u64>() {
        return Value::Const { val: v, ty: Arc::new(IrType::UInt(64)) };
    }
    if is_mem(op) {
        // Caller should use operand_ptr_full for a proper load; this path is
        // a fallback for callers that can't emit extra stmts.
        return Value::Var { id: 9999, ty: Arc::new(infer_mem_type(op)) };
    }

    // Register — strip AT&T `%` prefix before lookup.
    let reg = op.trim_start_matches('%');
    let id  = regs.get(reg)
        .unwrap_or_else(|| reg_name_to_placeholder_id(reg));
    Value::Var { id, ty: Arc::new(reg_type(reg)) }
}

/// Emit stmts for a memory operand's address and return `(stmts, ptr_val)`.
/// This is the preferred path for loads/stores in `lift_mov`.
fn operand_ptr_full(
    op:      &str,
    next_id: &mut u32,
    regs:    &RegisterTable,
) -> (Vec<Stmt>, Value) {
    MemExpr::parse(op).emit_addr(next_id, regs)
}

/// Simplified pointer extraction — no extra stmts, best-effort base register.
/// Used in paths that cannot thread `next_id` (e.g. `operand_to_val` fallback).
fn operand_ptr(op: &str, regs: &RegisterTable) -> Value {
    let inner = strip_mem_prefix(op)
        .trim_start_matches('[').trim_end_matches(']').trim();

    if let Some(addr) = parse_hex(inner) {
        return Value::Const { val: addr, ty: Arc::new(IrType::ptr(IrType::UInt(64))) };
    }

    let base_raw = inner
        .split(|c: char| c == '+' || c == '-' || c == '*')
        .next().unwrap_or(inner).trim();
    let base = base_raw.trim_start_matches('%');
    let id = regs.get(base)
        .unwrap_or_else(|| reg_name_to_placeholder_id(base));
    Value::Var { id, ty: Arc::new(IrType::ptr(IrType::UInt(64))) }
}

fn reg_val(reg: &str, regs: &RegisterTable) -> Value {
    let reg = reg.trim_start_matches('%');
    // Try the register as-is first, then fall back to the 64-bit canonical
    // form so that e.g. `esp` finds the id seeded for `rsp`.
    let id = regs.get(reg)
        .or_else(|| { let r64 = to_64bit_name(reg); regs.get(&r64) })
        .unwrap_or_else(|| reg_name_to_placeholder_id(reg));
    Value::Var { id, ty: Arc::new(reg_type(reg)) }
}

/// Map a register name to a stable placeholder SSA id for untracked registers.
///
/// Using id=0 for all untracked registers causes false aliasing in the IR
/// (the codegen sees v0 = both rax and rdi simultaneously).  Instead we use
/// a fixed but unique id per well-known register.  Ids 900..=931 are reserved
/// for this purpose and will never be allocated by `fresh()` in practice
/// (a single function would need >900 SSA variables to collide).
fn reg_name_to_placeholder_id(reg: &str) -> u32 {
    let r = reg.trim_start_matches('%').to_lowercase();
    match r.as_str() {
        "rax" | "eax" | "ax" | "al" | "ah" => 900,
        "rbx" | "ebx" | "bx" | "bl" | "bh" => 901,
        "rcx" | "ecx" | "cx" | "cl" | "ch" => 902,
        "rdx" | "edx" | "dx" | "dl" | "dh" => 903,
        "rsi" | "esi" | "si" | "sil"        => 904,
        "rdi" | "edi" | "di" | "dil"        => 905,
        "rsp" | "esp" | "sp" | "spl"        => 906,
        "rbp" | "ebp" | "bp" | "bpl"        => 907,
        "r8"  | "r8d"                        => 908,
        "r9"  | "r9d"                        => 909,
        "r10" | "r10d"                       => 910,
        "r11" | "r11d"                       => 911,
        "r12" | "r12d"                       => 912,
        "r13" | "r13d"                       => 913,
        "r14" | "r14d"                       => 914,
        "r15" | "r15d"                       => 915,
        "rip"                                => 916,
        _                                    => 931, // unknown register
    }
}

fn fresh(next_id: &mut u32, ty: impl Into<IrTypeRef>) -> (u32, Value) {
    let id = *next_id; *next_id += 1;
    (id, Value::Var { id, ty: ty.into() })
}

fn alloc(next_id: &mut u32, _ty: IrType) -> u32 {
    let id = *next_id; *next_id += 1; id
}

// ── Type helpers ──────────────────────────────────────────────────────────────

/// Convert a type to its signed equivalent.  `SInt`, `Float`, and pointer
/// types are returned unchanged — only `UInt(N)` becomes `SInt(N)`.
fn to_signed(ty: &IrType) -> IrType {
    match ty {
        IrType::UInt(b) => IrType::SInt(*b),
        other           => other.clone(),
    }
}

fn reg_type(reg: &str) -> IrType {
    let r = normalize_reg(reg);
    if r.starts_with('r') || matches!(r.as_str(), "rsp"|"rbp"|"rip") {
        IrType::UInt(64)
    } else if r.starts_with('e') || matches!(r.as_str(), "esp"|"ebp") {
        IrType::UInt(32)
    } else if matches!(r.as_str(), "ax"|"bx"|"cx"|"dx"|"sp"|"bp"|"si"|"di") {
        IrType::UInt(16)
    } else if r.ends_with('l') || r.ends_with('h') || r.ends_with('b') {
        IrType::UInt(8)
    } else {
        IrType::UInt(64) // safe default for unknown / r8-r15 variants
    }
}

fn infer_mem_type(op: &str) -> IrType {
    if op.contains("byte")        { IrType::UInt(8)  }
    else if op.contains("word")   { IrType::UInt(16) }
    else if op.contains("dword")  { IrType::UInt(32) }
    else                          { IrType::UInt(64) }
}

fn value_type(v: &Value) -> IrTypeRef {
    match v { Value::Var { ty, .. } | Value::Const { ty, .. } => Arc::clone(ty) }
}

fn is_mem(op: &str) -> bool {
    // Strip `*` prefix before checking — AT&T uses `*(%rax)` for indirect.
    let op = op.trim().trim_start_matches('*');
    op.contains('[') || op.contains("ptr") || op.contains('(')
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
    for p in &["qword ptr ","dword ptr ","word ptr ","byte ptr ","xmmword ptr "] {
        if let Some(r) = op.strip_prefix(p) { return r; }
    }
    op
}

fn parse_hex(s: &str) -> Option<u64> {
    let s = s.trim().trim_start_matches('*');
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        let hex = hex.trim_end_matches(|c: char| !c.is_ascii_hexdigit());
        return u64::from_str_radix(hex, 16).ok();
    }
    None
}

fn parse_int_signed(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() { return None; }
    let (neg, s) = if let Some(r) = s.strip_prefix('-') { (true, r.trim()) }
                   else { (false, s.trim_start_matches('+').trim()) };
    let val = if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        i64::from_str_radix(h, 16).ok()?
    } else {
        s.parse::<i64>().ok()?
    };
    Some(if neg { -val } else { val })
}

fn parse_int(s: &str) -> Option<u64> {
    parse_int_signed(s).map(|v| v as u64)
}

fn is_register(tok: &str) -> bool {
    // Accept with or without `%` prefix.
    let r = tok.trim_start_matches('%').to_lowercase();
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
