//! String table — scans binary sections for null-terminated ASCII/UTF-8 strings.
//!
//! ## What counts as a string
//!
//! We extract any sequence of printable bytes (0x20–0x7e) plus common
//! escape characters (`\t`, `\n`, `\r`) that is terminated by a null byte
//! and has a minimum length of 4 printable characters.
//!
//! This covers the vast majority of C string literals while avoiding false
//! positives from binary data blobs.
//!
//! ## Sections scanned
//!
//! - `SectionKind::ReadOnlyData`  (`.rodata`, `__TEXT,__cstring`, `.rdata`)
//! - `SectionKind::Data`          (`.data` — mutable globals, less common)
//!
//! Code sections are intentionally skipped — strings embedded there (e.g.
//! `mov rax, 0x...`) are resolved by the lifter via `Value::Const`, not here.

use crate::{BinaryObject, SectionKind};
use std::collections::HashMap;
use tracing::{debug, trace};

/// Maps virtual address → decoded string content.
pub type StringTable = HashMap<u64, String>;

/// Scan all data sections of `obj` and return a `StringTable`.
pub fn extract_strings(obj: &BinaryObject) -> StringTable {
    let mut table = StringTable::new();

    for section in obj.sections.iter().filter(|s| {
        matches!(s.kind, SectionKind::ReadOnlyData | SectionKind::Data)
    }) {
        let count_before = table.len();
        scan_section(section.virtual_addr, &section.data, &mut table);
        let found = table.len() - count_before;
        if found > 0 {
            debug!(section = %section.name,
                   va      = format_args!("{:#x}", section.virtual_addr),
                   found,
                   "strings extracted");
        }
    }

    debug!(total = table.len(), "string extraction complete");
    table
}

fn scan_section(base_va: u64, data: &[u8], out: &mut StringTable) {
    let mut i = 0;
    while i < data.len() {
        // Try to start a string at position i.
        let start = i;
        let mut printable = 0usize;
        let mut buf = String::new();

        while i < data.len() {
            let b = data[i];
            match b {
                0 => {
                    // Null terminator — end of candidate string.
                    i += 1;
                    break;
                }
                b'\t' | b'\n' | b'\r' => {
                    buf.push(b as char);
                    i += 1;
                }
                0x20..=0x7e => {
                    buf.push(b as char);
                    printable += 1;
                    i += 1;
                }
                _ => {
                    // Non-printable byte — not a string.
                    i += 1;
                    buf.clear();
                    printable = 0;
                    break;
                }
            }
        }

        // Accept strings with at least 4 printable characters.
        if printable >= 4 && !buf.is_empty() {
            let va = base_va + start as u64;
            trace!(va = format_args!("{:#x}", va), content = %buf, "string found");
            out.insert(va, buf);
        }
    }
}
