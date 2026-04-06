//! Function detection: collect entry points from symbols, entry point header,
//! and call-site scanning.
//!
//! ## Detection strategy
//!
//! 1. **Binary entry point** — `e_entry` / `AddressOfEntryPoint` from the header.
//! 2. **Symbol table** — all symbols of kind `Function` with a non-zero address.
//! 3. **Call-site scanning** — every `call <direct_target>` instruction reveals
//!    a new function entry point.
//! 4. **Indirect jump tables** — `jmp [rip + disp]` or `jmp [reg*4 + base]`
//!    patterns are heuristically scanned to discover switch-table targets and
//!    virtual dispatch stubs.  Addresses that fall inside a known code section
//!    are added as function candidates.

use rustdec_disasm::Instruction;
use rustdec_loader::{BinaryObject, SectionKind, SymbolKind};
use std::collections::BTreeMap;
use tracing::{debug, trace, warn};

/// Returns a map of `{ virtual_address → name }` for all detected functions.
pub fn detect_functions(
    obj:   &BinaryObject,
    insns: &[Instruction],
) -> BTreeMap<u64, String> {
    let mut funcs: BTreeMap<u64, String> = BTreeMap::new();

    // ── 1. Binary entry point ─────────────────────────────────────────────────
    if let Some(ep) = obj.entry_point {
        debug!(addr = format_args!("{:#x}", ep), "entry point added");
        funcs.insert(ep, "entry".to_string());
    }

    // ── 2. Symbol table ───────────────────────────────────────────────────────
    for sym in &obj.symbols {
        if sym.kind == SymbolKind::Function && sym.address != 0 {
            trace!(name = %sym.name,
                   addr = format_args!("{:#x}", sym.address),
                   "function from symbol table");
            funcs.insert(sym.address, sym.name.clone());
        }
    }
    debug!(count = funcs.len(), "functions from symbol table + entry point");

    // Build a set of all code-section address ranges for fast membership tests.
    let code_ranges: Vec<(u64, u64)> = obj.code_sections()
        .map(|s| (s.virtual_addr, s.virtual_addr + s.size))
        .collect();

    let in_code = |addr: u64| -> bool {
        code_ranges.iter().any(|&(start, end)| addr >= start && addr < end)
    };

    // ── 3. Call-site scanning ─────────────────────────────────────────────────
    let before = funcs.len();
    for insn in insns {
        if insn.is_call() {
            if let Some(target) = insn.branch_target() {
                if in_code(target) {
                    let is_new = funcs
                        .entry(target)
                        .or_insert_with(|| format!("sub_{:x}", target))
                        .is_empty(); // entry() always inserts or returns existing
                    // Log only new discoveries.
                    if !funcs.contains_key(&target) || funcs[&target].starts_with("sub_") {
                        trace!(caller = format_args!("{:#x}", insn.address),
                               target = format_args!("{:#x}", target),
                               "new function via direct call");
                    }
                }
            }
        }
    }
    let discovered_calls = funcs.len() - before;
    debug!(discovered = discovered_calls, total = funcs.len(),
           "call-site scan complete");

    // ── 4. Indirect jump table scanning ──────────────────────────────────────
    //
    // Patterns targeted:
    //   a) `jmp qword ptr [rip + disp]`  — PLT / import stub
    //   b) `jmp qword ptr [rax*8 + base]` — switch table
    //   c) `jmp rax` / `jmp [rax]`        — virtual dispatch (target unknown)
    //
    // For (a) and (b) we try to read the raw bytes at the computed address
    // and interpret them as a pointer array.  For (c) we can't know the target
    // statically, so we log a warning and skip.
    let before = funcs.len();
    for insn in insns {
        if !insn.is_terminator() { continue; }
        if insn.mnemonic != "jmp" { continue; }
        let ops = insn.operands.trim();

        // Skip direct jumps (already handled by call-site scan via CFG).
        if insn.branch_target().is_some() { continue; }

        // Indirect via register only — can't resolve statically.
        if !ops.contains('[') {
            trace!(at  = format_args!("{:#x}", insn.address),
                   ops = %ops,
                   "indirect jmp via register — target unknown, skipping");
            continue;
        }

        // Try to extract the base address from `[rip + disp]`.
        if let Some(target) = extract_rip_relative(insn, ops) {
            if in_code(target) && !funcs.contains_key(&target) {
                trace!(at     = format_args!("{:#x}", insn.address),
                       target = format_args!("{:#x}", target),
                       "function candidate from RIP-relative jmp");
                funcs.insert(target, format!("sub_{:x}", target));
            }
            continue;
        }

        // Try to scan a potential jump table at the base address.
        if let Some(base) = extract_jump_table_base(ops) {
            let targets = scan_jump_table(base, obj, &in_code);
            if !targets.is_empty() {
                debug!(at      = format_args!("{:#x}", insn.address),
                       base    = format_args!("{:#x}", base),
                       entries = targets.len(),
                       "jump table detected");
                for tgt in targets {
                    funcs.entry(tgt).or_insert_with(|| format!("sub_{:x}", tgt));
                }
            }
        }
    }
    let discovered_jmp = funcs.len() - before;
    if discovered_jmp > 0 {
        debug!(discovered = discovered_jmp, total = funcs.len(),
               "indirect jump scan complete");
    }

    funcs
}

