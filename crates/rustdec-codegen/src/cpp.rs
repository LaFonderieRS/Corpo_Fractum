//! C++17 pseudo-code backend.
//!
//! Delegates most logic to the C backend and overrides type names to use
//! C++ STL conventions (`std::uint64_t`, `std::array`, etc.).

use rustdec_ir::{IrFunction, IrType};
use tracing::{debug, warn};

use crate::{c::CBackend, CodegenBackend, CodegenResult};

pub struct CppBackend;

impl CodegenBackend for CppBackend {
    fn emit_function(&self, func: &IrFunction) -> CodegenResult<String> {
        debug!(func = %func.name, "C++: delegating to C backend");

        let c_src = CBackend { string_table: std::collections::HashMap::new() }.emit_function(func)?;

        // Prepend C++ header and retag the first comment line.
        let src = format!(
            "// C++ (decompiled by RustDec)\n\
             #include <cstdint>\n\
             #include <array>\n\n\
             {}",
            c_src.replacen(
                "// RustDec decompilation",
                "// RustDec C++ decompilation",
                1,
            )
        );

        debug!(func = %func.name, lines = src.lines().count(), "C++: function emitted");
        Ok(src)
    }

    fn emit_type(&self, ty: &IrType) -> String {
        match ty {
            IrType::UInt(8)             => "std::uint8_t".into(),
            IrType::UInt(16)            => "std::uint16_t".into(),
            IrType::UInt(32)            => "std::uint32_t".into(),
            IrType::UInt(64)            => "std::uint64_t".into(),
            IrType::SInt(8)             => "std::int8_t".into(),
            IrType::SInt(16)            => "std::int16_t".into(),
            IrType::SInt(32)            => "std::int32_t".into(),
            IrType::SInt(64)            => "std::int64_t".into(),
            IrType::Float(32)           => "float".into(),
            IrType::Float(64)           => "double".into(),
            IrType::Ptr(inner)          => format!("{}*", self.emit_type(inner)),
            IrType::Array { elem, len } => format!("std::array<{}, {len}>", self.emit_type(elem)),
            IrType::Struct { name, .. } => name.clone(), // no `struct` keyword in C++
            IrType::Void                => "void".into(),
            other => {
                warn!(ty = ?other, "C++: unknown type — falling back to auto");
                "auto /* unknown */".into()
            }
        }
    }
}
