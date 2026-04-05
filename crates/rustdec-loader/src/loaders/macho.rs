//! Mach-O loader — uses goblin::mach.

use goblin::mach::{segment::Section as MachSection, MachO, MultiArch};
use crate::{
    binary::{BinaryObject, Endian, Format, Section, SectionKind, Symbol, SymbolKind},
    Arch, LoadError, LoadResult,
};

pub fn load(macho: &MachO<'_>, _bytes: &[u8]) -> LoadResult<BinaryObject> {
    let arch = detect_arch_macho(macho.header.cputype);
    let is_64bit = macho.is_64;

    let mut sections = vec![];
    for seg in macho.segments.iter() {
        for (s, data) in seg.sections().unwrap_or_default() {
            sections.push(convert_section(&s, data.to_vec()));
        }
    }

    // Symbols
    let mut symbols = vec![];
    for sym_res in macho.symbols() {
        let (name, nlist) = sym_res.map_err(LoadError::Parse)?;
        if name.is_empty() {
            continue;
        }
        symbols.push(Symbol {
            name: name.trim_start_matches('_').to_string(),
            address: nlist.n_value,
            size: 0,
            kind: if nlist.is_stab() {
                SymbolKind::Other
            } else {
                SymbolKind::Function
            },
        });
    }

    let entry_point = Some(macho.entry);

    Ok(BinaryObject {
        format: Format::MachO,
        arch,
        endian: Endian::Little,
        is_64bit,
        base_address: 0,
        entry_point,
        sections,
        symbols,
    })
}

/// Pick the best slice from a fat binary (prefer arm64, then x86_64).
pub fn load_fat(fat: &MultiArch<'_>, bytes: &[u8]) -> LoadResult<BinaryObject> {
    let preferred = [0x0100_000C, 0x0100_0007]; // arm64, x86_64

    for arch_res in fat.iter_arches() {
        let arch = arch_res.map_err(LoadError::Parse)?;
        if preferred.contains(&(arch.cputype() as u32)) {
            // Parse MachO from the Fat offset
            let macho = MachO::parse(bytes, arch.offset as usize).map_err(LoadError::Parse)?;
            return load(&macho, bytes);
        }
    }

    // fallback: first slice
    let arch = fat.iter_arches()
        .next()
        .ok_or(LoadError::UnknownFormat)?
        .map_err(LoadError::Parse)?;

    let macho = MachO::parse(bytes, arch.offset as usize).map_err(LoadError::Parse)?;
    load(&macho, bytes)
}

fn convert_section(s: &MachSection, data: Vec<u8>) -> Section {
    let name = format!(
        "{},{}",
        std::str::from_utf8(&s.segname).unwrap_or("").trim_end_matches('\0'),
        std::str::from_utf8(&s.sectname).unwrap_or("").trim_end_matches('\0'),
    );
    let kind = classify_section_macho(&name);

    Section {
        name,
        virtual_addr: s.addr,
        file_offset: s.offset as u64,
        size: s.size,
        kind,
        data,
    }
}

fn classify_section_macho(name: &str) -> SectionKind {
    if name.contains("__TEXT,__text") {
        return SectionKind::Code;
    }
    if name.contains("__TEXT,__const") || name.contains("__TEXT,__cstring") {
        return SectionKind::ReadOnlyData;
    }
    if name.contains("__DATA,__data") || name.contains("__DATA,__got") {
        return SectionKind::Data;
    }
    if name.contains("__DATA,__bss") || name.contains("__DATA,__common") {
        return SectionKind::Bss;
    }
    if name.contains("__DWARF") {
        return SectionKind::Debug;
    }
    SectionKind::Other
}

fn detect_arch_macho(cputype: u32) -> Arch {
    match cputype & 0x00FF_FFFF {
        7 => Arch::X86_64,
        6 => Arch::X86,
        12 => Arch::Arm64,
        _ => Arch::Unknown,
    }
}

