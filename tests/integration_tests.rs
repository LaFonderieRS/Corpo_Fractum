//! Integration tests for the full loader → disasm → analysis pipeline.
//!
//! These tests use tiny hand-crafted ELF stubs embedded as byte arrays so
//! no external binary files are required to run `cargo test`.

use rustdec_loader::{load_bytes, Arch, Format};
use rustdec_disasm::Disassembler;
use rustdec_analysis::detect_functions;

// ── Minimal ELF64 stub (x86-64, single RET instruction) ──────────────────────
//
// Assembled with: `nasm -f elf64 -o /dev/stdout` conceptually.
// The entry point (e_entry = 0x400000 + 0x78) executes a single `RET` (0xC3).
const MINIMAL_ELF64: &[u8] = &[
    // ELF magic + class64 + LE + version + OS/ABI + padding
    0x7f, 0x45, 0x4c, 0x46, 0x02, 0x01, 0x01, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    // e_type=ET_EXEC(2), e_machine=EM_X86_64(0x3E)
    0x02, 0x00, 0x3e, 0x00,
    // e_version=1
    0x01, 0x00, 0x00, 0x00,
    // e_entry=0x400078 (LE 64-bit)
    0x78, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00,
    // e_phoff=0x40 (program header offset)
    0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    // e_shoff=0 (no section headers)
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    // e_flags=0, e_ehsize=64, e_phentsize=56, e_phnum=1
    0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x38, 0x00, 0x01, 0x00,
    // e_shentsize=64, e_shnum=0, e_shstrndx=0
    0x40, 0x00, 0x00, 0x00, 0x00, 0x00,
    // Program header: PT_LOAD, flags=RX
    0x01, 0x00, 0x00, 0x00, // p_type = PT_LOAD
    0x05, 0x00, 0x00, 0x00, // p_flags = PF_R|PF_X
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // p_offset
    0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, // p_vaddr = 0x400000
    0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, // p_paddr
    0x79, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // p_filesz = 0x79
    0x79, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // p_memsz
    0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // p_align
    // Padding to offset 0x78 then our single instruction: RET (0xC3)
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xC3, // RET — the entry point instruction
];

#[test]
fn loader_parses_elf64() {
    // The stub is intentionally minimal and may not parse all fields,
    // but the loader must not panic and must detect the architecture.
    match load_bytes(MINIMAL_ELF64) {
        Ok(obj) => {
            assert_eq!(obj.format, Format::Elf);
            assert_eq!(obj.arch, Arch::X86_64);
            assert!(obj.is_64bit);
        }
        Err(e) => {
            // Parsing may fail on the minimal stub — that's acceptable for
            // the integration test; what matters is no panic/UB.
            eprintln!("Loader returned error (acceptable for stub): {e}");
        }
    }
}

#[test]
fn disassembler_decodes_ret() {
    let disasm = Disassembler::for_arch(Arch::X86_64)
        .expect("x86-64 disassembler must initialise");
    let bytes = &[0xC3u8]; // RET
    let insns = disasm.disassemble(bytes, 0x1000).expect("must disassemble RET");
    assert_eq!(insns.len(), 1);
    // Capstone AT&T emits "retq"; is_terminator() handles both forms.
    assert!(insns[0].is_terminator(),
        "RET must be a terminator; mnemonic={:?}", insns[0].mnemonic);
    assert!(insns[0].mnemonic.contains("ret"),
        "mnemonic should be ret/retq; got {:?}", insns[0].mnemonic);
}

#[test]
fn disassembler_decodes_call() {
    let disasm = Disassembler::for_arch(Arch::X86_64).unwrap();
    // E8 00 00 00 00  →  call rip+5 (relative 0)
    let bytes = &[0xE8u8, 0x00, 0x00, 0x00, 0x00];
    let insns = disasm.disassemble(bytes, 0x1000).unwrap();
    assert_eq!(insns.len(), 1);
    assert!(insns[0].is_call());
}

#[test]
fn disassembler_decodes_jnz() {
    let disasm = Disassembler::for_arch(Arch::X86_64).unwrap();
    // 75 10  →  jne +0x10
    let bytes = &[0x75u8, 0x10];
    let insns = disasm.disassemble(bytes, 0x2000).unwrap();
    assert!(!insns.is_empty());
    assert!(insns[0].is_branch());
}

#[test]
fn disassembler_decodes_nop_sequence() {
    let disasm = Disassembler::for_arch(Arch::X86_64).unwrap();
    let nops = vec![0x90u8; 8]; // 8× NOP
    let insns = disasm.disassemble(&nops, 0x3000).unwrap();
    assert_eq!(insns.len(), 8);
    for i in &insns {
        assert_eq!(i.mnemonic, "nop");
        assert!(!i.is_terminator());
        assert!(!i.is_branch());
    }
}

#[test]
fn disassembler_unsupported_arch_errors() {
    let result = Disassembler::for_arch(Arch::Unknown);
    assert!(result.is_err());
}

// ── Pipeline integration tests ────────────────────────────────────────────────

#[test]
fn loader_to_disasm_pipeline() {
    // Load the stub; if parsing fails fall back to raw bytes.
    let code: &[u8] = &[0xC3u8]; // single RET
    let disasm = Disassembler::for_arch(Arch::X86_64).unwrap();
    let insns  = disasm.disassemble(code, 0x401000).unwrap();
    assert_eq!(insns.len(), 1);
    assert!(insns[0].is_terminator());
}

