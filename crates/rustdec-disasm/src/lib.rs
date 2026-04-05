//! # rustdec-disasm
//!
//! Multi-architecture disassembler built on top of `capstone-rs`.

use capstone::prelude::*;
use rustdec_loader::Arch;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, instrument, trace, warn};

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum DisasmError {
    #[error("Architecture {0:?} is not yet supported")]
    UnsupportedArch(Arch),
    #[error("Capstone error: {0}")]
    Capstone(#[from] capstone::Error),
}

pub type DisasmResult<T> = Result<T, DisasmError>;

// ── Instruction ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instruction {
    pub address:  u64,
    pub bytes:    Vec<u8>,
    pub mnemonic: String,
    pub operands: String,
    pub size:     usize,
}

impl Instruction {
    pub fn display(&self) -> String {
        format!("{:#010x}  {:8}  {}", self.address, self.mnemonic, self.operands)
    }

    pub fn is_terminator(&self) -> bool {
        matches!(self.mnemonic.as_str(),
            "ret" | "retf" | "retn" | "jmp" | "ljmp" | "hlt" | "ud2" | "int3")
    }

    pub fn is_branch(&self) -> bool {
        let m = self.mnemonic.as_str();
        m.starts_with('j') && m != "jmp"
    }

    pub fn is_call(&self) -> bool {
        matches!(self.mnemonic.as_str(), "call" | "lcall")
    }

    /// Extract the direct branch/jump target address from operands.
    ///
    /// Capstone AT&T syntax may render targets as `0x40036c` (with prefix)
    /// or `400366` (bare hex, no prefix) depending on the version.
    /// We try the `0x`-prefix form first, then fall back to bare hex,
    /// and reject values that look like register names or small immediates
    /// that are almost certainly not code addresses (< 0x1000).
    pub fn branch_target(&self) -> Option<u64> {
        for tok in self.operands.split_whitespace() {
            // Strip leading * used by AT&T indirect syntax (e.g. `jmp *%rax`) —
            // these are indirect and have no extractable immediate target.
            let tok = tok.trim_start_matches('*');

            // Skip register references (start with % in AT&T).
            if tok.starts_with('%') {
                continue;
            }

            // Try "0x<hex>" form first.
            if let Some(hex) = tok.strip_prefix("0x").or_else(|| tok.strip_prefix("0X")) {
                // Remove any trailing punctuation (comma, closing paren, etc.)
                let hex = hex.trim_end_matches(|c: char| !c.is_ascii_hexdigit());
                if let Ok(addr) = u64::from_str_radix(hex, 16) {
                    if addr >= 0x1000 {
                        return Some(addr);
                    }
                }
            }

            // Try bare decimal — Capstone sometimes emits decimal for short jumps.
            // Only accept values that look like code addresses.
            if let Ok(addr) = tok.trim_end_matches(|c: char| !c.is_ascii_digit()).parse::<u64>() {
                if addr >= 0x1000 {
                    return Some(addr);
                }
            }
        }
        None
    }
}

// ── Disassembler ──────────────────────────────────────────────────────────────

pub struct Disassembler {
    cs:   Capstone,
    arch: Arch,
}

impl Disassembler {
    /// Build a disassembler for the given target architecture.
    #[instrument(skip_all, fields(arch = ?arch))]
    pub fn for_arch(arch: Arch) -> DisasmResult<Self> {
        debug!("initialising Capstone for {:?}", arch);
        let cs = match arch {
            Arch::X86 => Capstone::new()
                .x86().mode(arch::x86::ArchMode::Mode32)
                .syntax(arch::x86::ArchSyntax::Att).detail(true).build()?,
            Arch::X86_64 => Capstone::new()
                .x86().mode(arch::x86::ArchMode::Mode64)
                .syntax(arch::x86::ArchSyntax::Att).detail(true).build()?,
            Arch::Arm32 => Capstone::new()
                .arm().mode(arch::arm::ArchMode::Arm).detail(true).build()?,
            Arch::Arm64 => Capstone::new()
                .arm64().mode(arch::arm64::ArchMode::Arm).detail(true).build()?,
            Arch::RiscV64 => Capstone::new()
                .riscv().mode(arch::riscv::ArchMode::RiscV64).detail(true).build()?,
            other => {
                warn!(arch = ?other, "unsupported architecture");
                return Err(DisasmError::UnsupportedArch(other));
            }
        };
        debug!("Capstone initialised for {:?}", arch);
        Ok(Self { cs, arch })
    }

    /// Disassemble `bytes` starting at virtual address `base_addr`.
    #[instrument(skip(self, bytes), fields(arch = ?self.arch, base = format_args!("{:#x}", base_addr), len = bytes.len()))]
    pub fn disassemble(&self, bytes: &[u8], base_addr: u64) -> DisasmResult<Vec<Instruction>> {
        debug!("disassembling {} bytes at {:#x}", bytes.len(), base_addr);

        let insns = self.cs.disasm_all(bytes, base_addr)?;
        let result: Vec<Instruction> = insns.iter().map(|i| {
            let insn = Instruction {
                address:  i.address(),
                bytes:    i.bytes().to_vec(),
                size:     i.bytes().len(),
                mnemonic: i.mnemonic().unwrap_or("").to_string(),
                operands: i.op_str().unwrap_or("").to_string(),
            };
            trace!(
                addr    = format_args!("{:#x}", insn.address),
                mnem    = %insn.mnemonic,
                ops     = %insn.operands,
                size    = insn.size,
                "insn"
            );
            insn
        }).collect();

        let branches   = result.iter().filter(|i| i.is_branch()).count();
        let calls      = result.iter().filter(|i| i.is_call()).count();
        let terminators = result.iter().filter(|i| i.is_terminator()).count();

        debug!(
            total       = result.len(),
            branches    = branches,
            calls       = calls,
            terminators = terminators,
            "disassembly complete"
        );

        Ok(result)
    }
}
