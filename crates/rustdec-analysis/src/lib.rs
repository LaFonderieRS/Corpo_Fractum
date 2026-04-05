//! # rustdec-analysis
//!
//! Static analysis passes: function detection, CFG construction.

pub mod cfg;
pub mod functions;

pub use cfg::build_cfg;
pub use functions::detect_functions;

use rustdec_ir::IrModule;
use rustdec_loader::BinaryObject;
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
#[instrument(skip(obj), fields(arch = %obj.arch, format = ?obj.format))]
pub fn analyse(obj: &BinaryObject) -> AnalysisResult<IrModule> {
    use rustdec_disasm::Disassembler;
    use std::time::Instant;

    info!("starting analysis — arch={}, format={:?}", obj.arch, obj.format);
    let t0 = Instant::now();

    let disasm = Disassembler::for_arch(obj.arch)?;

    // Disassemble all code sections.
    let mut all_insns = vec![];
    // Track the last address we can possibly reach (max end of any code section).
    let mut global_end: u64 = 0;

    for section in obj.code_sections() {
        debug!(section = %section.name,
               size    = section.data.len(),
               va      = format_args!("{:#x}", section.virtual_addr),
               "disassembling section");
        let insns = disasm.disassemble(&section.data, section.virtual_addr)?;
        debug!(section = %section.name, count = insns.len(), "section disassembled");

        // Update global_end to the end of the last instruction in this section.
        if let Some(last) = insns.last() {
            let end = last.address + last.size as u64;
            if end > global_end {
                global_end = end;
            }
        }
        all_insns.extend(insns);
    }

    if all_insns.is_empty() {
        warn!("no instructions found — binary has no executable sections");
        return Err(AnalysisError::NoCodeSection);
    }

    // Sort by address — required for binary search in build_cfg.
    all_insns.sort_by_key(|i| i.address);
    info!(total_instructions = all_insns.len(), "disassembly complete");

    // Detect function entry points.
    let entry_points = detect_functions(obj, &all_insns);
    info!(functions = entry_points.len(), "function detection complete");

    // ── Compute per-function boundaries ──────────────────────────────────────
    //
    // For each function, its end address is the entry point of the next
    // function in sorted order.  For the last function, we use `global_end`.
    //
    // This is the "next-function" heuristic — good enough for the MVP.
    // A future improvement would use call-graph reachability to handle
    // non-contiguous functions (tail-call optimisation, etc.).

    // BTreeMap iterates in key (address) order — no sort needed.
    // iter() yields (&u64, &String) — copy address, borrow name.
    let sorted_entries: Vec<(u64, &String)> =
        entry_points.iter().map(|(addr, name)| (*addr, name)).collect();

    debug!(funcs = sorted_entries.len(),
           global_end = format_args!("{:#x}", global_end),
           "computing function boundaries");

    let mut module = IrModule::default();

    for (i, &(entry_addr, name)) in sorted_entries.iter().enumerate() {
        // End address = next function's entry, or global_end for the last one.
        let end_addr = sorted_entries
            .get(i + 1)
            .map(|(next_entry, _)| *next_entry)
            .unwrap_or(global_end);

        debug!(func  = %name,
               entry = format_args!("{:#x}", entry_addr),
               end   = format_args!("{:#x}", end_addr),
               bytes = end_addr.saturating_sub(entry_addr),
               "building CFG");

        if end_addr <= entry_addr {
            warn!(func = %name,
                  entry = format_args!("{:#x}", entry_addr),
                  end   = format_args!("{:#x}", end_addr),
                  "end_addr <= entry_addr — skipping function");
            continue;
        }

        let func = build_cfg(name.clone(), entry_addr, end_addr, &all_insns);
        debug!(func   = %name,
               blocks = func.cfg.node_count(),
               edges  = func.cfg.edge_count(),
               "CFG complete");
        module.functions.push(func);
    }

    let elapsed = t0.elapsed();
    info!(
        functions    = module.functions.len(),
        instructions = all_insns.len(),
        elapsed_ms   = elapsed.as_millis(),
        "analysis finished"
    );

    Ok(module)
}
