//! # rustdec-lift
//!
//! Lifts x86-64 instructions into SSA IR statements inside each BasicBlock.
//!
//! ## What this does
//!
//! After `build_cfg` creates the block structure, every block's `stmts` vec
//! is empty and every value has type `Unknown`.  This pass fills in:
//!
//! - **Variable assignments** from register reads/writes
//! - **Memory loads and stores**
//! - **Call statements** with argument registers (System V ABI: rdi, rsi, rdx,
//!   rcx, r8, r9)
//! - **Concrete types**: pointer-sized values → `u64`, byte operands → `u8`, etc.
//! - **Return type inference**: functions ending with a value in `rax` get
//!   return type `u64`; void otherwise.
//!
//! ## What this does NOT yet do
//!
//! - Full data-flow / def-use chains (future: proper SSA φ-nodes)
//! - Flag modelling (CF, ZF, SF, OF)
//! - Floating-point / SIMD
//! - ARM / RISC-V (stubs return unlifted blocks)

pub mod x86;

use petgraph::visit::NodeIndexable;
use rustdec_disasm::Instruction;
use rustdec_ir::{IrFunction, IrType, Stmt, Terminator};
use tracing::{debug, instrument, trace};

/// Lift all basic blocks of `func` in-place.
///
/// `insns` must be the full sorted instruction slice for the binary
/// (same slice passed to `build_cfg`).
#[instrument(skip_all, fields(func = %func.name))]
pub fn lift_function(func: &mut IrFunction, insns: &[Instruction]) {
    debug!(func = %func.name, blocks = func.cfg.node_count(), "lifting function");

    // Iterate by raw NodeIndex — petgraph guarantees indices 0..node_count()
    // are valid for a stable graph that has not had nodes removed.
    let node_count = func.cfg.node_count();
    for ni in 0..node_count {
        let idx = func.cfg.from_index(ni);
        let (start_addr, end_addr) = {
            let bb = &func.cfg[idx];
            (bb.start_addr, bb.end_addr)
        };

        let block_insns: Vec<&Instruction> = insns
            .iter()
            .skip_while(|i| i.address < start_addr)
            .take_while(|i| i.address < end_addr)
            .collect();

        trace!(func  = %func.name,
               block = format_args!("{:#x}", start_addr),
               insns = block_insns.len(),
               "lifting block");

        let stmts = x86::lift_block(&block_insns, &mut func.next_var_id);
        func.cfg[idx].stmts = stmts;
    }

    infer_return_type(func);

    debug!(func = %func.name, ret = ?func.ret_ty, "lift complete");
}

/// Infer return type from the lifted blocks.
///
/// Uses `blocks_sorted()` — exposed by `IrFunction` — which already
/// imports `IntoNodeReferences` internally, so we don't need it here.
fn infer_return_type(func: &mut IrFunction) {
    // Heuristic: if any block has a Return terminator AND contains at least
    // one Assign stmt (i.e. some computed value exists), treat ret as u64.
    let has_return_value = func.blocks_sorted().iter().any(|bb| {
        matches!(bb.terminator, Terminator::Return(_))
            && bb.stmts.iter().any(|s| matches!(s, Stmt::Assign { .. }))
    });

    func.ret_ty = if has_return_value {
        IrType::UInt(64)
    } else {
        IrType::Void
    };
}