// ── Indirect jump helpers ─────────────────────────────────────────────────────

/// Try to resolve a `jmp [rip + disp]` to an absolute address.
///
/// In Intel 64 RIP-relative addressing, the effective address is
/// `rip_after_insn + disp` where `rip_after_insn = insn.address + insn.size`.
fn extract_rip_relative(insn: &Instruction, ops: &str) -> Option<u64> {
    // Operand looks like: `qword ptr [rip + 0x1234]` or `[rip + 0x1234]`
    if !ops.contains("rip") { return None; }

    // Extract the displacement token after '+' or '-'.
    let inner = ops
        .trim_start_matches(|c: char| !c.is_ascii_punctuation() || c == '[')
        .trim_start_matches('[')
        .split(']').next()?;

    // Find the displacement: the numeric token after rip +/-.
    let (sign, disp_str) = if let Some(pos) = inner.find('+') {
        (1i64, inner[pos + 1..].trim())
    } else if let Some(pos) = inner.find('-') {
        (-1i64, inner[pos + 1..].trim())
    } else {
        return None;
    };

    let disp = parse_hex_or_dec(disp_str)? as i64;
    let rip_after = insn.address + insn.size as u64;
    Some(rip_after.wrapping_add((sign * disp) as u64))
}

/// Try to extract the base address of a jump table from an operand like
/// `[rax*8 + 0x402000]` or `[0x402000 + rax*8]`.
fn extract_jump_table_base(ops: &str) -> Option<u64> {
    // Must be a memory operand.
    if !ops.contains('[') { return None; }
    let inner = ops
        .split('[').nth(1)?
        .split(']').next()?
        .trim();

    // Look for a hex constant token that looks like an absolute address (≥ 0x1000).
    for tok in inner.split(|c| c == '+' || c == '-' || c == '*' || c == ' ') {
        let tok = tok.trim();
        if let Some(v) = parse_hex_or_dec(tok) {
            if v >= 0x1000 {
                return Some(v);
            }
        }
    }
    None
}

/// Scan a potential jump table at `base` and return valid code pointers.
///
/// Reads 64-bit little-endian values from the binary at `base` until we hit
/// an invalid pointer.  Stops after 256 entries (upper bound for switch tables).
fn scan_jump_table(
    base:      u64,
    obj:       &BinaryObject,
    in_code:   &impl Fn(u64) -> bool,
) -> Vec<u64> {
    let mut targets = Vec::new();

    // Find the section containing `base`.
    let section = obj.section_at(base);
    let section = match section {
        Some(s) => s,
        None    => return targets,
    };

    let offset = (base - section.virtual_addr) as usize;
    let data   = &section.data;

    // Read up to 256 8-byte pointers.
    for i in 0..256usize {
        let ptr_off = offset + i * 8;
        if ptr_off + 8 > data.len() { break; }

        let bytes: [u8; 8] = data[ptr_off..ptr_off + 8]
            .try_into()
            .unwrap_or([0u8; 8]);
        let ptr = u64::from_le_bytes(bytes);

        if in_code(ptr) {
            targets.push(ptr);
        } else {
            // First non-code pointer → end of table.
            break;
        }
    }

    targets
}

// ── Parsing helpers ───────────────────────────────────────────────────────────

fn parse_hex_or_dec(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex.trim_end_matches(|c: char| !c.is_ascii_hexdigit()), 16).ok()
    } else {
        s.parse::<u64>().ok()
    }
}
