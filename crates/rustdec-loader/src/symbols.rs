//! Symbol map — unified lookup table combining string literals, ELF/PE symbols,
//! and DWARF debug info.
//!
//! [`build_symbol_map`] is the single entry point called by the analysis
//! pipeline after loading a binary.  The resulting [`SymbolMap`] is passed to
//! the lift pass so that every virtual address that corresponds to a known
//! symbol can be annotated in the IR as an [`rustdec_ir::Expr::Symbol`].
//!
//! ## Priority
//!
//! When multiple sources describe the same address the first match wins:
//!
//! 1. String literals (extracted from `.rodata` / `.rdata` / `.data`)
//! 2. DWARF function names (highest-quality debug info, if present)
//! 3. ELF / PE symbol table entries
//!
//! This ordering ensures that a `.rodata` slot that also has a symbol entry
//! is treated as a string, not a global variable.

use std::collections::HashMap;

use crate::{BinaryObject, StringTable, SymbolKind as LoaderSymbolKind};

// ── Public types ──────────────────────────────────────────────────────────────

/// Semantic kind of a symbol entry — mirrors `rustdec_ir::SymbolKind` so the
/// lift pass can emit the right IR node without importing the loader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolMapKind {
    /// Null-terminated string literal.
    String,
    /// Named function (import, export, or DWARF-named).
    Function,
    /// Named global variable / object.
    Global,
}

/// A single resolved symbol entry.
#[derive(Debug, Clone)]
pub struct SymbolEntry {
    /// Display name.  For strings this is the decoded text content;
    /// for functions and globals it is the symbol identifier.
    pub name: String,
    /// How this symbol should be rendered by codegen.
    pub kind: SymbolMapKind,
}

/// Maps virtual address → [`SymbolEntry`].
pub type SymbolMap = HashMap<u64, SymbolEntry>;

// ── Builder ───────────────────────────────────────────────────────────────────

/// Build a unified symbol map from all available sources in `obj`.
///
/// `strings` must be the table previously produced by [`crate::extract_strings`]
/// for the same object; it is consumed to avoid re-scanning the sections.
pub fn build_symbol_map(obj: &BinaryObject, strings: &StringTable) -> SymbolMap {
    let mut map: SymbolMap = HashMap::new();

    // ── 1. String literals (highest priority) ────────────────────────────────
    for (&addr, content) in strings {
        map.insert(addr, SymbolEntry {
            name: content.clone(),
            kind: SymbolMapKind::String,
        });
    }

    // ── 2. DWARF function names (richer than symbol table, if present) ────────
    if let Some(dwarf) = &obj.dwarf {
        for func in &dwarf.functions {
            if func.low_pc == 0 { continue; }
            map.entry(func.low_pc).or_insert(SymbolEntry {
                name: func.name.clone(),
                kind: SymbolMapKind::Function,
            });
        }
    }

    // ── 3. Symbol table ───────────────────────────────────────────────────────
    for sym in &obj.symbols {
        if sym.name.is_empty() { continue; }
        // Skip symbols at address 0 — these are unresolved dynamic imports
        // (st_value == 0 in the dynsym table before relocation).  The real
        // PLT stubs added by extract_plt_symbols carry the correct addresses.
        if sym.address == 0 { continue; }
        let kind = match sym.kind {
            LoaderSymbolKind::Function
            | LoaderSymbolKind::Import
            | LoaderSymbolKind::Export  => SymbolMapKind::Function,
            LoaderSymbolKind::Object
            | LoaderSymbolKind::Other   => SymbolMapKind::Global,
        };
        map.entry(sym.address).or_insert(SymbolEntry {
            name: sym.name.clone(),
            kind,
        });
    }

    map
}
