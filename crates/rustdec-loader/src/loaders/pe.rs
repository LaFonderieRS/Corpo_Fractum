//! PE / PE+ loader — uses goblin::pe.

use goblin::pe::PE;
use tracing::warn;

use crate::{
    binary::{BinaryObject, Endian, Format, Section, SectionKind, Symbol, SymbolKind},
    Arch, LoadResult,
};

pub fn load(pe: &PE<'_>, bytes: &[u8]) -> LoadResult<BinaryObject> {
    let arch = if pe.is_64 { Arch::X86_64 } else { Arch::X86 };
    let image_base = pe.image_base as u64;

    let sections = pe
        .sections
        .iter()
        .map(|s| {
            let name = String::from_utf8_lossy(&s.name)
                .trim_end_matches('\0')
                .to_string();
            let characteristics = s.characteristics;
            let kind = classify_section_pe(&name, characteristics);
            let start = s.pointer_to_raw_data as usize;
            let size = s.size_of_raw_data as usize;
            let data = if size == 0 || start + size > bytes.len() {
                warn!("PE section '{}' data out of bounds", name);
                vec![]
            } else {
                bytes[start..start + size].to_vec()
            };
            Section {
                name,
                virtual_addr: image_base + s.virtual_address as u64,
                file_offset: s.pointer_to_raw_data as u64,
                size: s.virtual_size as u64,
                kind,
                data,
            }
        })
        .collect();

    let imports: Vec<Symbol> = pe
        .imports
        .iter()
        .map(|imp| Symbol {
            name: imp.name.to_string(),
            address: imp.rva as u64 + image_base,
            size: 0,
            kind: SymbolKind::Import,
        })
        .collect();

    let exports: Vec<Symbol> = pe
        .exports
        .iter()
        .filter_map(|exp| {
            let name = exp.name?.to_string();
            Some(Symbol {
                name,
                address: exp.rva as u64 + image_base,
                size: 0,
                kind: SymbolKind::Export,
            })
        })
        .collect();

    let symbols = imports.into_iter().chain(exports).collect();

    Ok(BinaryObject {
        format: Format::Pe,
        arch,
        endian: Endian::Little,
        is_64bit: pe.is_64,
        base_address: image_base,
        entry_point: Some(pe.entry as u64 + image_base),
        sections,
        symbols,
    })
}

fn classify_section_pe(name: &str, characteristics: u32) -> SectionKind {
    // IMAGE_SCN_CNT_CODE | IMAGE_SCN_MEM_EXECUTE
    if characteristics & 0x20000020 != 0 { return SectionKind::Code; }
    if characteristics & 0x80000000 != 0 { return SectionKind::Data; } // MEM_WRITE
    match name {
        ".rdata" | ".rsrc" | ".reloc" => SectionKind::ReadOnlyData,
        ".bss"                        => SectionKind::Bss,
        n if n.starts_with(".debug") => SectionKind::Debug,
        _                             => SectionKind::Other,
    }
}
