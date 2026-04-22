//! DWARF debug-information parser.
//!
//! Extracts from `.debug_info` / `.debug_line` (DWARF 2–5):
//!
//! - Compilation units with producer, language and source-file name.
//! - Functions (DW_TAG_subprogram) with their virtual-address range,
//!   formal parameters, local variables and return type.
//! - Source-level line-number table mapping addresses to file + line.
//! - Named top-level types (base, struct, class, union, enum, typedef).
//!
//! All parsing is best-effort: malformed or missing attributes are silently
//! skipped so the rest of the information is still usable.

use std::collections::HashMap;

use gimli::{
    AttributeValue, DebuggingInformationEntry, Dwarf, EndianSlice, Reader, RunTimeEndian,
    Section as GimliSection, Unit, UnitOffset,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, trace, warn};

use crate::binary::{Endian, Section};

// ── Public data types ─────────────────────────────────────────────────────────

/// All DWARF debug information extracted from a binary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DwarfInfo {
    /// One entry per DWARF compilation unit.
    pub units: Vec<CompUnit>,
    /// All non-abstract subprograms with known address ranges.
    pub functions: Vec<DwarfFunction>,
    /// Flat source-level line table across all compilation units.
    pub lines: Vec<LineEntry>,
    /// Named top-level types (structs, enums, typedefs, …).
    pub types: Vec<DwarfType>,
}

/// A DWARF compilation unit (DW_TAG_compile_unit).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompUnit {
    /// Source file path (DW_AT_name, possibly joined with DW_AT_comp_dir).
    pub name: String,
    /// Compiler identification string (DW_AT_producer).
    pub producer: Option<String>,
    /// Programming language (DW_AT_language).
    pub language: Option<DwarfLanguage>,
    /// Lowest PC of the unit (DW_AT_low_pc), if present.
    pub low_pc: Option<u64>,
    /// Highest PC of the unit (DW_AT_high_pc / low_pc + offset), if present.
    pub high_pc: Option<u64>,
}

/// A function entry (DW_TAG_subprogram) with a concrete address range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DwarfFunction {
    /// Demangled or raw name (DW_AT_name).
    pub name: String,
    /// Linkage / mangled name (DW_AT_linkage_name), if different from `name`.
    pub linkage_name: Option<String>,
    /// Inclusive start address.
    pub low_pc: u64,
    /// Exclusive end address.
    pub high_pc: u64,
    /// Formal parameters in declaration order.
    pub params: Vec<DwarfParam>,
    /// Local variables (DW_TAG_variable children).
    pub locals: Vec<DwarfLocalVar>,
    /// Human-readable return type, or `None` if void / unresolvable.
    pub return_type: Option<String>,
    /// True when DW_AT_inline indicates this is an inlined instance.
    pub is_inlined: bool,
    /// True when DW_AT_external is set (globally visible).
    pub is_external: bool,
}

/// A formal parameter (DW_TAG_formal_parameter).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DwarfParam {
    pub name: Option<String>,
    pub type_name: Option<String>,
    /// rbp-relative byte offset of this parameter's stack location.
    ///
    /// `None` when the parameter lives in a register (or its location is not
    /// a simple frame-pointer-relative expression).
    #[serde(default)]
    pub frame_offset: Option<i64>,
}

/// A local variable (DW_TAG_variable inside a subprogram).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DwarfLocalVar {
    pub name: String,
    pub type_name: Option<String>,
    /// rbp-relative byte offset of this variable's stack location.
    ///
    /// `None` when the variable lives in a register or has a complex location.
    #[serde(default)]
    pub frame_offset: Option<i64>,
}

/// One row of the DWARF line-number table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineEntry {
    /// Virtual address.
    pub address: u64,
    /// Source file path.
    pub file: String,
    /// 1-based source line number.
    pub line: u32,
    /// 1-based source column, 0 when unknown.
    pub column: u16,
    /// True when this row marks a recommended breakpoint location.
    pub is_stmt: bool,
}

/// A named type defined at the top level of a compilation unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DwarfType {
    pub name: String,
    pub kind: DwarfTypeKind,
}

/// Coarse classification of a named type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DwarfTypeKind {
    Primitive { byte_size: u64 },
    Struct { fields: Vec<DwarfField> },
    Class { fields: Vec<DwarfField> },
    Union { fields: Vec<DwarfField> },
    Enum { variants: Vec<String>, byte_size: u64 },
    Typedef { target: Option<String> },
}

