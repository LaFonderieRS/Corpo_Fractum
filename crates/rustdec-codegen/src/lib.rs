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

/// Emit pseudo-code for every function in `module`.
/// Returns `Vec<(function_name, source_code)>`.
#[instrument(skip(module), fields(lang = ?lang, functions = module.functions.len()))]
pub fn emit_module(
    module: &IrModule,
    lang:   Language,
) -> CodegenResult<Vec<(String, String)>> {
    info!("emitting {:?} code for {} functions", lang, module.functions.len());

    let mut results = Vec::with_capacity(module.functions.len());

    for func in &module.functions {
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
