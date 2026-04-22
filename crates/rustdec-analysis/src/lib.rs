//! # rustdec-analysis
//!
//! Static analysis passes: function detection, CFG construction,
//! instruction lifting, and CFG structuration.

pub mod callgraph;
pub mod cfg;
pub mod dominance;
pub mod functions;
pub mod structure;
pub mod string_recovery;

pub use callgraph::{build_call_graph, CallGraph};
pub use cfg::build_cfg;
pub use dominance::{DomTree, NaturalLoop, find_natural_loops, find_convergence};
pub use functions::detect_functions;
pub use structure::{structure_function, StructuredFunc, SNode, CondExpr};
pub use string_recovery::{RecoveredString, StringEncoding, recover_strings_from_binary, recover_strings_with_cfg};

use rustdec_ir::IrModule;
use rustdec_loader::{BinaryObject, build_symbol_map, extract_strings};
use thiserror::Error;
use tracing::{debug, info, instrument, warn};

#[derive(Debug, Error)]
pub enum AnalysisError {
    #[error("Disassembly error: {0}")]
    Disasm(#[from] rustdec_disasm::DisasmError),
    #[error("No executable section found in binary")]
    NoCodeSection,
}

pub type AnalysisResult<T> = Result<T, AnalysisError>;

/// Run the full analysis pipeline on a [`BinaryObject`] and return an [`IrModule`].
///
/// Pipeline:
/// 1. Disassemble all code sections **and** extract strings in parallel (rayon).
/// 2. Detect function entry points (symbols + call-site scan).
/// 3. Build per-function CFG (basic blocks + edges) **and** lift each block
///    in parallel — one rayon task per function.
#[instrument(skip(obj), fields(arch = %obj.arch, format = ?obj.format))]
pub fn analyse(obj: &BinaryObject) -> AnalysisResult<IrModule> {
    use rayon::prelude::*;
    use rustdec_disasm::Disassembler;
    use rustdec_lift::lift_function;
    use std::collections::HashMap;
    use std::time::Instant;

    info!("starting analysis — arch={}, format={:?}", obj.arch, obj.format);
    let t0 = Instant::now();

    // ── 1. Disassemble + extract strings (parallel) ───────────────────────────
    let arch = obj.arch;
    let (disasm_result, string_table) = rayon::join(
        || -> AnalysisResult<_> {
            let disasm = Disassembler::for_arch(arch)?;
            let mut all_insns = vec![];
            let mut global_end: u64 = 0;
            for section in obj.code_sections() {
                debug!(section = %section.name,
                       size    = section.data.len(),
                       va      = format_args!("{:#x}", section.virtual_addr),
                       "disassembling section");
                let insns = disasm.disassemble(&section.data, section.virtual_addr)?;
                debug!(section = %section.name, count = insns.len(), "section disassembled");
                if let Some(last) = insns.last() {
                    let end = last.address + last.size as u64;
                    if end > global_end { global_end = end; }
                }
                all_insns.extend(insns);
            }
            if all_insns.is_empty() {
                warn!("no instructions found — binary has no executable sections");
                return Err(AnalysisError::NoCodeSection);
            }
            all_insns.sort_by_key(|i| i.address);
            Ok((all_insns, global_end))
        },
        || extract_strings(obj),
    );

    let (all_insns, global_end) = disasm_result?;
    info!(total_instructions = all_insns.len(), "disassembly complete");
    info!(strings = string_table.len(), "string table built");

    let symbol_map = build_symbol_map(obj, &string_table);
    info!(symbols = symbol_map.len(), "symbol map built");

    // ── 2. Detect functions ───────────────────────────────────────────────────
    let entry_points = detect_functions(obj, &all_insns);
    info!(functions = entry_points.len(), "function detection complete");

    // ── 3. Build CFGs + 4. Lift (parallel) ───────────────────────────────────
    let sorted_entries: Vec<(u64, &String)> =
        entry_points.iter().map(|(addr, name)| (*addr, name)).collect();

    debug!(funcs      = sorted_entries.len(),
           global_end = format_args!("{:#x}", global_end),
           "computing function boundaries");

    // Build an O(1) address → slice-index map once, shared across all tasks.
    let addr_to_idx: HashMap<u64, usize> = all_insns
        .iter()
        .enumerate()
        .map(|(i, insn)| (insn.address, i))
        .collect();

    // Precompute (entry, end, name) tuples, filtering out degenerate ranges.
    let func_ranges: Vec<(u64, u64, String)> = sorted_entries
        .iter()
        .enumerate()
        .filter_map(|(i, &(entry_addr, name))| {
            let end_addr = sorted_entries.get(i + 1)
                .map(|(a, _)| *a)
                .unwrap_or(global_end);
            if end_addr <= entry_addr {
                warn!(func = %name, "end_addr <= entry_addr — skipping");
                None
            } else {
                Some((entry_addr, end_addr, name.clone()))
            }
        })
        .collect();

    let functions: Vec<_> = func_ranges
        .par_iter()
        .map(|(entry_addr, end_addr, name)| {
            let (entry_addr, end_addr) = (*entry_addr, *end_addr);

            debug!(func  = %name,
                   entry = format_args!("{:#x}", entry_addr),
                   end   = format_args!("{:#x}", end_addr),
                   bytes = end_addr - entry_addr,
                   "building CFG");

            let mut func = build_cfg(name.clone(), entry_addr, end_addr, &all_insns, &addr_to_idx);

            debug!(func   = %name,
                   blocks = func.cfg.node_count(),
                   edges  = func.cfg.edge_count(),
                   "CFG complete — lifting");

            lift_function(&mut func, &all_insns, &symbol_map);

            let total_stmts: usize = func.blocks_sorted()
                .iter()
                .map(|b| b.stmts.len())
                .sum();
            info!(func   = %name,
                  blocks = func.cfg.node_count(),
                  edges  = func.cfg.edge_count(),
                  stmts  = total_stmts,
                  ret_ty = ?func.ret_ty,
                  "function ready");

            func
        })
        .collect();

    let mut module = IrModule::default();
    module.functions = functions;
    
    // Use enhanced string recovery with DWARF if available
    let string_table = if let Some(ref dwarf) = obj.dwarf {
        debug!("Using DWARF-enhanced string recovery");
        string_recovery::recover_strings_with_dwarf(obj, dwarf)
            .into_iter()
            .map(|s| (s.address, s.content))
            .collect()
    } else {
        string_table
    };
    
    module.string_table = string_table;

    let elapsed = t0.elapsed();
    info!(
        functions    = module.functions.len(),
        instructions = all_insns.len(),
        elapsed_ms   = elapsed.as_millis(),
        "analysis finished"
    );

    Ok(module)
}
