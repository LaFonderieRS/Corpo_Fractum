//! # rustdec-ir
//!
//! Intermediate Representation (IR) in SSA form.
//!
//! The IR is the pivot between the disassembler/analyser and the code
//! generators.  Each function is a list of basic blocks; each block is a
//! list of [`Stmt`] (statements) plus a [`Terminator`].
//!
//! # Design goals
//! - Architecture-independent: no mention of registers after lifting.
//! - Target-independent: no mention of C/Rust syntax.
//! - Explicit types on every value.
//! - Confidence scores so the UI can highlight uncertain deductions.

use petgraph::graph::DiGraph;
use serde::{Deserialize, Serialize};

// ── Types ─────────────────────────────────────────────────────────────────────

/// A type in the IR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IrType {
    /// Unsigned integer of given bit-width (8, 16, 32, 64, 128).
    UInt(u8),
    /// Signed integer of given bit-width.
    SInt(u8),
    /// Floating point (32 or 64 bits).
    Float(u8),
    /// Raw pointer (address-sized).
    Ptr(Box<IrType>),
    /// Fixed-length array.
    Array { elem: Box<IrType>, len: u64 },
    /// Opaque struct (name + size in bytes).
    Struct { name: String, size: u64 },
    /// Void (used as function return type only).
    Void,
    /// Type not yet inferred.
    Unknown,
}

impl IrType {
    pub fn u64() -> Self { Self::UInt(64) }
    pub fn u32() -> Self { Self::UInt(32) }
    pub fn u8()  -> Self { Self::UInt(8) }
    pub fn ptr(inner: IrType) -> Self { Self::Ptr(Box::new(inner)) }

    /// Return the size of this type in bytes, if statically known.
    pub fn byte_size(&self) -> Option<u64> {
        match self {
            Self::UInt(b) | Self::SInt(b) | Self::Float(b) => Some(*b as u64 / 8),
            Self::Ptr(_)    => Some(8), // assume 64-bit
            Self::Array { elem, len } => elem.byte_size().map(|s| s * len),
            Self::Struct { size, .. } => Some(*size),
            Self::Void | Self::Unknown => None,
        }
    }
}

// ── Values ────────────────────────────────────────────────────────────────────

/// A value: either a variable reference or an immediate constant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Value {
    /// SSA variable, uniquely identified by (function-scoped) index.
    Var { id: u32, ty: IrType },
    /// Integer constant.
    Const { val: u64, ty: IrType },
}

impl Value {
    pub fn ty(&self) -> &IrType {
        match self { Self::Var { ty, .. } | Self::Const { ty, .. } => ty }
    }

    pub fn display(&self) -> String {
        match self {
            Self::Var { id, .. }   => format!("v{id}"),
            Self::Const { val, .. } => format!("{val:#x}"),
        }
    }
}

// ── Expressions ───────────────────────────────────────────────────────────────

/// Binary operation kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp { Add, Sub, Mul, UDiv, SDiv, URem, SRem,
                 And, Or, Xor, Shl, LShr, AShr,
                 Eq, Ne, Ult, Ule, Slt, Sle }

/// An expression — the right-hand side of an assignment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Expr {
    /// Plain value (copy / phi resolution).
    Value(Value),
    /// Binary operation.
    BinOp { op: BinOp, lhs: Value, rhs: Value },
    /// Memory load: `*ptr`.
    Load { ptr: Value, ty: IrType },
    /// Function call.
    Call { target: CallTarget, args: Vec<Value>, ret_ty: IrType },
    /// Type cast / bit-cast.
    Cast { val: Value, to: IrType },
    /// Unresolved / opaque expression.
    Opaque(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CallTarget {
    /// Direct call to a known address.
    Direct(u64),
    /// Indirect call through a computed value.
    Indirect(Value),
    /// Call to a named import/symbol.
    Named(String),
}

// ── Statements ────────────────────────────────────────────────────────────────

/// A statement inside a basic block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Stmt {
    /// `v{id} = expr`
    Assign { lhs: u32, ty: IrType, rhs: Expr },
    /// `*ptr = val`
    Store { ptr: Value, val: Value },
    /// No-op (removed instruction).
    Nop,
}

// ── Terminators ───────────────────────────────────────────────────────────────

/// The last statement of a basic block — determines control flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Terminator {
    /// Unconditional jump to a block.
    Jump(BlockId),
    /// Conditional branch.
    Branch { cond: Value, true_bb: BlockId, false_bb: BlockId },
    /// Return from function.
    Return(Option<Value>),
    /// Unreachable (e.g. after `ud2`, `hlt`).
    Unreachable,
}

// ── Basic Block ───────────────────────────────────────────────────────────────

pub type BlockId = u32;

/// A linear sequence of statements with a single entry and a terminator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BasicBlock {
    pub id:         BlockId,
    /// Virtual address of the first instruction in this block.
    pub start_addr: u64,
    /// Virtual address one past the last instruction.
    pub end_addr:   u64,
    pub stmts:      Vec<Stmt>,
    pub terminator: Terminator,
    /// Confidence score for the IR of this block (0.0 = guess, 1.0 = certain).
    pub confidence: f32,
}

impl BasicBlock {
    pub fn new(id: BlockId, start_addr: u64) -> Self {
        Self {
            id,
            start_addr,
            end_addr: start_addr,
            stmts: vec![],
            terminator: Terminator::Unreachable,
            confidence: 1.0,
        }
    }
}

// ── Function ─────────────────────────────────────────────────────────────────

/// Control-flow graph edge — carries no data but is required by petgraph.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CfgEdge;

pub type CfgGraph = DiGraph<BasicBlock, CfgEdge>;

/// An IR function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IrFunction {
    /// Demangled name (may be synthetic like `sub_<addr>`).
    pub name:        String,
    /// Entry virtual address.
    pub entry_addr:  u64,
    /// Control-flow graph.  Node weights are [`BasicBlock`].
    #[serde(skip)]
    pub cfg:         CfgGraph,
    /// Parameter types (may be [`IrType::Unknown`] if not inferred).
    pub params:      Vec<IrType>,
    /// Return type.
    pub ret_ty:      IrType,
    /// Next SSA variable index (monotonically increasing).
    pub next_var_id: u32,
}

impl IrFunction {
    pub fn new(name: impl Into<String>, entry_addr: u64) -> Self {
        Self {
            name: name.into(),
            entry_addr,
            cfg: CfgGraph::new(),
            params: vec![],
            ret_ty: IrType::Unknown,
            next_var_id: 0,
        }
    }

    /// Allocate a fresh SSA variable id.
    pub fn fresh_var(&mut self) -> u32 {
        let id = self.next_var_id;
        self.next_var_id += 1;
        id
    }

    /// Return all basic blocks sorted by start address.
    ///
    /// This is the canonical iteration order used by code generators — it
    /// avoids exposing `petgraph` internals to crates that only need to
    /// emit code (e.g. `rustdec-codegen`).
    pub fn blocks_sorted(&self) -> Vec<&BasicBlock> {
        use petgraph::visit::IntoNodeReferences;
        let mut blocks: Vec<&BasicBlock> =
            self.cfg.node_references().map(|(_, b)| b).collect();
        blocks.sort_by_key(|b| b.start_addr);
        blocks
    }
}

// ── Module ────────────────────────────────────────────────────────────────────

/// Top-level IR module — one per analysed binary.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct IrModule {
    pub functions: Vec<IrFunction>,
}
