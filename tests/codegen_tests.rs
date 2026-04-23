//! Unit tests for rustdec-codegen: type emission, CRT filtering, module output.

use rustdec_codegen::{emit_module, CodegenBackend, Language};
use rustdec_codegen::c::CBackend;
use rustdec_ir::{IrFunction, IrModule, IrType};
use std::collections::HashMap;

fn c_backend() -> CBackend {
    CBackend { string_table: HashMap::new() }
}

fn empty_module() -> IrModule {
    IrModule::default()
}

fn module_with(func: IrFunction) -> IrModule {
    let mut m = IrModule::default();
    m.functions.push(func);
    m
}

fn trivial_func(name: &str) -> IrFunction {
    let mut f = IrFunction::new(name, 0x401000);
    f.ret_ty = IrType::Void;
    f
}

// ── CBackend::emit_type ───────────────────────────────────────────────────────

#[test]
fn c_emit_type_unsigned_integers() {
    let b = c_backend();
    assert_eq!(b.emit_type(&IrType::UInt(8)),  "char");
    assert_eq!(b.emit_type(&IrType::UInt(16)), "uint16_t");
    assert_eq!(b.emit_type(&IrType::UInt(32)), "uint32_t");
    assert_eq!(b.emit_type(&IrType::UInt(64)), "uint64_t");
}

#[test]
fn c_emit_type_signed_integers() {
    let b = c_backend();
    assert_eq!(b.emit_type(&IrType::SInt(8)),  "int8_t");
    assert_eq!(b.emit_type(&IrType::SInt(16)), "int16_t");
    assert_eq!(b.emit_type(&IrType::SInt(32)), "int");
    assert_eq!(b.emit_type(&IrType::SInt(64)), "int64_t");
}

#[test]
fn c_emit_type_floats() {
    let b = c_backend();
    assert_eq!(b.emit_type(&IrType::Float(32)), "float");
    assert_eq!(b.emit_type(&IrType::Float(64)), "double");
}

#[test]
fn c_emit_type_void() {
    assert_eq!(c_backend().emit_type(&IrType::Void), "void");
}

#[test]
fn c_emit_type_pointer_wraps_inner() {
    let b = c_backend();
    assert_eq!(b.emit_type(&IrType::ptr(IrType::UInt(8))),   "char*");
    assert_eq!(b.emit_type(&IrType::ptr(IrType::UInt(32))),  "uint32_t*");
    assert_eq!(b.emit_type(&IrType::ptr(IrType::Void)),      "void*");
}

#[test]
fn c_emit_type_struct() {
    let b = c_backend();
    let s = IrType::Struct { name: "MyStruct".into(), size: 16 };
    assert_eq!(b.emit_type(&s), "struct MyStruct");
}

#[test]
fn c_emit_type_array() {
    let b = c_backend();
    let arr = IrType::Array { elem: Box::new(IrType::UInt(32)), len: 8 };
    assert_eq!(b.emit_type(&arr), "uint32_t[8]");
}

// ── emit_module filtering ─────────────────────────────────────────────────────

#[test]
fn emit_module_on_empty_module_returns_empty_vec() {
    let results = emit_module(&empty_module(), Language::C).unwrap();
    assert!(results.is_empty());
}

#[test]
fn emit_module_filters_crt_functions() {
    let crt_names = [
        "_start", "_init", "_fini",
        "__libc_start_main", "__libc_csu_init", "__libc_csu_fini",
        "deregister_tm_clones", "register_tm_clones",
        "frame_dummy", "__do_global_dtors_aux",
    ];
    for name in &crt_names {
        let mut m = IrModule::default();
        m.functions.push(trivial_func(name));
        let results = emit_module(&m, Language::C)
            .unwrap_or_else(|e| panic!("emit_module failed for {name}: {e}"));
        assert!(results.is_empty(),
            "CRT function '{name}' must be filtered from output");
    }
}

#[test]
fn emit_module_emits_non_crt_function() {
    let m = module_with(trivial_func("my_function"));
    let results = emit_module(&m, Language::C).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "my_function");
}

#[test]
fn emit_module_output_contains_function_name() {
    let m = module_with(trivial_func("do_stuff"));
    let results = emit_module(&m, Language::C).unwrap();
    let src = &results[0].1;
    assert!(src.contains("do_stuff"),
        "emitted source must contain the function name; got:\n{src}");
}

#[test]
fn emit_module_output_contains_function_signature() {
    let m = module_with(trivial_func("widget"));
    let results = emit_module(&m, Language::C).unwrap();
    let src = &results[0].1;
    // A void function with no params must produce `void widget(void)`.
    assert!(src.contains("void") && src.contains("widget"),
        "emitted source must include return type and name; got:\n{src}");
}

#[test]
fn emit_module_mixed_skips_crt_keeps_user() {
    let mut m = IrModule::default();
    m.functions.push(trivial_func("_start"));     // CRT — filtered
    m.functions.push(trivial_func("user_main"));  // user — kept
    m.functions.push(trivial_func("_init"));      // CRT — filtered

    let results = emit_module(&m, Language::C).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "user_main");
}

// ── Language enum ─────────────────────────────────────────────────────────────

#[test]
fn language_variants_are_distinct() {
    assert_ne!(Language::C,   Language::Cpp);
    assert_ne!(Language::C,   Language::Rust);
    assert_ne!(Language::Cpp, Language::Rust);
}
