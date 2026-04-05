//! ELF loader — uses goblin::elf.

use goblin::elf::{self, Elf};
use goblin::elf::sym::STT_FUNC;
use tracing::warn;

use crate::{
    binary::{BinaryObject, Endian, Format, Section, SectionKind, Symbol, SymbolKind},
    Arch, LoadResult,
};

pub fn load(elf: &Elf<'_>, bytes: &[u8]) -> LoadResult<BinaryObject> {
    let arch = detect_arch(elf.header.e_machine);
    let endian = if elf.little_endian { Endian::Little } else { Endian::Big };
    let is_64bit = elf.is_64;
    let base_address = 0u64; // PIE: resolved at runtime
    let entry_point = if elf.header.e_entry != 0 {
        Some(elf.header.e_entry)
    } else {
        None
    };

    let sections = elf
        .section_headers
        .iter()
        .filter_map(|sh| {
            let name = elf.shdr_strtab.get_at(sh.sh_name).unwrap_or("").to_string();
            if name.is_empty() { return None; }
            let kind = classify_section_elf(&name, sh.sh_flags, sh.sh_type);
            let data = if sh.sh_type == elf::section_header::SHT_NOBITS || sh.sh_size == 0 {
                vec![]
            } else {
                let start = sh.sh_offset as usize;
                let end = start + sh.sh_size as usize;
                if end <= bytes.len() {
                    bytes[start..end].to_vec()
                } else {
                    warn!("Section '{}' references data outside file bounds", name);
                    vec![]
                }
            };
            Some(Section {
                name,
                virtual_addr: sh.sh_addr,
                file_offset: sh.sh_offset,
                size: sh.sh_size,
                kind,
                data,
            })
        })
        .collect();

    let symbols = elf
        .syms
        .iter()
        .filter_map(|sym| {
            let name = elf.strtab.get_at(sym.st_name)?.to_string();
            if name.is_empty() || sym.st_value == 0 { return None; }
            let kind = if sym.st_type() == STT_FUNC {
                SymbolKind::Function
            } else {
                SymbolKind::Object
            };
            Some(Symbol { name, address: sym.st_value, size: sym.st_size, kind })
        })
        .chain(elf.dynsyms.iter().filter_map(|sym| {
            let name = elf.dynstrtab.get_at(sym.st_name)?.to_string();
            if name.is_empty() { return None; }
            Some(Symbol {
                name,
                address: sym.st_value,
                size: sym.st_size,
                kind: SymbolKind::Import,
            })
        }))
        .collect();

    Ok(BinaryObject {
        format: Format::Elf,
        arch,
        endian,
        is_64bit,
        base_address,
        entry_point,
        sections,
        symbols,
    })
}

fn detect_arch(machine: u16) -> Arch {
    use goblin::elf::header::*;
    match machine {
        EM_386      => Arch::X86,
        EM_X86_64   => Arch::X86_64,
        EM_ARM      => Arch::Arm32,
        EM_AARCH64  => Arch::Arm64,
        EM_RISCV    => Arch::RiscV64, // refined later from EF flags
        EM_MIPS     => Arch::Mips32,
        _           => Arch::Unknown,
    }
}

fn classify_section_elf(name: &str, flags: u64, sh_type: u32) -> SectionKind {
    use goblin::elf::section_header::*;
    if sh_type == SHT_NOBITS { return SectionKind::Bss; }
    if flags & SHF_EXECINSTR as u64 != 0 { return SectionKind::Code; }
    match name {
        ".rodata" | ".rodata1" | ".eh_frame" | ".eh_frame_hdr" => SectionKind::ReadOnlyData,
        ".data" | ".data1" | ".got" | ".got.plt"               => SectionKind::Data,
        ".bss"                                                  => SectionKind::Bss,
        n if n.starts_with(".debug") || n.starts_with(".zdebug") => SectionKind::Debug,
        _                                                       => SectionKind::Other,
    }
}
