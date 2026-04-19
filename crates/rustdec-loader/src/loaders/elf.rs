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
        .chain(extract_plt_symbols(elf, bytes))
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
        dwarf: None,
    })
}


/// Extract PLT stub addresses by correlating `.plt` section with `.rela.plt`
/// relocation entries and the dynamic symbol table.
///
/// Each PLT entry `puts@plt` at address `0x400370` becomes a symbol
/// `Symbol { name: "puts", address: 0x400370, kind: Import }` so the
/// `SymbolMap` can resolve `sub_400370` → `puts`.
fn extract_plt_symbols(elf: &Elf<'_>, bytes: &[u8]) -> Vec<Symbol> {
    let mut syms = Vec::new();

    // Find the PLT section. Modern GCC with IBT/CET generates `.plt.sec`
    // (indirect branch tracking) where the actual stubs live, while the
    // old `.plt` only contains the resolver and GOT trampolines.
    // We prefer .plt.sec > .plt in that order.
    let plt_names = [".plt.sec", ".plt"];
    let plt_section = plt_names.iter().find_map(|&wanted| {
        elf.section_headers.iter().find(|sh| {
            elf.shdr_strtab.get_at(sh.sh_name)
                .map(|n| n == wanted)
                .unwrap_or(false)
        })
    });

    let plt = match plt_section {
        Some(s) => s,
        None    => return syms,
    };

    // Entry size: use sh_entsize when set, otherwise 16 (standard x86-64).
    let entry_size: u64 = if plt.sh_entsize > 0 { plt.sh_entsize } else { 16 };
    let plt_base = plt.sh_addr;

    // Determine stub index offset:
    // - .plt.sec: stubs start at index 0 (no resolver stub)
    // - classic .plt: entry 0 is the resolver, stubs start at index 1
    let is_plt_sec = elf.shdr_strtab
        .get_at(plt.sh_name)
        .map(|n| n == ".plt.sec")
        .unwrap_or(false);
    let stub_offset: u64 = if is_plt_sec { 0 } else { 1 };

    // Find .rela.plt section and parse it directly from bytes.
    // We avoid relying on elf.pltrelocs because goblin may not populate it
    // correctly for binaries with IBT (.plt.sec) or unusual section layouts.
    let rela_plt = elf.section_headers.iter().find(|sh| {
        elf.shdr_strtab.get_at(sh.sh_name)
            .map(|n| n == ".rela.plt")
            .unwrap_or(false)
    });

    let rela_plt = match rela_plt {
        Some(s) => s,
        None    => {
            // Fall back to goblin's pltrelocs if the section isn't found.
            for (rel_idx, rel) in elf.pltrelocs.iter().enumerate() {
                let sym_idx = rel.r_sym;
                let name = elf.dynstrtab
                    .get_at(elf.dynsyms.get(sym_idx).map(|s| s.st_name).unwrap_or(0))
                    .unwrap_or("")
                    .to_string();
                if name.is_empty() { continue; }
                let name = name.split('@').next().unwrap_or(&name).to_string();
                let stub_addr = plt_base + entry_size * (rel_idx as u64 + stub_offset);
                syms.push(Symbol { name, address: stub_addr, size: entry_size, kind: SymbolKind::Import });
            }
            return syms;
        }
    };

    // Each Rela64 entry is 24 bytes: r_offset(8) + r_info(8) + r_addend(8).
    // r_sym = r_info >> 32.
    let rela_entsize = if rela_plt.sh_entsize > 0 { rela_plt.sh_entsize as usize } else { 24 };
    let rela_offset  = rela_plt.sh_offset as usize;
    let rela_count   = (rela_plt.sh_size as usize) / rela_entsize;

    for rel_idx in 0..rela_count {
        let off = rela_offset + rel_idx * rela_entsize;
        if off + 16 > bytes.len() { break; }

        let r_info = u64::from_le_bytes(bytes[off+8..off+16].try_into().unwrap_or([0;8]));
        let sym_idx = (r_info >> 32) as usize;

        let name = elf.dynstrtab
            .get_at(elf.dynsyms.get(sym_idx).map(|s| s.st_name).unwrap_or(0))
            .unwrap_or("")
            .to_string();

        if name.is_empty() { continue; }

        // Strip version suffix e.g. "puts@GLIBC_2.2.5" → "puts".
        let name = name.split('@').next().unwrap_or(&name).to_string();

        let stub_addr = plt_base + entry_size * (rel_idx as u64 + stub_offset);

        syms.push(Symbol {
            name,
            address: stub_addr,
            size: entry_size,
            kind: SymbolKind::Import,
        });
    }

    syms
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