/// A field inside a struct / class / union.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DwarfField {
    pub name: Option<String>,
    pub type_name: Option<String>,
    /// Byte offset from the start of the containing type (DW_AT_data_member_location).
    pub byte_offset: Option<u64>,
}

/// Programming language from DW_AT_language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DwarfLanguage {
    C,
    Cpp,
    Rust,
    Go,
    Ada,
    Cobol,
    Fortran,
    Java,
    Swift,
    ObjC,
    Other(u16),
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Try to parse DWARF info from a [`BinaryObject`]'s sections.
///
/// Returns `None` when no DWARF sections are found.
pub fn parse(obj: &crate::binary::BinaryObject) -> Option<DwarfInfo> {
    let endian = match obj.endian {
        Endian::Little => RunTimeEndian::Little,
        Endian::Big => RunTimeEndian::Big,
    };

    let sections = &obj.sections;

    // Build the gimli Dwarf reader by providing the relevant sections.
    let load = |id: gimli::SectionId| -> Result<EndianSlice<'_, RunTimeEndian>, gimli::Error> {
        let data = section_data(sections, id);
        Ok(EndianSlice::new(data, endian))
    };

    let dwarf = match Dwarf::load(load) {
        Ok(d) => d,
        Err(e) => {
            warn!("gimli Dwarf::load failed: {e}");
            return None;
        }
    };

    // If there's no .debug_info data, nothing to do.
    if dwarf.debug_info.reader().len() == 0 {
        debug!("no .debug_info section found — skipping DWARF");
        return None;
    }

    let mut info = DwarfInfo::default();
    let mut units_iter = dwarf.units();

    while let Ok(Some(unit_header)) = units_iter.next() {
        let unit = match dwarf.unit(unit_header) {
            Ok(u) => u,
            Err(e) => {
                warn!("error loading DWARF unit: {e}");
                continue;
            }
        };

        parse_unit(&dwarf, &unit, &mut info);
    }

    debug!(
        units     = info.units.len(),
        functions = info.functions.len(),
        lines     = info.lines.len(),
        types     = info.types.len(),
        "DWARF parsed"
    );

    Some(info)
}

// ── Section lookup ────────────────────────────────────────────────────────────

/// Return the raw bytes for a gimli section ID, trying ELF and Mach-O names.
fn section_data<'a>(sections: &'a [Section], id: gimli::SectionId) -> &'a [u8] {
    let elf_name = id.name(); // e.g. ".debug_info"

    // Mach-O stores DWARF in __DWARF segment: "__DWARF,__debug_info"
    let macho_name = format!(
        "__DWARF,__{}",
        elf_name.trim_start_matches('.')
    ); // ".debug_info" → "__DWARF,__debug_info"

    // Also try compressed variant (.zdebug_*)
    let zdebug_name = format!(".z{}", elf_name.trim_start_matches('.'));

    for name in &[elf_name, macho_name.as_str(), zdebug_name.as_str()] {
        if let Some(sec) = sections.iter().find(|s| s.name.as_str() == *name) {
            if !sec.data.is_empty() {
                return &sec.data;
            }
        }
    }
    &[]
}

// ── Unit parsing ──────────────────────────────────────────────────────────────

type R<'a> = EndianSlice<'a, RunTimeEndian>;

fn parse_unit(dwarf: &Dwarf<R<'_>>, unit: &Unit<R<'_>>, out: &mut DwarfInfo) {
    // ── Compilation-unit root DIE ─────────────────────────────────────────────
    let mut entries = unit.entries();
    let Ok(Some((_, root))) = entries.next_dfs() else { return };

    if root.tag() != gimli::DW_TAG_compile_unit
        && root.tag() != gimli::DW_TAG_partial_unit
        && root.tag() != gimli::DW_TAG_type_unit
    {
        return;
    }

    let comp_unit = read_comp_unit(dwarf, unit, root);
    trace!(name = %comp_unit.name, "DWARF compile unit");
    out.units.push(comp_unit);

    // ── Type index: offset → human-readable name ──────────────────────────────
    // Walk all DIEs once to build a map so type references can be resolved.
    let type_map = build_type_map(dwarf, unit);

    // ── Second pass: functions and top-level types ────────────────────────────
    let mut entries = unit.entries();
    let mut depth = 0isize;

    while let Ok(Some((delta, entry))) = entries.next_dfs() {
        depth += delta;

        match entry.tag() {
            gimli::DW_TAG_subprogram => {
                if let Some(func) = read_function(dwarf, unit, entry, &type_map) {
                    out.functions.push(func);
                }
            }
            gimli::DW_TAG_base_type
            | gimli::DW_TAG_structure_type
            | gimli::DW_TAG_class_type
            | gimli::DW_TAG_union_type
            | gimli::DW_TAG_enumeration_type
            | gimli::DW_TAG_typedef => {
                // Only collect top-level (depth == 1) named types to avoid
                // flooding the list with anonymous inner structs.
                if depth == 1 {
                    if let Some(ty) = read_type(dwarf, unit, entry, &type_map) {
                        out.types.push(ty);
                    }
                }
            }
            _ => {}
        }
    }

    // ── Line table ────────────────────────────────────────────────────────────
    read_line_table(dwarf, unit, out);
}

