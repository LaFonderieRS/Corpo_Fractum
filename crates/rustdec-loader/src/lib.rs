//! # rustdec-loader
//!
//! Parses ELF, PE and Mach-O binaries into a normalized [`BinaryObject`].
//! All offsets and addresses are resolved to `u64` regardless of the source
//! bitness.  The caller receives a self-contained structure with no lifetime
//! ties to the original byte slice.

use std::path::Path;
use thiserror::Error;

// ── Public re-exports ────────────────────────────────────────────────────────

pub use arch::Arch;
pub use binary::{BinaryObject, Endian, Format, Section, SectionKind, Symbol, SymbolKind};

// ── Error ────────────────────────────────────────────────────────────────────

/// Errors that can occur during binary loading.
#[derive(Debug, Error)]
pub enum LoadError {
    #[error("I/O error reading {path}: {source}")]
    Io { path: String, source: std::io::Error },

    #[error("Unsupported or unrecognised binary format")]
    UnknownFormat,

    #[error("Parse error: {0}")]
    Parse(#[from] goblin::error::Error),

    #[error("Truncated binary: expected {expected} bytes at offset {offset}")]
    Truncated { offset: usize, expected: usize },
}

pub type LoadResult<T> = Result<T, LoadError>;

// ── Sub-modules ──────────────────────────────────────────────────────────────

mod arch;
mod binary;
mod loaders;

// ── Entry points ─────────────────────────────────────────────────────────────

/// Load a binary from disk.
///
/// ```no_run
/// let obj = rustdec_loader::load_file("/usr/bin/ls")?;
/// println!("arch = {:?}, {} sections", obj.arch, obj.sections.len());
/// # Ok::<(), rustdec_loader::LoadError>(())
/// ```
pub fn load_file(path: impl AsRef<Path>) -> LoadResult<BinaryObject> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|e| LoadError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    load_bytes(&bytes)
}


fn is_pe(bytes: &[u8]) -> bool {
    if bytes.len() < 0x40 {
        return false;
    }

    // 1. Signature DOS
    if &bytes[0..2] != b"MZ" {
        return false;
    }

    // 2. Lire e_lfanew (offset du header PE)
    let e_lfanew = u32::from_le_bytes([
        bytes[0x3C],
        bytes[0x3D],
        bytes[0x3E],
        bytes[0x3F],
    ]) as usize;

    // 3. Vérifier que l'offset est valide
    if e_lfanew + 4 > bytes.len() {
        return false;
    }

    // 4. Signature PE
    &bytes[e_lfanew..e_lfanew + 4] == b"PE\0\0"
}

fn is_elf(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && &bytes[0..4] == b"\x7FELF"
}

fn is_macho(bytes: &[u8]) -> bool {
    if bytes.len() < 4 {
        return false;
    }

    let magic = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);

    matches!(
        magic,
        0xFEEDFACE | // 32-bit
        0xFEEDFACF | // 64-bit
        0xCEFAEDFE | // swapped
        0xCFFAEDFE | // swapped 64
        0xCAFEBABE | // FAT
        0xBEBAFECA   // FAT swapped
    )
}



/// Load a binary from an in-memory byte slice.
pub fn load_bytes(bytes: &[u8]) -> LoadResult<BinaryObject> {
    if is_elf(bytes) {
        let elf = goblin::elf::Elf::parse(bytes)?;
        return loaders::elf::load(&elf, bytes);
    }

    if is_pe(bytes) {
        let pe = goblin::pe::PE::parse(bytes)?;
        return loaders::pe::load(&pe, bytes);
    }

    if is_macho(bytes) {
        let mach = goblin::mach::Mach::parse(bytes)?;
        return match mach {
            goblin::mach::Mach::Binary(m) => loaders::macho::load(&m, bytes),
            goblin::mach::Mach::Fat(fat)  => loaders::macho::load_fat(&fat, bytes),
        };
    }

    Err(LoadError::UnknownFormat)
}
