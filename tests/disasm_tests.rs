//! Unit tests for rustdec-disasm: instruction decoding and classification.

use rustdec_disasm::{Disassembler, Instruction};
use rustdec_loader::Arch;

// ── Helper ────────────────────────────────────────────────────────────────────

fn disasm_x64() -> Disassembler {
    Disassembler::for_arch(Arch::X86_64).expect("x86-64 disassembler must initialise")
}

// ── Classification predicates ─────────────────────────────────────────────────

#[test]
fn ret_is_terminator_not_branch_not_call() {
    let d = disasm_x64();
    let insns = d.disassemble(&[0xC3], 0x1000).unwrap();
    assert_eq!(insns.len(), 1);
    // Capstone AT&T emits "retq"; is_terminator() must handle both "ret" and "retq".
    assert!(insns[0].is_terminator(),
        "ret must be a terminator; mnemonic={:?}", insns[0].mnemonic);
    assert!(!insns[0].is_branch());
    assert!(!insns[0].is_call());
}

#[test]
fn jmp_is_terminator_not_branch() {
    let d = disasm_x64();
    // EB 00  →  jmp +0 (short, relative)
    let insns = d.disassemble(&[0xEBu8, 0x00], 0x1000).unwrap();
    assert!(!insns.is_empty());
    assert_eq!(insns[0].mnemonic, "jmp");
    assert!(insns[0].is_terminator(), "jmp must be a terminator");
    assert!(!insns[0].is_branch(), "jmp is not a conditional branch");
}

#[test]
fn conditional_jne_is_branch_not_terminator() {
    let d = disasm_x64();
    // 75 10  →  jne +0x10
    let insns = d.disassemble(&[0x75u8, 0x10], 0x2000).unwrap();
    assert!(!insns.is_empty());
    assert!(insns[0].is_branch(),     "jne must be a branch");
    assert!(!insns[0].is_terminator(), "jne is not a terminator");
    assert!(!insns[0].is_call());
}

#[test]
fn call_is_call_not_terminator_not_branch() {
    let d = disasm_x64();
    // E8 00 00 00 00  →  callq (AT&T) / call (Intel)
    let insns = d.disassemble(&[0xE8u8, 0x00, 0x00, 0x00, 0x00], 0x1000).unwrap();
    assert_eq!(insns.len(), 1);
    // Capstone AT&T emits "callq"; is_call() must handle both "call" and "callq".
    assert!(insns[0].is_call(),
        "is_call() must return true; mnemonic={:?}", insns[0].mnemonic);
    assert!(!insns[0].is_terminator());
    assert!(!insns[0].is_branch());
}

#[test]
fn nop_is_none_of_the_above() {
    let d = disasm_x64();
    let insns = d.disassemble(&[0x90u8], 0x1000).unwrap();
    assert_eq!(insns.len(), 1);
    assert_eq!(insns[0].mnemonic, "nop");
    assert!(!insns[0].is_terminator());
    assert!(!insns[0].is_branch());
    assert!(!insns[0].is_call());
}

// ── branch_target ─────────────────────────────────────────────────────────────

fn fake_insn(mnemonic: &str, operands: &str) -> Instruction {
    Instruction {
        address:  0x1000,
        bytes:    vec![],
        mnemonic: mnemonic.to_string(),
        operands: operands.to_string(),
        size:     1,
    }
}

#[test]
fn branch_target_hex_prefix_address() {
    let insn = fake_insn("jmp", "0x401234");
    assert_eq!(insn.branch_target(), Some(0x401234));
}

#[test]
fn branch_target_uppercase_hex_prefix() {
    let insn = fake_insn("je", "0X40ABCD");
    assert_eq!(insn.branch_target(), Some(0x40ABCD));
}

#[test]
fn branch_target_indirect_register_is_none() {
    // AT&T: `jmpq *%rax` — no extractable immediate target.
    let insn = fake_insn("jmp", "*%rax");
    assert_eq!(insn.branch_target(), None);
}

#[test]
fn branch_target_small_address_below_threshold_is_none() {
    // Addresses < 0x1000 are treated as immediates, not code addresses.
    let insn = fake_insn("jmp", "0x100");
    assert_eq!(insn.branch_target(), None);
}

#[test]
fn branch_target_no_operands_is_none() {
    let insn = fake_insn("ret", "");
    assert_eq!(insn.branch_target(), None);
}

// ── Instruction::display ─────────────────────────────────────────────────────

#[test]
fn instruction_display_format() {
    let insn = fake_insn("ret", "");
    let s = insn.display();
    // Must contain the address in hex and the mnemonic.
    assert!(s.contains("0x00001000") || s.contains("1000"),
            "display must include address: got {s:?}");
    assert!(s.contains("ret"), "display must include mnemonic: got {s:?}");
}

// ── Instruction sizes ─────────────────────────────────────────────────────────

#[test]
fn ret_is_one_byte() {
    let d = disasm_x64();
    let insns = d.disassemble(&[0xC3u8], 0x1000).unwrap();
    assert_eq!(insns[0].size, 1);
    assert_eq!(insns[0].bytes, vec![0xC3]);
}

#[test]
fn call_rel32_is_five_bytes() {
    let d = disasm_x64();
    let insns = d.disassemble(&[0xE8u8, 0x00, 0x00, 0x00, 0x00], 0x1000).unwrap();
    assert_eq!(insns[0].size, 5);
}

#[test]
fn address_is_correct_for_each_instruction() {
    let d = disasm_x64();
    // Three NOPs starting at 0x2000; each should be at 0x2000, 0x2001, 0x2002.
    let nops = [0x90u8; 3];
    let insns = d.disassemble(&nops, 0x2000).unwrap();
    assert_eq!(insns.len(), 3);
    for (i, insn) in insns.iter().enumerate() {
        assert_eq!(insn.address, 0x2000 + i as u64,
            "instruction {i} must be at address {:#x}", 0x2000 + i);
    }
}

// ── Multi-instruction sequences ───────────────────────────────────────────────

#[test]
fn push_pop_sequence() {
    let d = disasm_x64();
    // 55  →  push %rbp  (AT&T: "pushq")
    // 5D  →  pop  %rbp  (AT&T: "popq")
    let insns = d.disassemble(&[0x55u8, 0x5D], 0x1000).unwrap();
    assert_eq!(insns.len(), 2);
    // Capstone AT&T emits "pushq" / "popq" — check prefix to be syntax-agnostic.
    assert!(insns[0].mnemonic.starts_with("push"),
        "expected push; got {:?}", insns[0].mnemonic);
    assert!(insns[1].mnemonic.starts_with("pop"),
        "expected pop; got {:?}", insns[1].mnemonic);
}

#[test]
fn unsupported_arch_returns_error() {
    let result = Disassembler::for_arch(Arch::Unknown);
    assert!(result.is_err(), "Unknown arch must produce an error");
}

// ── ARM64 support ─────────────────────────────────────────────────────────────

#[test]
fn arm64_disassembler_initialises() {
    assert!(Disassembler::for_arch(Arch::Arm64).is_ok(),
        "ARM64 disassembler must initialise");
}

#[test]
fn arm64_decodes_ret() {
    let d = Disassembler::for_arch(Arch::Arm64).unwrap();
    // C0 03 5F D6  →  ret  (AArch64)
    let insns = d.disassemble(&[0xC0u8, 0x03, 0x5F, 0xD6], 0x1000).unwrap();
    assert_eq!(insns.len(), 1);
    assert_eq!(insns[0].mnemonic, "ret");
    assert!(insns[0].is_terminator());
}
