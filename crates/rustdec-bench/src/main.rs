mod compare;
mod corpus;
mod metrics;
mod model;
mod runner;

use anyhow::Context;
use clap::{Parser, Subcommand};
use model::BenchReport;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "rustdec-bench", about = "RustDec decompilation benchmark")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the corpus and write results to JSON.
    Run {
        /// Where to write the results (JSON).
        #[arg(long, default_value = "bench/results/current.json")]
        output: PathBuf,
        /// Root of the test corpus.
        #[arg(long, default_value = "tests/tests_binaries")]
        corpus: PathBuf,
        /// Also save the result as the new baseline.
        #[arg(long)]
        save_baseline: bool,
    },
    /// Compare current results against a baseline.
    Compare {
        #[arg(long, default_value = "bench/baselines/latest.json")]
        baseline: PathBuf,
        #[arg(long, default_value = "bench/results/current.json")]
        current: PathBuf,
    },
    /// Print a human-readable summary of a result file.
    Report {
        #[arg(long, default_value = "bench/results/current.json")]
        input: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run { output, corpus, save_baseline } => {
            cmd_run(&corpus, &output, save_baseline)
        }
        Commands::Compare { baseline, current } => {
            cmd_compare(&baseline, &current)
        }
        Commands::Report { input } => {
            cmd_report(&input)
        }
    }
}

// ── run ───────────────────────────────────────────────────────────────────────

fn cmd_run(corpus_dir: &PathBuf, output: &PathBuf, save_baseline: bool) -> anyhow::Result<()> {
    let cases = corpus::discover(corpus_dir)
        .with_context(|| format!("loading corpus from {}", corpus_dir.display()))?;

    if cases.is_empty() {
        println!("No binaries found in {}", corpus_dir.display());
        return Ok(());
    }

    println!("Corpus: {} cases from {}", cases.len(), corpus_dir.display());
    println!();

    let mut results = Vec::with_capacity(cases.len());
    for case in &cases {
        let r = runner::run_case(case);
        let status = if r.success {
            format!("ok  ({} ms)", r.elapsed_ms)
        } else {
            format!("ERR ({}) ", r.error.as_deref().unwrap_or("?"))
        };
        println!("  {:<30}  {status}", case.name);
        results.push(r);
    }

    let report = BenchReport::new(results);

    print_totals(&report);

    // Write JSON output
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&report)?;
    std::fs::write(output, &json)
        .with_context(|| format!("writing {}", output.display()))?;
    println!("\nResults written to {}", output.display());

    if save_baseline {
        let baseline = PathBuf::from("bench/baselines/latest.json");
        if let Some(p) = baseline.parent() { std::fs::create_dir_all(p)?; }
        std::fs::write(&baseline, &json)?;
        println!("Baseline updated: {}", baseline.display());
    }

    Ok(())
}

// ── compare ───────────────────────────────────────────────────────────────────

fn cmd_compare(baseline_path: &PathBuf, current_path: &PathBuf) -> anyhow::Result<()> {
    let baseline: BenchReport = load_report(baseline_path)
        .with_context(|| format!("loading baseline {}", baseline_path.display()))?;
    let current: BenchReport = load_report(current_path)
        .with_context(|| format!("loading current {}", current_path.display()))?;

    compare::CompareReport::new(&baseline, &current).print();
    Ok(())
}

// ── report ────────────────────────────────────────────────────────────────────

fn cmd_report(input: &PathBuf) -> anyhow::Result<()> {
    let report: BenchReport = load_report(input)
        .with_context(|| format!("loading {}", input.display()))?;

    println!("Report: {} ({})", report.timestamp, report.git_hash.as_deref().unwrap_or("?"));
    println!();

    let ok  = report.cases.iter().filter(|c| c.success).count();
    let err = report.cases.iter().filter(|c| !c.success).count();
    println!("  Cases: {ok} ok, {err} failed");

    println!();
    println!("{:<22}  {:>6}  {:>5}  {:>5}  {:>4}  {:>5}  {:>5}",
        "case", "ms", "fns", "slots", "if", "loops", "vars");
    println!("{}", "-".repeat(60));

    for c in &report.cases {
        let m = &c.metrics;
        let flag = if c.success { "" } else { "  [FAIL]" };
        println!("{:<22}  {:>6}  {:>5}  {:>5}  {:>4}  {:>5}  {:>5}{flag}",
            c.case, c.elapsed_ms, m.functions, m.stack_slots,
            m.if_count, m.loop_count, m.temp_vars);
    }

    print_totals(&report);
    Ok(())
}

// ── shared helpers ────────────────────────────────────────────────────────────

fn load_report(path: &PathBuf) -> anyhow::Result<BenchReport> {
    let text = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

fn print_totals(report: &BenchReport) {
    let t = &report.totals;
    println!();
    println!("Totals — functions:{} slots:{} if:{} loops:{} temps:{} goto:{}",
        t.functions, t.stack_slots, t.if_count, t.loop_count, t.temp_vars, t.goto_count);
}
