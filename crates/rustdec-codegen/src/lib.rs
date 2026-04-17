//! # rustdec-codegen
//!
//! Code generation backends: C, C++, Rust.

pub mod c;
pub mod cpp;
pub mod libc_signatures;
pub mod rust;
pub mod syscalls;

use rustdec_ir::{IrFunction, IrModule};
use thiserror::Error;
use tracing::{debug, info, instrument, warn};

#[derive(Debug, Error)]
pub enum CodegenError {
    #[error("Unsupported IR construct: {0}")]
    Unsupported(String),
    #[error("Internal codegen error: {0}")]
    Internal(String),
}

pub type CodegenResult<T> = Result<T, CodegenError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    C,
    Cpp,
    Rust,
}

pub trait CodegenBackend {
    fn emit_function(&self, func: &IrFunction) -> CodegenResult<String>;
    fn emit_type(&self, ty: &rustdec_ir::IrType) -> String;
}

// ── CRT / runtime function filter ────────────────────────────────────────────

/// Functions injected by the linker or C runtime that have no value for the
/// reverse-engineer.  They are kept in the IR (for the call graph) but are
/// silently skipped during code generation.
const CRT_SKIP: &[&str] = &[
    // ELF initialisation / finalisation stubs.
    "_init",
    "_fini",
    "_start",
    // glibc CRT helpers.
    "__libc_start_main",
    "__libc_csu_init",
    "__libc_csu_fini",
    // GCC TM / global-destructor helpers.
    "__do_global_dtors_aux",
    "deregister_tm_clones",
    "register_tm_clones",
    "frame_dummy",
    // Dynamic linker relocations.
    "_dl_relocate_static_pie",
    // MSVC / MinGW equivalents.
    "__DllMainCRTStartup",
    "_DllMainCRTStartup",
    "mainCRTStartup",
    "WinMainCRTStartup",
];

/// Return `true` if `name` is a well-known CRT/runtime symbol that should be
/// excluded from decompiled output.
fn is_crt_function(name: &str) -> bool {
    CRT_SKIP.contains(&name)
}

/// Emit pseudo-code for every non-CRT function in `module`.
/// Returns `Vec<(function_name, source_code)>`.
#[instrument(skip(module), fields(lang = ?lang, functions = module.functions.len()))]
pub fn emit_module(
    module: &IrModule,
    lang:   Language,
) -> CodegenResult<Vec<(String, String)>> {
    info!("emitting {:?} code for {} functions", lang, module.functions.len());

    let mut results = Vec::with_capacity(module.functions.len());

    for func in &module.functions {
        if is_crt_function(&func.name) {
            debug!(func = %func.name, "skipping CRT function");
            continue;
        }

        debug!(func = %func.name, blocks = func.cfg.node_count(), "emitting function");

        let src = match lang {
            Language::C    => c::CBackend { string_table: module.string_table.clone() }.emit_function(func)?,
            Language::Cpp  => cpp::CppBackend.emit_function(func)?,
            Language::Rust => rust::RustBackend.emit_function(func)?,
        };

        let lines = src.lines().count();
        debug!(func = %func.name, lines, "function emitted");

        if src.contains("no blocks decoded") {
            warn!(func = %func.name, "function has no decoded blocks — output may be empty");
        }

        results.push((func.name.clone(), src));
    }

    info!(lang = ?lang, emitted = results.len(), "codegen complete");
    Ok(results)
}
