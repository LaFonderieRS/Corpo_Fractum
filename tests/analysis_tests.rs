//! Unit tests for rustdec-analysis: CFG construction and function detection.

use rustdec_analysis::{build_cfg, detect_functions};
use rustdec_disasm::{Disassembler, Instruction};
use rustdec_ir::Terminator;
use rustdec_loader::{Arch, BinaryObject, Endian, Format, Section, SectionKind, Symbol};
use std::collections::HashMap;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn addr_map(insns: &[Instruction]) -> HashMap<u64, usize> {
    insns.iter().enumerate().map(|(i, insn)| (insn.address, i)).collect()
}

fn disasm(bytes: &[u8], base: u64) -> Vec<Instruction> {
    Disassembler::for_arch(Arch::X86_64)
        .unwrap()
        .disassemble(bytes, base)
        .unwrap()
}

/// Construct a minimal BinaryObject with a single executable section.
fn make_binary(code: Vec<u8>, base: u64, entry: u64) -> BinaryObject {
    BinaryObject {
        format:       Format::Elf,
        arch:         Arch::X86_64,
        endian:       Endian::Little,
        is_64bit:     true,
        base_address: base,
        entry_point:  Some(entry),
        sections: vec![Section {
            name:         ".text".into(),
            virtual_addr: base,
            file_offset:  0,
            size:         code.len() as u64,
            kind:         SectionKind::Code,
            data:         code,
        }],
        symbols: vec![],
        dwarf:   None,
    }
}

// ── build_cfg ─────────────────────────────────────────────────────────────────

#[test]
fn build_cfg_single_ret_produces_one_block() {
    let insns = disasm(&[0xC3], 0x401000);
    let aidx  = addr_map(&insns);

    let func = build_cfg("test".into(), 0x401000, 0x401001, &insns, &aidx);

    assert_eq!(func.name,        "test");
    assert_eq!(func.entry_addr,  0x401000);
    assert_eq!(func.cfg.node_count(), 1, "a single-RET function has exactly one block");
}

#[test]
fn build_cfg_entry_block_has_return_terminator() {
    let insns = disasm(&[0xC3], 0x401000);
    let aidx  = addr_map(&insns);

    let func = build_cfg("ret_fn".into(), 0x401000, 0x401001, &insns, &aidx);

    let blocks = func.blocks_sorted();
    assert_eq!(blocks.len(), 1);
    assert!(matches!(blocks[0].terminator, Terminator::Return(_)),
        "the only block of a RET function must have Return terminator");
}

#[test]
fn build_cfg_call_then_ret_gives_one_block() {
    // E8 00 00 00 00 → call rip+5 (no-op call: falls through)
    // C3             → ret
    let bytes = [0xE8u8, 0x00, 0x00, 0x00, 0x00, 0xC3];
    let insns = disasm(&bytes, 0x401000);
    let aidx  = addr_map(&insns);

    let func = build_cfg("call_ret".into(), 0x401000, 0x401006, &insns, &aidx);

    // No branch → single block.
    assert_eq!(func.cfg.node_count(), 1);
}

#[test]
fn build_cfg_unconditional_jmp_splits_blocks() {
    // EB 05  →  jmp +7 (short jump, jumps past the 3 nops and the ret below it)
    // 90 90 90 → nops (skipped target)
    // C3      → ret  (actual landing pad)
    let bytes = [0xEBu8, 0x05, 0x90, 0x90, 0x90, 0x90, 0x90, 0xC3];
    let insns = disasm(&bytes, 0x401000);
    let aidx  = addr_map(&insns);

    let end = 0x401000 + bytes.len() as u64;
    let func = build_cfg("jmp_fn".into(), 0x401000, end, &insns, &aidx);

    // The unconditional jmp must split the linear sequence into at least 2 blocks.
    assert!(func.cfg.node_count() >= 2,
        "jmp must produce at least 2 blocks, got {}",
        func.cfg.node_count());
}

#[test]
fn build_cfg_conditional_branch_splits_into_two_edges() {
    // 85 C0      →  test %eax, %eax
    // 74 01      →  je   +3  (→ 0x401005)
    // C3         →  ret      (→ 0x401004, fall-through)
    // C3         →  ret      (→ 0x401005, target)
    let bytes = [0x85u8, 0xC0, 0x74, 0x01, 0xC3, 0xC3];
    let insns = disasm(&bytes, 0x401000);
    let aidx  = addr_map(&insns);

    let end = 0x401000 + bytes.len() as u64;
    let func = build_cfg("branch_fn".into(), 0x401000, end, &insns, &aidx);

    // At minimum: [header block with je] + [fall-through ret] + [target ret].
    assert!(func.cfg.node_count() >= 2,
        "a conditional branch must produce at least 2 blocks, got {}",
        func.cfg.node_count());
    assert!(func.cfg.edge_count() >= 2,
        "a conditional branch must produce at least 2 CFG edges, got {}",
        func.cfg.edge_count());
}

// ── detect_functions ─────────────────────────────────────────────────────────

#[test]
fn detect_functions_finds_entry_point() {
    let code = vec![0xC3u8]; // single RET
    let obj  = make_binary(code.clone(), 0x400000, 0x400000);
    let insns = disasm(&code, 0x400000);

    let funcs = detect_functions(&obj, &insns);
    assert!(funcs.contains_key(&0x400000),
        "entry point 0x400000 must be detected as a function");
}

#[test]
fn detect_functions_finds_symbol_table_functions() {
    let code = vec![0xC3u8, 0xC3u8]; // two RET instructions
    let mut obj = make_binary(code.clone(), 0x400000, 0x400000);
    // Add a second function at 0x400001 via the symbol table.
    obj.symbols.push(Symbol {
        name:    "helper".into(),
        address: 0x400001,
        size:    1,
        kind:    rustdec_loader::SymbolKind::Function,
    });
    let insns = disasm(&code, 0x400000);

    let funcs = detect_functions(&obj, &insns);
    assert!(funcs.contains_key(&0x400001),
        "symbol-table function must be detected");
}

#[test]
fn detect_functions_finds_call_targets() {
    // call 0x40100A (E8 05 00 00 00 at 0x401000, next insn at 0x401005,
    //                so target = 0x401005 + 0 = 0x401005)
    //
    // More specifically: E8 rel32 where rel32 = target - (addr+5).
    // addr=0x401000, so for target=0x401008: rel32 = 0x401008-0x401005 = 3
    let bytes = [
        0xE8u8, 0x03, 0x00, 0x00, 0x00, // call 0x401008
        0xC3,                            // ret (at 0x401005)
        0x90, 0x90,                      // nop nop (padding)
        0xC3,                            // ret (at 0x401008 = detected callee)
    ];
    let insns = disasm(&bytes, 0x401000);
    let obj   = make_binary(bytes.to_vec(), 0x401000, 0x401000);

    let funcs = detect_functions(&obj, &insns);
    assert!(funcs.contains_key(&0x401008),
        "call target 0x401008 must be detected as a function entry point");
}

#[test]
fn detect_functions_empty_insns_returns_only_entry() {
    let obj = make_binary(vec![], 0x400000, 0x400000);
    let funcs = detect_functions(&obj, &[]);
    // Entry point is still recorded even without instruction data.
    assert!(funcs.contains_key(&0x400000));
}