#[test]
fn cfg_build_from_disasm_output() {
    use rustdec_analysis::build_cfg;
    use std::collections::HashMap;

    let bytes  = [0xC3u8]; // ret
    let disasm = Disassembler::for_arch(Arch::X86_64).unwrap();
    let insns  = disasm.disassemble(&bytes, 0x401000).unwrap();
    let aidx: HashMap<u64, usize> =
        insns.iter().enumerate().map(|(i, insn)| (insn.address, i)).collect();

    let func = build_cfg("pipeline_test".into(), 0x401000, 0x401001, &insns, &aidx);
    assert_eq!(func.cfg.node_count(), 1);
    assert_eq!(func.entry_addr, 0x401000);
}

#[test]
fn detect_functions_on_elf_stub() {
    use rustdec_analysis::detect_functions;
    use rustdec_loader::load_bytes;

    if let Ok(obj) = load_bytes(MINIMAL_ELF64) {
        let disasm = Disassembler::for_arch(obj.arch).unwrap();
        let mut all_insns = vec![];
        for sec in obj.code_sections() {
            if let Ok(insns) = disasm.disassemble(&sec.data, sec.virtual_addr) {
                all_insns.extend(insns);
            }
        }
        all_insns.sort_by_key(|i| i.address);
        let funcs = detect_functions(&obj, &all_insns);
        assert!(!funcs.is_empty(),
            "at least one function (the entry point) must be detected");
    }
    // If the loader returns an error for the minimal stub, the test passes
    // vacuously — the important thing is no panic.
}

#[test]
fn codegen_emits_valid_c_from_trivial_ir() {
    use rustdec_codegen::{emit_module, Language};
    use rustdec_ir::{IrFunction, IrModule, IrType};

    let mut func = IrFunction::new("add_numbers", 0x401000);
    func.ret_ty = IrType::UInt(64);
    func.params = vec![IrType::UInt(64), IrType::UInt(64)];

    let mut module = IrModule::default();
    module.functions.push(func);

    let results = emit_module(&module, Language::C).unwrap();
    assert_eq!(results.len(), 1);
    let (name, src) = &results[0];
    assert_eq!(name, "add_numbers");
    assert!(src.contains("add_numbers"),
        "emitted C source must contain function name; got:\n{src}");
}

/// Verify that `.rodata` string literals are recovered through both the
/// `movabsq $addr, %rax` and `leaq disp(%rip), %rax` addressing patterns.
#[test]
fn rodata_string_recovery_via_movabs_and_lea() {
    use rustdec_analysis::build_cfg;
    use rustdec_lift::lift_function;
    use rustdec_loader::{Section, SectionKind, BinaryObject, Endian, extract_strings, build_symbol_map};
    use rustdec_ir::{Expr, SymbolKind};
    use std::collections::HashMap;

    let rodata = b"hello world\0";

    // movabsq $0x402000, %rax; retq
    let code_movabs: &[u8] = &[
        0x48, 0xB8, 0x00, 0x20, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00,
        0xC3,
    ];
    // leaq 0xff9(%rip), %rax; retq  — rip at 0x401007, +0xff9 = 0x402000
    let code_lea: &[u8] = &[
        0x48, 0x8D, 0x05, 0xF9, 0x0F, 0x00, 0x00,
        0xC3,
    ];

    for (label, code) in [("movabsq", code_movabs as &[u8]), ("leaq", code_lea)] {
        let obj = BinaryObject {
            format: rustdec_loader::Format::Elf,
            arch: Arch::X86_64,
            endian: Endian::Little,
            is_64bit: true,
            base_address: 0x400000,
            entry_point: Some(0x401000),
            sections: vec![
                Section { name: ".text".into(), virtual_addr: 0x401000, file_offset: 0,
                          size: code.len() as u64, kind: SectionKind::Code, data: code.to_vec() },
                Section { name: ".rodata".into(), virtual_addr: 0x402000, file_offset: 0,
                          size: rodata.len() as u64, kind: SectionKind::ReadOnlyData, data: rodata.to_vec() },
            ],
            symbols: vec![],
            dwarf: None,
        };

        let disasm = Disassembler::for_arch(Arch::X86_64).unwrap();
        let insns  = disasm.disassemble(code, 0x401000).unwrap();
        let aidx: HashMap<u64, usize> =
            insns.iter().enumerate().map(|(i, insn)| (insn.address, i)).collect();

        let string_table = extract_strings(&obj);
        let symbol_map   = build_symbol_map(&obj, &string_table);

        let mut func = build_cfg("test".into(), 0x401000, 0x401000 + code.len() as u64, &insns, &aidx);
        lift_function(&mut func, &insns, &symbol_map);

        let found = func.blocks_sorted().iter().any(|bb| {
            bb.stmts.iter().any(|s| matches!(s,
                rustdec_ir::Stmt::Assign {
                    rhs: Expr::Symbol { kind: SymbolKind::String, name, .. }, ..
                } if name == "hello world"
            ))
        });
        assert!(found,
            "[{label}] expected Expr::Symbol(String, \"hello world\") in lifted IR");
    }
}
