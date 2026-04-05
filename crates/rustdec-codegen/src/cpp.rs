//! C++17 pseudo-code backend.
//!
//! Delegates most logic to the C backend and overrides type names and
//! function signatures to use C++ conventions.

use rustdec_ir::{IrFunction, IrType};
use crate::{c::CBackend, CodegenBackend, CodegenResult};

pub struct CppBackend;

impl CodegenBackend for CppBackend {
    fn emit_function(&self, func: &IrFunction) -> CodegenResult<String> {
        // Reuse C backend output and prepend a C++ header comment.
        let c_src = CBackend.emit_function(func)?;
        Ok(format!(
            "// C++ (decompiled by RustDec)\n\
             #include <cstdint>\n\
             #include <cstddef>\n\n\
             {}",
            c_src.replacen("// RustDec decompilation", "// RustDec C++ decompilation", 1)
        ))
    }

    fn emit_type(&self, ty: &IrType) -> String {
        match ty {
            IrType::UInt(8)   => "std::uint8_t".into(),
            IrType::UInt(16)  => "std::uint16_t".into(),
            IrType::UInt(32)  => "std::uint32_t".into(),
            IrType::UInt(64)  => "std::uint64_t".into(),
            IrType::SInt(8)   => "std::int8_t".into(),
            IrType::SInt(16)  => "std::int16_t".into(),
            IrType::SInt(32)  => "std::int32_t".into(),
            IrType::SInt(64)  => "std::int64_t".into(),
            IrType::Float(32) => "float".into(),
            IrType::Float(64) => "double".into(),
            IrType::Ptr(inner) => format!("{}*", self.emit_type(inner)),
            IrType::Array { elem, len } => {
                format!("std::array<{}, {len}>", self.emit_type(elem))
            }
            IrType::Struct { name, .. } => name.clone(), // no `struct` keyword in C++
            IrType::Void    => "void".into(),
            _ => "auto /* unknown */".into(),
        }
    }
}
