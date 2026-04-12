//! String table — scans binary sections for null-terminated ASCII/UTF-8 strings
//! and UTF-16 LE strings (Windows PE wide strings).
//!
//! ## What counts as a string
//!
//! **ASCII / UTF-8** — any sequence of printable bytes (0x20–0x7e) plus common
//! escape characters (`\t`, `\n`, `\r`) that is terminated by a null byte and
//! has a minimum length of 4 printable characters.
//!
//! **UTF-16 LE** — sequences of `(printable_byte, 0x00)` pairs terminated by
//! `0x00 0x00`, minimum 4 printable characters.  Common in Windows PE binaries
//! where string literals are stored as wide strings (`wchar_t*`).
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
        scan_utf16_le(section.virtual_addr, &section.data, &mut table);
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

/// Scan `data` for null-terminated ASCII strings, inserting each into `out`.
///
/// Single-pass flat loop: `start` and `buf` are reset explicitly on every
/// non-printable byte so that the next candidate begins immediately after the
/// bad byte without re-entering the outer loop.
fn scan_section(base_va: u64, data: &[u8], out: &mut StringTable) {
    let mut i         = 0usize;
    let mut start     = 0usize;
    let mut buf       = String::new();
    let mut printable = 0usize;

    while i < data.len() {
        match data[i] {
            0 => {
                // Null terminator — emit candidate if long enough.
                if printable >= 4 {
                    let va = base_va + start as u64;
                    trace!(va = format_args!("{:#x}", va), content = %buf, "string found");
                    out.insert(va, buf.clone());
                }
                buf.clear();
                printable = 0;
                i += 1;
                start = i;
            }
            b'\t' | b'\n' | b'\r' => {
                buf.push(data[i] as char);
                i += 1;
            }
            0x20..=0x7e => {
                buf.push(data[i] as char);
                printable += 1;
                i += 1;
            }
            _ => {
                // Non-printable — discard current candidate and start fresh
                // from the position immediately after this byte.
                buf.clear();
                printable = 0;
                i += 1;
                start = i;
            }
        }
    }
}

/// Scan `data` for null-terminated UTF-16 LE strings (Windows PE wide strings).
///
/// A valid wide character in the ASCII range is a `(lo, 0x00)` pair where `lo`
/// is a printable byte.  Strings are terminated by `(0x00, 0x00)`.  If a
/// virtual address already has an ASCII entry the wide entry is silently
/// dropped so the ASCII version takes precedence.
fn scan_utf16_le(base_va: u64, data: &[u8], out: &mut StringTable) {
    let mut i         = 0usize;
    let mut start     = 0usize;
    let mut buf       = String::new();
    let mut printable = 0usize;

    while i + 1 < data.len() {
        let lo = data[i];
        let hi = data[i + 1];

        if lo == 0 && hi == 0 {
            // Wide null terminator — emit candidate.
            if printable >= 4 {
                let va = base_va + start as u64;
                trace!(va = format_args!("{:#x}", va), content = %buf, "utf16le string found");
                out.entry(va).or_insert_with(|| buf.clone());
            }
            buf.clear();
            printable = 0;
            i += 2;
            start = i;
        } else if hi == 0 && is_wide_printable(lo) {
            // Printable BMP character in ASCII range.
            buf.push(lo as char);
            if lo >= 0x20 { printable += 1; }
            i += 2;
        } else {
            // Not a valid wide char pair — discard candidate and advance by
            // one byte so we try every possible alignment.
            buf.clear();
            printable = 0;
            i += 1;
            start = i;
        }
    }
}

#[inline]
fn is_wide_printable(b: u8) -> bool {
    matches!(b, b'\t' | b'\n' | b'\r' | 0x20..=0x7e)
}
