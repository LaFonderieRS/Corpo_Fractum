//! Function detection: collect entry points from symbols, entry point header,
//! and call-site scanning.

use rustdec_disasm::Instruction;
use rustdec_loader::{BinaryObject, SymbolKind};
use std::collections::BTreeMap;
use tracing::{debug, trace};

/// Returns a map of `{ virtual_address → name }` for all detected functions.
pub fn detect_functions(
    obj: &BinaryObject,
    insns: &[Instruction],
) -> BTreeMap<u64, String> {
    let mut funcs: BTreeMap<u64, String> = BTreeMap::new();

    // 1. Binary entry point.
    if let Some(ep) = obj.entry_point {
        debug!(addr = format_args!("{:#x}", ep), "entry point added as function");
        funcs.insert(ep, "entry".to_string());
    }

    // 2. Symbols of kind Function.
    for sym in &obj.symbols {
        if sym.kind == SymbolKind::Function && sym.address != 0 {
            trace!(name = %sym.name, addr = format_args!("{:#x}", sym.address), "function from symbol table");
            funcs.insert(sym.address, sym.name.clone());
        }
    }
    debug!(count = funcs.len(), "functions from symbol table + entry point");

    // 3. Call-site scanning.
    let before = funcs.len();
    for insn in insns {
        if insn.is_call() {
            if let Some(target) = insn.branch_target() {
                let name = format!("sub_{:x}", target);
                let is_new = funcs.insert(target, name.clone()).is_none();
                if is_new {
                    trace!(
                        caller = format_args!("{:#x}", insn.address),
                        target = format_args!("{:#x}", target),
                        name   = %name,
                        "new function discovered via call"
                    );
                }
            }
        }
    }
    let discovered = funcs.len() - before;
    debug!(discovered, total = funcs.len(), "call-site scan complete");

    funcs
}
