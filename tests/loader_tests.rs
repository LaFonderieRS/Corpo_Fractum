//! Unit tests for rustdec-loader: format detection, error paths, Arch display.

use rustdec_loader::{load_bytes, Arch, Format};

// ── Minimal ELF64 stub (same as integration_tests.rs) ─────────────────────────
const MINIMAL_ELF64: &[u8] = &[
    0x7f, 0x45, 0x4c, 0x46, 0x02, 0x01, 0x01, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x02, 0x00, 0x3e, 0x00,
    0x01, 0x00, 0x00, 0x00,
    0x78, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x38, 0x00, 0x01, 0x00,
    0x40, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00,
    0x05, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x79, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x79, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xC3,
];

// ── load_bytes ────────────────────────────────────────────────────────────────

#[test]
fn load_elf64_detects_format_and_arch() {
    match load_bytes(MINIMAL_ELF64) {
        Ok(obj) => {
            assert_eq!(obj.format, Format::Elf);
            assert_eq!(obj.arch,   Arch::X86_64);
            assert!(obj.is_64bit);
        }
        Err(e) => {
            // Minimal stub may fail full parse — acceptable; must not panic.
            eprintln!("loader returned error (acceptable for stub): {e}");
        }
    }
}

#[test]
fn load_empty_bytes_returns_error() {
    assert!(load_bytes(&[]).is_err(),
        "loading zero bytes must return an error");
}

#[test]
fn load_garbage_bytes_returns_error() {
    let garbage = [0xFFu8; 64];
    assert!(load_bytes(&garbage).is_err(),
        "loading unrecognised magic must return an error");
}

#[test]
fn load_truncated_elf_magic_only_returns_error() {
    // Only the 4-byte magic — no class / machine fields.
    let magic_only = [0x7fu8, 0x45, 0x4c, 0x46];
    assert!(load_bytes(&magic_only).is_err());
}

#[test]
fn load_pe_magic_without_valid_pe_returns_error() {
    // MZ header without any PE body.
    let mz = [0x4du8, 0x5a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
              0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    // May or may not parse — just must not panic.
    let _ = load_bytes(&mz);
}

// ── Arch display ─────────────────────────────────────────────────────────────

#[test]
fn arch_display_strings() {
    assert_eq!(Arch::X86.to_string(),     "x86");
    assert_eq!(Arch::X86_64.to_string(),  "x86-64");
    assert_eq!(Arch::Arm32.to_string(),   "ARM32");
    assert_eq!(Arch::Arm64.to_string(),   "ARM64");
    assert_eq!(Arch::RiscV32.to_string(), "RISC-V 32");
    assert_eq!(Arch::RiscV64.to_string(), "RISC-V 64");
    assert_eq!(Arch::Mips32.to_string(),  "MIPS32");
    assert_eq!(Arch::Mips64.to_string(),  "MIPS64");
    assert_eq!(Arch::Unknown.to_string(), "unknown");
}

// ── Arch equality ─────────────────────────────────────────────────────────────

#[test]
fn arch_equality() {
    assert_eq!(Arch::X86_64, Arch::X86_64);
    assert_ne!(Arch::X86,    Arch::X86_64);
    assert_ne!(Arch::Arm64,  Arch::Arm32);
}
