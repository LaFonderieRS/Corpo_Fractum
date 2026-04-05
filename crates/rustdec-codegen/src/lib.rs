//! # rustdec-codegen
//!
//! Code generation backends.  Each backend implements [`CodegenBackend`] and
//! receives an [`IrFunction`] to turn into a pretty-printed source string.
//!
//! Current backends:
//! - [`c::CBackend`]   — C99 pseudo-code (MVP, priority)
//! - [`cpp::CppBackend`] — C++17 pseudo-code
//! - [`rust::RustBackend`] — Rust pseudo-code

pub mod c;
pub mod cpp;
pub mod rust;

use rustdec_ir::{IrFunction, IrModule};
use thiserror::Error;

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum CodegenError {
    #[error("Unsupported IR construct: {0}")]
    Unsupported(String),
    #[error("Internal codegen error: {0}")]
    Internal(String),
}

pub type CodegenResult<T> = Result<T, CodegenError>;

// ── Target language selector ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    C,
    Cpp,
    Rust,
}

// ── Trait ─────────────────────────────────────────────────────────────────────

/// A code generation backend: transforms one IR function into source text.
pub trait CodegenBackend {
    /// Emit source code for a single function.
    fn emit_function(&self, func: &IrFunction) -> CodegenResult<String>;

    /// Emit a type name for display (e.g. in struct/variable declarations).
    fn emit_type(&self, ty: &rustdec_ir::IrType) -> String;
}

// ── Module-level helper ───────────────────────────────────────────────────────

/// Emit pseudo-code for every function in `module` using the requested language.
///
/// Returns a `Vec<(function_name, source_code)>`.
pub fn emit_module(
    module: &IrModule,
    lang: Language,
) -> CodegenResult<Vec<(String, String)>> {
    let results = module
        .functions
        .iter()
        .map(|func| {
            let src = match lang {
                Language::C    => c::CBackend.emit_function(func)?,
                Language::Cpp  => cpp::CppBackend.emit_function(func)?,
                Language::Rust => rust::RustBackend.emit_function(func)?,
            };
            Ok((func.name.clone(), src))
        })
        .collect::<CodegenResult<Vec<_>>>()?;
    Ok(results)
}
