//! Target architecture enumeration.

use serde::{Deserialize, Serialize};

/// CPU architecture of the analysed binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Arch {
    X86,
    X86_64,
    Arm32,
    Arm64,
    RiscV32,
    RiscV64,
    Mips32,
    Mips64,
    Unknown,
}

impl std::fmt::Display for Arch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::X86     => "x86",
            Self::X86_64  => "x86-64",
            Self::Arm32   => "ARM32",
            Self::Arm64   => "ARM64",
            Self::RiscV32 => "RISC-V 32",
            Self::RiscV64 => "RISC-V 64",
            Self::Mips32  => "MIPS32",
            Self::Mips64  => "MIPS64",
            Self::Unknown => "unknown",
        };
        f.write_str(s)
    }
}
