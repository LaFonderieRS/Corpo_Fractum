//! Normalised binary representation, independent of the source format.

use crate::dwarf::DwarfInfo;
use crate::Arch;
use serde::{Deserialize, Serialize};

// ── Format / Endianness ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    Elf,
    Pe,
    MachO,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Endian {
    Little,
    Big,
}

// ── Section ───────────────────────────────────────────────────────────────────

/// Kind of a binary section, coarsely classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SectionKind {
    /// Executable machine code.
    Code,
    /// Read-only data (strings, constants, jump tables).
    ReadOnlyData,
    /// Initialised read-write data.
    Data,
    /// Uninitialised data (BSS).
    Bss,
    /// Debug information.
    Debug,
    /// Other / unknown.
    Other,
}

/// A contiguous region of the binary image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    pub name:         String,
    pub virtual_addr: u64,
    pub file_offset:  u64,
    pub size:         u64,
    pub kind:         SectionKind,
    /// Raw bytes, empty for Bss sections.
    pub data:         Vec<u8>,
}

impl Section {
    /// Return `true` if `va` falls inside this section's virtual address range.
    #[inline]
    pub fn contains_va(&self, va: u64) -> bool {
        va >= self.virtual_addr && va < self.virtual_addr.saturating_add(self.size)
    }
}

// ── Symbol ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Object,
    Import,
    Export,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub name:    String,
    pub address: u64,
    pub size:    u64,
    pub kind:    SymbolKind,
}

// ── BinaryObject ──────────────────────────────────────────────────────────────

/// The normalised result of loading a binary.
///
/// All addresses are virtual addresses as they appear in memory
/// (i.e. with base address applied for PIE binaries).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryObject {
    pub format:       Format,
    pub arch:         Arch,
    pub endian:       Endian,
    pub is_64bit:     bool,
    /// Preferred load address (0 for PIE / ASLR binaries).
    pub base_address: u64,
    /// Virtual address of the entry point, if known.
    pub entry_point:  Option<u64>,
    pub sections:     Vec<Section>,
    pub symbols:      Vec<Symbol>,
    /// DWARF debug information, if present in the binary.
    pub dwarf:        Option<DwarfInfo>,
}

impl BinaryObject {
    /// Return all sections that contain executable code.
    pub fn code_sections(&self) -> impl Iterator<Item = &Section> {
        self.sections.iter().filter(|s| s.kind == SectionKind::Code)
    }

    /// Look up a symbol by name (case-sensitive, first match).
    pub fn symbol_by_name(&self, name: &str) -> Option<&Symbol> {
        self.symbols.iter().find(|s| s.name == name)
    }

    /// Return the section that contains the given virtual address, if any.
    pub fn section_at(&self, va: u64) -> Option<&Section> {
        self.sections.iter().find(|s| s.contains_va(va))
    }
}
