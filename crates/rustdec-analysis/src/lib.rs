//! # rustdec-analysis
//!
//! Static analysis passes: function detection, CFG construction,
//! instruction lifting, and CFG structuration.

pub mod cfg;
pub mod functions;
pub mod structure;

pub use cfg::build_cfg;
pub use functions::detect_functions;
pub use structure::{structure_function, StructuredFunc, SNode, CondExpr};

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
///
/// Pipeline:
/// 1. Disassemble all code sections.
/// 2. Detect function entry points (symbols + call-site scan).
/// 3. Build per-function CFG (basic blocks + edges).
/// 4. **Lift** each block: ASM instructions → IR SSA statements + types.
#[instrument(skip(obj), fields(arch = %obj.arch, format = ?obj.format))]
pub fn analyse(obj: &BinaryObject) -> AnalysisResult<IrModule> {
    use rustdec_disasm::Disassembler;
    use rustdec_lift::lift_function;
    use std::time::Instant;

    info!("starting analysis — arch={}, format={:?}", obj.arch, obj.format);
    let t0 = Instant::now();

    let disasm = Disassembler::for_arch(obj.arch)?;

    // ── 1. Disassemble ────────────────────────────────────────────────────────
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
    info!(total_instructions = all_insns.len(), "disassembly complete");

    // ── 2. Detect functions ───────────────────────────────────────────────────
    let entry_points = detect_functions(obj, &all_insns);
    info!(functions = entry_points.len(), "function detection complete");

    // ── 3. Build CFGs + 4. Lift ───────────────────────────────────────────────
    let sorted_entries: Vec<(u64, &String)> =
        entry_points.iter().map(|(addr, name)| (*addr, name)).collect();

    debug!(funcs = sorted_entries.len(),
           global_end = format_args!("{:#x}", global_end),
           "computing function boundaries");

    let mut module = IrModule::default();

    for (i, &(entry_addr, name)) in sorted_entries.iter().enumerate() {
        let end_addr = sorted_entries.get(i + 1)
            .map(|(a, _)| *a)
            .unwrap_or(global_end);

        if end_addr <= entry_addr {
            warn!(func = %name, "end_addr <= entry_addr — skipping");
            continue;
        }

        debug!(func  = %name,
               entry = format_args!("{:#x}", entry_addr),
               end   = format_args!("{:#x}", end_addr),
               bytes = end_addr - entry_addr,
               "building CFG");

        let mut func = build_cfg(name.clone(), entry_addr, end_addr, &all_insns);

        debug!(func   = %name,
               blocks = func.cfg.node_count(),
               edges  = func.cfg.edge_count(),
               "CFG complete — lifting");

        // Lift: fill in IR stmts and infer types.
        lift_function(&mut func, &all_insns);

        let total_stmts: usize = func.blocks_sorted()
            .iter()
            .map(|b| b.stmts.len())
            .sum();
        info!(func       = %name,
              blocks      = func.cfg.node_count(),
              edges       = func.cfg.edge_count(),
              stmts       = total_stmts,
              ret_ty      = ?func.ret_ty,
              "function ready");

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
