//! # rustdec-loader
//!
//! Parses ELF, PE and Mach-O binaries into a normalized [`BinaryObject`].

use goblin::mach::Mach;
use goblin::Object;
use std::path::Path;
use thiserror::Error;
use tracing::{debug, info, instrument, warn};

pub use arch::Arch;
pub use binary::{BinaryObject, Endian, Format, Section, SectionKind, Symbol, SymbolKind};
pub use dwarf::{
    CompUnit, DwarfField, DwarfFunction, DwarfInfo, DwarfLanguage, DwarfLocalVar, DwarfParam,
    DwarfType, DwarfTypeKind, LineEntry,
};
pub use strings::{extract_strings, StringTable};

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

mod arch;
mod binary;
pub mod dwarf;
mod loaders;
mod strings;

/// Load a binary from disk.
#[instrument(skip_all, fields(path = %path.as_ref().display()))]
pub fn load_file(path: impl AsRef<Path>) -> LoadResult<BinaryObject> {
    let path = path.as_ref();
    info!("opening binary: {}", path.display());

    let bytes = std::fs::read(path).map_err(|e| {
        LoadError::Io { path: path.display().to_string(), source: e }
    })?;

    info!("read {} bytes from disk", bytes.len());
    let obj = load_bytes(&bytes)?;

    info!(
        format   = ?obj.format,
        arch     = %obj.arch,
        is_64bit = obj.is_64bit,
        sections = obj.sections.len(),
        symbols  = obj.symbols.len(),
        entry    = obj.entry_point.map(|a| format!("{a:#x}")).unwrap_or_else(|| "none".into()),
        "binary loaded"
    );
    Ok(obj)
}

/// Load a binary from an in-memory byte slice.
#[instrument(skip(bytes), fields(len = bytes.len()))]
pub fn load_bytes(bytes: &[u8]) -> LoadResult<BinaryObject> {
    debug!("parsing {} bytes", bytes.len());

    let obj = match Object::parse(bytes)? {
        Object::Elf(elf) => {
            debug!("detected format: ELF");
            loaders::elf::load(&elf, bytes)
        }
        Object::PE(pe) => {
            debug!("detected format: PE");
            loaders::pe::load(&pe, bytes)
        }
        Object::Mach(mach) => match mach {
            Mach::Binary(m) => {
                debug!("detected format: Mach-O");
                loaders::macho::load(&m, bytes)
            }
            Mach::Fat(fat) => {
                debug!("detected format: Mach-O fat binary");
                loaders::macho::load_fat(&fat, bytes)
            }
        },
        Object::Archive(_) => {
            warn!("archive format is not supported");
            Err(LoadError::UnknownFormat)
        }
        Object::Unknown(magic) => {
            warn!(magic = magic, "unknown binary magic, cannot parse");
            Err(LoadError::UnknownFormat)
        }
        _ => Err(LoadError::UnknownFormat),
    }?;

    let dwarf = dwarf::parse(&obj);
    if let Some(ref d) = dwarf {
        debug!(
            units     = d.units.len(),
            functions = d.functions.len(),
            lines     = d.lines.len(),
            types     = d.types.len(),
            "DWARF debug info parsed"
        );
    }
    let obj = BinaryObject { dwarf, ..obj };

    for sec in &obj.sections {
        debug!(
            name   = %sec.name,
            va     = format_args!("{:#x}", sec.virtual_addr),
            size   = sec.size,
            kind   = ?sec.kind,
            "section"
        );
    }
    for sym in &obj.symbols {
        debug!(
            name = %sym.name,
            addr = format_args!("{:#x}", sym.address),
            kind = ?sym.kind,
            "symbol"
        );
    }

    Ok(obj)
}
