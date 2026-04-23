use crate::model::Metrics;
use rustdec_ir::IrModule;

/// Compute metrics from the emitted C output and the IR module.
pub fn compute(module: &IrModule, outputs: &[(String, String)]) -> Metrics {
    let src: String = outputs.iter()
        .map(|(_, s)| s.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    Metrics {
        functions:   outputs.len() as u32,
        stack_slots: module.functions.iter().map(|f| f.slot_table.len() as u32).sum(),
        if_count:    count(&src, "if ("),
        loop_count:  count(&src, "while ("),
        goto_count:  count(&src, "goto "),
        temp_vars:   count_temp_vars(&src),
    }
}

fn count(text: &str, pat: &str) -> u32 {
    let mut n = 0u32;
    let mut pos = 0;
    while let Some(i) = text[pos..].find(pat) {
        n += 1;
        pos += i + pat.len();
    }
    n
}

/// Count occurrences of `v` followed by one or more digits that are not part
/// of a longer identifier (i.e. preceded by a non-alphanumeric, non-`_` char).
fn count_temp_vars(text: &str) -> u32 {
    let b = text.as_bytes();
    let mut n = 0u32;
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'v' {
            let prev_ok = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
            let next_ok = i + 1 < b.len() && b[i + 1].is_ascii_digit();
            if prev_ok && next_ok {
                n += 1;
            }
        }
        i += 1;
    }
    n
}