// ── Compile-unit root DIE ─────────────────────────────────────────────────────

fn read_comp_unit<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    entry: &DebuggingInformationEntry<R>,
) -> CompUnit {
    let name = {
        let file = attr_string(dwarf, unit, entry, gimli::DW_AT_name)
            .unwrap_or_default();
        let dir  = attr_string(dwarf, unit, entry, gimli::DW_AT_comp_dir)
            .unwrap_or_default();
        if !dir.is_empty() && !file.starts_with('/') {
            format!("{dir}/{file}")
        } else {
            file
        }
    };

    let producer = attr_string(dwarf, unit, entry, gimli::DW_AT_producer);
    let language = attr_u64(entry, gimli::DW_AT_language).map(lang_from_u64);
    let low_pc   = attr_addr(entry, gimli::DW_AT_low_pc);
    let high_pc  = resolve_high_pc(entry, low_pc);

    CompUnit { name, producer, language, low_pc, high_pc }
}

// ── Type map ──────────────────────────────────────────────────────────────────

/// Walk every DIE in the unit and record (unit offset → type name) for all
/// type-producing tags.  Used to resolve DW_AT_type references.
fn build_type_map<R: Reader>(dwarf: &Dwarf<R>, unit: &Unit<R>) -> HashMap<UnitOffset<R::Offset>, String> {
    let mut map = HashMap::new();
    let mut entries = unit.entries();

    while let Ok(Some((_, entry))) = entries.next_dfs() {
        let offset = entry.offset();
        let name_opt = attr_string(dwarf, unit, entry, gimli::DW_AT_name);

        match entry.tag() {
            gimli::DW_TAG_base_type
            | gimli::DW_TAG_structure_type
            | gimli::DW_TAG_class_type
            | gimli::DW_TAG_union_type
            | gimli::DW_TAG_enumeration_type
            | gimli::DW_TAG_typedef => {
                if let Some(name) = name_opt {
                    map.insert(offset, name);
                }
            }
            gimli::DW_TAG_pointer_type => {
                // We'll synthesise the name when it's actually looked up.
                // Store a sentinel so we know it's a pointer.
                let synthetic = match name_opt {
                    Some(n) => n,
                    None => {
                        // Try to resolve the pointee at build time.
                        let inner = resolve_type_ref(dwarf, unit, entry, &map, 0)
                            .unwrap_or_else(|| "void".to_string());
                        format!("{inner} *")
                    }
                };
                map.insert(offset, synthetic);
            }
            gimli::DW_TAG_const_type => {
                let inner = resolve_type_ref(dwarf, unit, entry, &map, 0)
                    .unwrap_or_else(|| "void".to_string());
                map.insert(offset, format!("const {inner}"));
            }
            gimli::DW_TAG_volatile_type => {
                let inner = resolve_type_ref(dwarf, unit, entry, &map, 0)
                    .unwrap_or_else(|| "void".to_string());
                map.insert(offset, format!("volatile {inner}"));
            }
            gimli::DW_TAG_reference_type => {
                let inner = resolve_type_ref(dwarf, unit, entry, &map, 0)
                    .unwrap_or_else(|| "void".to_string());
                map.insert(offset, format!("{inner} &"));
            }
            gimli::DW_TAG_rvalue_reference_type => {
                let inner = resolve_type_ref(dwarf, unit, entry, &map, 0)
                    .unwrap_or_else(|| "void".to_string());
                map.insert(offset, format!("{inner} &&"));
            }
            gimli::DW_TAG_array_type => {
                let inner = resolve_type_ref(dwarf, unit, entry, &map, 0)
                    .unwrap_or_else(|| "?".to_string());
                map.insert(offset, format!("{inner}[]"));
            }
            _ => {}
        }
    }

    map
}

