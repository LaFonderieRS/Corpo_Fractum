use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One entry in the corpus (name + path to the binary).
#[derive(Debug, Clone)]
pub struct BenchCase {
    pub name: String,
    pub path: PathBuf,
}

/// Metrics extracted from a single decompiled binary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metrics {
    /// Number of functions emitted (CRT stubs excluded).
    pub functions:   u32,
    /// Sum of `slot_table.len()` across all IR functions.
    pub stack_slots: u32,
    /// Occurrences of `if (` in the emitted C.
    pub if_count:    u32,
    /// Occurrences of `while (` in the emitted C.
    pub loop_count:  u32,
    /// Occurrences of `v{n}` temporaries in the emitted C.
    pub temp_vars:   u32,
    /// Occurrences of `goto ` in the emitted C (always 0 for now).
    pub goto_count:  u32,
}

/// Result for one binary in the corpus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseResult {
    pub case:       String,
    pub success:    bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error:      Option<String>,
    pub elapsed_ms: u64,
    pub metrics:    Metrics,
}

/// Full benchmark report written to / read from JSON.
#[derive(Debug, Serialize, Deserialize)]
pub struct BenchReport {
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_hash:  Option<String>,
    pub cases:     Vec<CaseResult>,
    pub totals:    Metrics,
}

impl BenchReport {
    pub fn new(cases: Vec<CaseResult>) -> Self {
        let totals = aggregate(&cases);
        BenchReport {
            timestamp: timestamp_now(),
            git_hash:  git_hash(),
            cases,
            totals,
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub fn aggregate(cases: &[CaseResult]) -> Metrics {
    cases.iter().fold(Metrics::default(), |mut acc, c| {
        acc.functions   += c.metrics.functions;
        acc.stack_slots += c.metrics.stack_slots;
        acc.if_count    += c.metrics.if_count;
        acc.loop_count  += c.metrics.loop_count;
        acc.temp_vars   += c.metrics.temp_vars;
        acc.goto_count  += c.metrics.goto_count;
        acc
    })
}

fn timestamp_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Format as YYYY-MM-DDTHH:MM:SSZ (no external dep)
    let s = secs;
    let sec  = s % 60;
    let min  = (s / 60) % 60;
    let hour = (s / 3600) % 24;
    let days = s / 86400;
    // Approximate date from Unix epoch (good enough for a bench timestamp)
    let (y, mo, d) = days_to_ymd(days);
    format!("{y:04}-{mo:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    let mut y = 1970u64;
    loop {
        let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
        let ydays = if leap { 366 } else { 365 };
        if days < ydays { break; }
        days -= ydays;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let mdays = [31u64, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut mo = 1u64;
    for md in &mdays {
        if days < *md { break; }
        days -= md;
        mo += 1;
    }
    (y, mo, days + 1)
}

fn git_hash() -> Option<String> {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| if o.status.success() {
            String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string())
        } else {
            None
        })
}
