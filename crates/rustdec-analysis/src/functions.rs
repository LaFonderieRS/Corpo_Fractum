//! Function detection: collect entry points from symbols, entry point header,
//! and call-site scanning.

use rustdec_disasm::Instruction;
use rustdec_loader::{BinaryObject, SymbolKind};
use std::collections::BTreeMap;

/// Returns a map of `{ virtual_address → name }` for all detected functions.
pub fn detect_functions(
    obj: &BinaryObject,
    insns: &[Instruction],
) -> BTreeMap<u64, String> {
    let mut funcs: BTreeMap<u64, String> = BTreeMap::new();

    // 1. Binary entry point.
    if let Some(ep) = obj.entry_point {
        funcs.insert(ep, "entry".to_string());
    }

    // 2. Symbols of kind Function.
    for sym in &obj.symbols {
        if sym.kind == SymbolKind::Function && sym.address != 0 {
            funcs.insert(sym.address, sym.name.clone());
        }
    }

    // 3. Call-site scanning: every `call <direct_target>` reveals a new function.
    for insn in insns {
        if insn.is_call() {
            if let Some(target) = insn.branch_target() {
                funcs.entry(target).or_insert_with(|| {
                    format!("sub_{:x}", target)
                });
            }
        }
    }

    funcs
}