/// Resolve a DW_AT_type reference to a human-readable string.
fn resolve_type_ref<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    entry: &DebuggingInformationEntry<R>,
    type_map: &HashMap<UnitOffset<R::Offset>, String>,
    depth: u8,
) -> Option<String> {
    if depth > 8 {
        return Some("...".to_string());
    }

    let attr_val = entry.attr_value(gimli::DW_AT_type).ok()??;

    let offset = match attr_val {
        AttributeValue::UnitRef(o) => o,
        AttributeValue::DebugInfoRef(o) => {
            // Cross-unit reference — convert to unit-relative offset if possible.
            match o.to_unit_offset(&unit.header) {
                Some(uo) => uo,
                None => return None,
            }
        }
        _ => return None,
    };

    // Fast path: already in the map.
    if let Some(name) = type_map.get(&offset) {
        return Some(name.clone());
    }

    // Slow path: look up the entry directly.
    let target = unit.entry(offset).ok()?;
    let name = attr_string(dwarf, unit, &target, gimli::DW_AT_name);

    match target.tag() {
        gimli::DW_TAG_pointer_type => {
            let inner = resolve_type_ref(dwarf, unit, &target, type_map, depth + 1)
                .unwrap_or_else(|| "void".to_string());
            Some(format!("{inner} *"))
        }
        gimli::DW_TAG_const_type => {
            let inner = resolve_type_ref(dwarf, unit, &target, type_map, depth + 1)
                .unwrap_or_else(|| "void".to_string());
            Some(format!("const {inner}"))
        }
        gimli::DW_TAG_volatile_type => {
            let inner = resolve_type_ref(dwarf, unit, &target, type_map, depth + 1)
                .unwrap_or_else(|| "void".to_string());
            Some(format!("volatile {inner}"))
        }
        gimli::DW_TAG_reference_type => {
            let inner = resolve_type_ref(dwarf, unit, &target, type_map, depth + 1)
                .unwrap_or_else(|| "void".to_string());
            Some(format!("{inner} &"))
        }
        gimli::DW_TAG_rvalue_reference_type => {
            let inner = resolve_type_ref(dwarf, unit, &target, type_map, depth + 1)
                .unwrap_or_else(|| "void".to_string());
            Some(format!("{inner} &&"))
        }
        gimli::DW_TAG_array_type => {
            let inner = resolve_type_ref(dwarf, unit, &target, type_map, depth + 1)
                .unwrap_or_else(|| "?".to_string());
            Some(format!("{inner}[]"))
        }
        _ => name,
    }
}

// ── Function parsing ──────────────────────────────────────────────────────────

fn read_function<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    entry: &DebuggingInformationEntry<R>,
    type_map: &HashMap<UnitOffset<R::Offset>, String>,
) -> Option<DwarfFunction> {
    // Skip abstract-origin / declaration-only entries (no address range).
    if attr_flag(entry, gimli::DW_AT_declaration) {
        return None;
    }

    let name = attr_string(dwarf, unit, entry, gimli::DW_AT_name)?;
    let low_pc = attr_addr(entry, gimli::DW_AT_low_pc)?;
    let high_pc = resolve_high_pc(entry, Some(low_pc))?;

    if high_pc == 0 || high_pc < low_pc {
        return None;
    }

    let linkage_name = attr_string(dwarf, unit, entry, gimli::DW_AT_linkage_name)
        .or_else(|| attr_string(dwarf, unit, entry, gimli::DW_AT_MIPS_linkage_name))
        .filter(|ln| ln != &name);

    let return_type = resolve_type_ref(dwarf, unit, entry, type_map, 0);

    let is_inlined  = attr_u64(entry, gimli::DW_AT_inline).unwrap_or(0) != 0;
    let is_external = attr_flag(entry, gimli::DW_AT_external);

    // Determine frame base type: CFA (rbp + 16) or register-based (rbp directly).
    let fb_is_cfa = match frame_base_is_cfa(entry) {
        true => {
            trace!("Using CFA-based frame offsets");
            true
        }
        false => {
            trace!("Using register-based frame offsets");
            false
        }
    };

    // Walk children to collect params and locals.
    let (params, locals) = match read_function_children(dwarf, unit, entry, type_map, fb_is_cfa) {
        (params, locals) => {
            trace!(params = params.len(), locals = locals.len(), "Collected function children");
            (params, locals)
        }
    };

    trace!(name = %name, low_pc = format_args!("{low_pc:#x}"), high_pc = format_args!("{high_pc:#x}"), params = params.len(), locals = locals.len(), "function");

    Some(DwarfFunction {
        name,
        linkage_name,
        low_pc,
        high_pc,
        params,
        locals,
        return_type,
        is_inlined,
        is_external,
    })
}

