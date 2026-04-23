use crate::model::{BenchReport, Metrics};

pub struct CompareReport<'a> {
    pub baseline: &'a BenchReport,
    pub current:  &'a BenchReport,
}

impl<'a> CompareReport<'a> {
    pub fn new(baseline: &'a BenchReport, current: &'a BenchReport) -> Self {
        Self { baseline, current }
    }

    pub fn print(&self) {
        let b = self.baseline;
        let c = self.current;

        println!("Baseline : {} ({})", b.timestamp, b.git_hash.as_deref().unwrap_or("?"));
        println!("Current  : {} ({})", c.timestamp, c.git_hash.as_deref().unwrap_or("?"));
        println!();

        // Table header
        println!("{:<22}  {:>6}  {:>6}  {:>5}  {:>5}  {:>5}  {:>5}",
            "case", "funcs", "slots", "if", "loop", "vars", "goto");
        println!("{}", "-".repeat(66));

        // Per-case rows
        let mut any_regression = false;
        let mut any_improvement = false;

        for cc in &c.cases {
            let bb = b.cases.iter().find(|r| r.case == cc.case);
            let status = if !cc.success {
                "FAIL"
            } else if bb.map_or(false, |b| !b.success) || bb.is_none() {
                "NEW "
            } else {
                "    "
            };
            let bm = bb.map(|r| &r.metrics);
            println!("{:<22}  {}  {}  {}  {}  {}  {}  {}",
                cc.case,
                diff_cell(bm.map(|m| m.functions as i64),   cc.metrics.functions as i64,   Sign::Neutral),
                diff_cell(bm.map(|m| m.stack_slots as i64), cc.metrics.stack_slots as i64, Sign::Higher),
                diff_cell(bm.map(|m| m.if_count as i64),    cc.metrics.if_count as i64,    Sign::Higher),
                diff_cell(bm.map(|m| m.loop_count as i64),  cc.metrics.loop_count as i64,  Sign::Higher),
                diff_cell(bm.map(|m| m.temp_vars as i64),   cc.metrics.temp_vars as i64,   Sign::Lower),
                diff_cell(bm.map(|m| m.goto_count as i64),  cc.metrics.goto_count as i64,  Sign::Lower),
                status,
            );
            if let Some(bm) = bm {
                if is_regression(bm, &cc.metrics)  { any_regression  = true; }
                if is_improvement(bm, &cc.metrics) { any_improvement = true; }
            }
        }

        // Cases removed from current
        for bb in &b.cases {
            if !c.cases.iter().any(|r| r.case == bb.case) {
                println!("{:<22}  (removed)", bb.case);
            }
        }

        println!("{}", "-".repeat(66));

        // Totals row
        let bt = &b.totals;
        let ct = &c.totals;
        println!("{:<22}  {}  {}  {}  {}  {}  {}",
            "TOTALS",
            diff_cell(Some(bt.functions as i64),   ct.functions as i64,   Sign::Neutral),
            diff_cell(Some(bt.stack_slots as i64), ct.stack_slots as i64, Sign::Higher),
            diff_cell(Some(bt.if_count as i64),    ct.if_count as i64,    Sign::Higher),
            diff_cell(Some(bt.loop_count as i64),  ct.loop_count as i64,  Sign::Higher),
            diff_cell(Some(bt.temp_vars as i64),   ct.temp_vars as i64,   Sign::Lower),
            diff_cell(Some(bt.goto_count as i64),  ct.goto_count as i64,  Sign::Lower),
        );

        println!();
        let verdict = match (any_improvement, any_regression) {
            (true,  false) => "IMPROVED",
            (false, true)  => "REGRESSED",
            (true,  true)  => "MIXED (some improvements, some regressions)",
            (false, false) => "UNCHANGED",
        };
        println!("Verdict: {verdict}");
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

enum Sign { Higher, Lower, Neutral }

fn diff_cell(baseline: Option<i64>, current: i64, sign: Sign) -> String {
    match baseline {
        None => format!("{current:>6}"),
        Some(b) if b == current => format!("{current:>6}"),
        Some(b) => {
            let delta = current - b;
            let arrow = match sign {
                Sign::Neutral => "~",
                Sign::Higher  => if delta > 0 { "+" } else { "-" },
                Sign::Lower   => if delta < 0 { "+" } else { "-" },
            };
            format!("{current:>4}{arrow}{}", delta.unsigned_abs())
        }
    }
}

fn is_regression(b: &Metrics, c: &Metrics) -> bool {
    c.goto_count > b.goto_count || c.temp_vars > b.temp_vars
}

fn is_improvement(b: &Metrics, c: &Metrics) -> bool {
    c.goto_count  < b.goto_count
        || c.temp_vars   < b.temp_vars
        || c.if_count    > b.if_count
        || c.loop_count  > b.loop_count
        || c.stack_slots > b.stack_slots
}