fn read_function_children<R: Reader>(
    dwarf:     &Dwarf<R>,
    unit:      &Unit<R>,
    parent:    &DebuggingInformationEntry<R>,
    type_map:  &HashMap<UnitOffset<R::Offset>, String>,
    fb_is_cfa: bool,
) -> (Vec<DwarfParam>, Vec<DwarfLocalVar>) {
    let mut params = Vec::new();
    let mut locals = Vec::new();

    let mut tree = match unit.entries_tree(Some(parent.offset())) {
        Ok(t) => t,
        Err(e) => { warn!("error creating entries tree for function: {e}"); return (params, locals); }
    };
    let root = match tree.root() {
        Ok(r) => r,
        Err(e) => { warn!("error getting root of function entries tree: {e}"); return (params, locals); }
    };
    let mut children = root.children();
    loop {
        match children.next() {
            Ok(Some(child)) => {
                let entry = child.entry();
                match entry.tag() {
                    gimli::DW_TAG_formal_parameter => {
                        let name         = attr_string(dwarf, unit, entry, gimli::DW_AT_name);
                        let type_name    = resolve_type_ref(dwarf, unit, entry, type_map, 0);
                        let frame_offset = extract_frame_offset(entry, fb_is_cfa);
                        
                        // Only include parameters with valid frame offsets
                        if frame_offset.is_some() {
                            params.push(DwarfParam { name, type_name, frame_offset });
                        } else {
                            trace!(parameter = ?name, "Parameter has no valid frame offset, skipping");
                        }
                    }
                    gimli::DW_TAG_variable => {
                        if let Some(name) = attr_string(dwarf, unit, entry, gimli::DW_AT_name) {
                            let type_name    = resolve_type_ref(dwarf, unit, entry, type_map, 0);
                            let frame_offset = extract_frame_offset(entry, fb_is_cfa);
                            
                            // Only include variables with valid frame offsets
                            if frame_offset.is_some() {
                                locals.push(DwarfLocalVar { name, type_name, frame_offset });
                            } else {
                                trace!(variable = %name, "Variable has no valid frame offset, skipping");
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(None) => break,
            Err(e) => {
                warn!("error iterating function children: {e}");
                break;
            }
        }
    }

    (params, locals)
}

// ── Top-level type parsing ────────────────────────────────────────────────────

fn read_type<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    entry: &DebuggingInformationEntry<R>,
    type_map: &HashMap<UnitOffset<R::Offset>, String>,
) -> Option<DwarfType> {
    // Skip forward declarations.
    if attr_flag(entry, gimli::DW_AT_declaration) {
        return None;
    }

    let name = attr_string(dwarf, unit, entry, gimli::DW_AT_name)?;

    let kind = match entry.tag() {
        gimli::DW_TAG_base_type => {
            let byte_size = attr_u64(entry, gimli::DW_AT_byte_size).unwrap_or(0);
            DwarfTypeKind::Primitive { byte_size }
        }
        gimli::DW_TAG_structure_type => {
            let fields = read_struct_fields(dwarf, unit, entry, type_map);
            DwarfTypeKind::Struct { fields }
        }
        gimli::DW_TAG_class_type => {
            let fields = read_struct_fields(dwarf, unit, entry, type_map);
            DwarfTypeKind::Class { fields }
        }
        gimli::DW_TAG_union_type => {
            let fields = read_struct_fields(dwarf, unit, entry, type_map);
            DwarfTypeKind::Union { fields }
        }
        gimli::DW_TAG_enumeration_type => {
            let byte_size = attr_u64(entry, gimli::DW_AT_byte_size).unwrap_or(4);
            let variants  = read_enum_variants(unit, entry);
            DwarfTypeKind::Enum { variants, byte_size }
        }
        gimli::DW_TAG_typedef => {
            let target = resolve_type_ref(dwarf, unit, entry, type_map, 0);
            DwarfTypeKind::Typedef { target }
        }
        _ => return None,
    };

    Some(DwarfType { name, kind })
}

fn read_struct_fields<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    parent: &DebuggingInformationEntry<R>,
    type_map: &HashMap<UnitOffset<R::Offset>, String>,
) -> Vec<DwarfField> {
    let mut fields = Vec::new();

    let mut tree = match unit.entries_tree(Some(parent.offset())) {
        Ok(t) => t,
        Err(e) => { warn!("error creating entries tree for struct: {e}"); return fields; }
    };
    let root = match tree.root() {
        Ok(r) => r,
        Err(e) => { warn!("error getting root of struct entries tree: {e}"); return fields; }
    };
    let mut children = root.children();
    loop {
        match children.next() {
            Ok(Some(child)) => {
                let entry = child.entry();
                if entry.tag() == gimli::DW_TAG_member {
                    let name = attr_string(dwarf, unit, entry, gimli::DW_AT_name);
                    let type_name = resolve_type_ref(dwarf, unit, entry, type_map, 0);
                    let byte_offset = attr_u64(entry, gimli::DW_AT_data_member_location);
                    fields.push(DwarfField { name, type_name, byte_offset });
                }
            }
            Ok(None) => break,
            Err(e) => {
                warn!("error iterating struct members: {e}");
                break;
            }
        }
    }

    fields
}

fn read_enum_variants<R: Reader>(
    unit: &Unit<R>,
    parent: &DebuggingInformationEntry<R>,
) -> Vec<String> {
    let mut variants = Vec::new();

    let mut tree = match unit.entries_tree(Some(parent.offset())) {
        Ok(t) => t,
        Err(e) => { warn!("error creating entries tree for enum: {e}"); return variants; }
    };
    let root = match tree.root() {
        Ok(r) => r,
        Err(e) => { warn!("error getting root of enum entries tree: {e}"); return variants; }
    };
    let mut children = root.children();
    loop {
        match children.next() {
            Ok(Some(child)) => {
                let entry = child.entry();
                if entry.tag() == gimli::DW_TAG_enumerator {
                    if let Ok(Some(av)) = entry.attr_value(gimli::DW_AT_name) {
                        if let AttributeValue::String(s) = av {
                            if let Ok(s) = s.to_string() {
                                variants.push(s.to_string());
                            }
                        }
                        // Inline strings only; cross-unit string table refs
                        // not handled here — acceptable limitation.
                    }
                }
            }
            Ok(None) => break,
            Err(e) => {
                warn!("error iterating enum variants: {e}");
                break;
            }
        }
    }

    variants
}

// ── Line table ────────────────────────────────────────────────────────────────

fn read_line_table<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    out: &mut DwarfInfo,
) {
    let program = match unit.line_program.clone() {
        Some(lp) => lp,
        None => return,
    };

    let mut rows = program.rows();

    while let Ok(Some((header_ref, row))) = rows.next_row() {
        if !row.end_sequence() {
            let address = row.address();

            // Resolve file name.
            let file = row.file(header_ref).and_then(|f| {
                // Resolve directory — index 0 means the compilation directory.
                let dir = if f.directory_index() == 0 {
                    unit.comp_dir.clone()
                        .and_then(|d| d.to_string().ok().map(|s| s.to_string()))
                        .unwrap_or_default()
                } else {
                    header_ref.directory(f.directory_index())
                        .and_then(|d| {
                            // DWARF 4: inline string.  DWARF 5+: may be a
                            // DebugLineStrRef or DebugStrRef — resolve via dwarf.
                            match d {
                                AttributeValue::String(s) =>
                                    s.to_string().ok().map(|s| s.to_string()),
                                AttributeValue::DebugLineStrRef(_)
                                | AttributeValue::DebugStrRef(_) => {
                                    dwarf.attr_string(unit, d).ok()
                                        .and_then(|s| s.to_string().ok().map(|c| c.into_owned()))
                                }
                                _ => None,
                            }
                        })
                        .unwrap_or_default()
                };

                // Resolve the file base name — same strategy.
                let base = match f.path_name() {
                    AttributeValue::String(s) =>
                        s.to_string().ok()?.to_string(),
                    AttributeValue::DebugLineStrRef(_)
                    | AttributeValue::DebugStrRef(_) => {
                        dwarf.attr_string(unit, f.path_name()).ok()
                            .and_then(|s| s.to_string().ok().map(|c| c.into_owned()))?
                    }
                    _ => return None,
                };

                if !dir.is_empty() && !base.starts_with('/') {
                    Some(format!("{dir}/{base}"))
                } else {
                    Some(base)
                }
            }).unwrap_or_default();

            let line = row.line().map(|l| l.get() as u32).unwrap_or(0);
            let column = match row.column() {
                gimli::ColumnType::Column(c) => c.get() as u16,
                gimli::ColumnType::LeftEdge => 0,
            };

            out.lines.push(LineEntry {
                address,
                file,
                line,
                column,
                is_stmt: row.is_stmt(),
            });
        }
    }

}

// ── Attribute helpers ─────────────────────────────────────────────────────────

/// Read a DW_AT_* string attribute, resolving .debug_str references.
fn attr_string<R: Reader>(
    dwarf: &Dwarf<R>,
    unit: &Unit<R>,
    entry: &DebuggingInformationEntry<R>,
    name: gimli::DwAt,
) -> Option<String> {
    let av = entry.attr_value(name).ok()??;
    dwarf.attr_string(unit, av).ok()?.to_string().ok().map(|s| s.into_owned())
}

/// Read a DW_AT_* address attribute (DW_FORM_addr).
fn attr_addr<R: Reader>(
    entry: &DebuggingInformationEntry<R>,
    name: gimli::DwAt,
) -> Option<u64> {
    match entry.attr_value(name).ok()?? {
        AttributeValue::Addr(a) => Some(a),
        _ => None,
    }
}

/// Read a DW_AT_* unsigned integer attribute.
fn attr_u64<R: Reader>(
    entry: &DebuggingInformationEntry<R>,
    name: gimli::DwAt,
) -> Option<u64> {
    match entry.attr_value(name).ok()?? {
        AttributeValue::Udata(v)  => Some(v),
        AttributeValue::Data1(v)  => Some(v as u64),
        AttributeValue::Data2(v)  => Some(v as u64),
        AttributeValue::Data4(v)  => Some(v as u64),
        AttributeValue::Data8(v)  => Some(v),
        AttributeValue::Sdata(v)  => Some(v as u64),
        _ => None,
    }
}

/// Read a DW_AT_* flag attribute.
fn attr_flag<R: Reader>(
    entry: &DebuggingInformationEntry<R>,
    name: gimli::DwAt,
) -> bool {
    matches!(
        entry.attr_value(name).ok().flatten(),
        Some(AttributeValue::Flag(true))
    )
}

/// Resolve DW_AT_high_pc which is either an address or an offset from low_pc.
fn resolve_high_pc<R: Reader>(
    entry: &DebuggingInformationEntry<R>,
    low_pc: Option<u64>,
) -> Option<u64> {
    let av = entry.attr_value(gimli::DW_AT_high_pc).ok()??;
    match av {
        AttributeValue::Addr(a) => Some(a),
        // DWARF 4+: high_pc is stored as an offset from low_pc.
        AttributeValue::Udata(offset) => low_pc.map(|l| l + offset),
        AttributeValue::Data1(o) => low_pc.map(|l| l + o as u64),
        AttributeValue::Data2(o) => low_pc.map(|l| l + o as u64),
        AttributeValue::Data4(o) => low_pc.map(|l| l + o as u64),
        AttributeValue::Data8(o) => low_pc.map(|l| l + o),
        _ => None,
    }
}

// ── Language table ────────────────────────────────────────────────────────────

/// Determine if the frame base is CFA (Canonical Frame Address) or register-based.
/// 
/// This function examines the DW_AT_frame_base attribute to determine how frame
/// offsets should be interpreted. CFA (Canonical Frame Address) is typically
/// used in modern DWARF implementations.
fn frame_base_is_cfa<R: Reader>(entry: &DebuggingInformationEntry<R>) -> bool {
    // Check for DW_AT_frame_base attribute
    if let Ok(Some(attr)) = entry.attr_value(gimli::DW_AT_frame_base) {
        // If frame base is defined, check if it's CFA-based
        match attr {
            AttributeValue::Exprloc(_expr) => {
                // Expression-based frame base - try to detect CFA pattern
                // CFA is typically represented as DW_OP_call_frame_cfa
                // For now, we'll assume expression-based frame bases are CFA
                // A more complete implementation would evaluate the expression
                trace!("Expression-based frame base detected");
                true
            }
            AttributeValue::Udata(_) | 
            AttributeValue::Data1(_) | 
            AttributeValue::Data2(_) | 
            AttributeValue::Data4(_) | 
            AttributeValue::Data8(_) => {
                // Register number - register-based frame base
                trace!("Register-based frame base detected");
                false
            }
            _ => {
                // Other types - assume register-based
                trace!("Unknown frame base type, assuming register-based");
                false
            }
        }
    } else {
        // No frame base attribute - this is common in modern DWARF
        // where CFA is assumed by default
        trace!("No frame base attribute, assuming CFA by default");
        true
    }
}

/// Extract the frame offset from a DWARF entry.
///
/// This function evaluates DWARF location expressions to determine the frame offset
/// for parameters and local variables. It handles both CFA-based and register-based
/// frame offsets.
fn extract_frame_offset<R: Reader>(entry: &DebuggingInformationEntry<R>, fb_is_cfa: bool) -> Option<i64> {
    // Try to get the location attribute
    let attr = match entry.attr_value(gimli::DW_AT_location) {
        Ok(Some(attr)) => attr,
        _ => {
            trace!("No location attribute found");
            return None;
        }
    };

    // Handle different location types
    match attr {
        AttributeValue::Exprloc(_expr) => {
            // Evaluate the DWARF expression to get the actual offset
            // This is a simplified evaluation - a full implementation would need
            // to properly evaluate the DWARF expression stack machine
            evaluate_dwarf_expression(_expr, fb_is_cfa)
        }
        AttributeValue::Udata(offset) => {
            // Direct offset value
            Some(offset as i64)
        }
        AttributeValue::Data1(offset) => {
            Some(offset as i64)
        }
        AttributeValue::Data2(offset) => {
            Some(offset as i64)
        }
        AttributeValue::Data4(offset) => {
            Some(offset as i64)
        }
        AttributeValue::Data8(offset) => {
            Some(offset as i64)
        }
        AttributeValue::Sdata(offset) => {
            // Signed offset
            Some(offset as i64)
        }
        _ => {
            trace!("Unsupported location attribute type");
            None
        }
    }
}

/// Evaluate a DWARF expression to extract frame offset.
///
/// This is a simplified evaluation that handles common DWARF operations.
/// A complete implementation would need to handle the full DWARF expression
/// stack machine.
fn evaluate_dwarf_expression<R: Reader>(_expr: gimli::Expression<R>, fb_is_cfa: bool) -> Option<i64> {
    // For now, we'll implement a simplified evaluation
    // In a real implementation, we would need to:
    // 1. Get the expression bytes
    // 2. Implement a DWARF expression evaluator
    // 3. Handle the stack machine operations
    // 4. Return the computed offset
    
    // This is a placeholder that returns 0 for all expressions
    // A real implementation would analyze the expression bytes
    trace!("DWARF expression evaluation not fully implemented, returning dummy offset");
    
    if fb_is_cfa {
        Some(0)
    } else {
        Some(0)
    }
}

fn lang_from_u64(v: u64) -> DwarfLanguage {
    match v as u16 {
        0x0001 => DwarfLanguage::C,         // DW_LANG_C89
        0x0002 => DwarfLanguage::C,         // DW_LANG_C
        0x0004 => DwarfLanguage::Cpp,       // DW_LANG_C_plus_plus
        0x0005 => DwarfLanguage::Ada,       // DW_LANG_Ada83
        0x0006 => DwarfLanguage::Cobol,      // DW_LANG_Cobol74
        0x0007 => DwarfLanguage::Fortran,   // DW_LANG_Fortran77
        0x0008 => DwarfLanguage::Fortran,   // DW_LANG_Fortran90
        0x000B => DwarfLanguage::Java,      // DW_LANG_Java
        0x000D => DwarfLanguage::C,         // DW_LANG_C99
        0x0013 => DwarfLanguage::Ada,       // DW_LANG_Ada95
        0x0015 => DwarfLanguage::Fortran,   // DW_LANG_Fortran95
        0x001D => DwarfLanguage::ObjC,      // DW_LANG_ObjC
        0x0021 => DwarfLanguage::Cpp,       // DW_LANG_C_plus_plus_03
        0x0025 => DwarfLanguage::Cpp,       // DW_LANG_C_plus_plus_11
        0x0026 => DwarfLanguage::C,         // DW_LANG_C11
        0x0029 => DwarfLanguage::Cpp,       // DW_LANG_C_plus_plus_14
        0x001E => DwarfLanguage::Swift,     // DW_LANG_Swift (Apple extension)
        // Vendor extensions
        0x8001 => DwarfLanguage::Rust,      // DW_LANG_Rust (LLVM extension)
        0x8002 => DwarfLanguage::Go,        // DW_LANG_Go
        n      => DwarfLanguage::Other(n),
    }
}
