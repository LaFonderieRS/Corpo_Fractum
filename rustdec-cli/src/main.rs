//! # corpo-fractum-cli
//!
//! Command-line front-end for the Corpo Fractum decompiler.
//!
//! ```text
//! Usage: corpo-fractum-cli [OPTIONS] <BINARY>
//!
//! Options:
//!   -l, --lang <LANG>          Output language [default: c] [values: c, cpp, rust]
//!   -o, --output <DIR>         Write one file per function into DIR
//!   -F, --function <NAME>...   Only decompile the named function(s)
//!       --list                 List detected functions and exit (no full analysis)
//!       --emit-ir              Dump the lifted IR instead of decompiled code
//!   -v, --verbose...           Increase log verbosity (-v info, -vv debug, -vvv trace)
//!   -h, --help
//!   -V, --version
//! ```

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use rustdec_analysis::analyse;
use rustdec_codegen::{emit_module, Language};
use rustdec_loader::load_file;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

// ── CLI arguments ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Lang {
    C,
    Cpp,
    Rust,
}

impl From<Lang> for Language {
    fn from(l: Lang) -> Self {
        match l {
            Lang::C    => Language::C,
            Lang::Cpp  => Language::Cpp,
            Lang::Rust => Language::Rust,
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name    = "corpo-fractum-cli",
    about   = "Corpo Fractum — binary decompiler",
    version,
    arg_required_else_help = true,
)]
struct Args {
    /// Binary to decompile (ELF, PE, Mach-O)
    binary: PathBuf,

    /// Target output language
    #[arg(short, long, value_enum, default_value = "c")]
    lang: Lang,

    /// Write one file per function into this directory instead of printing to stdout
    #[arg(short, long, value_name = "DIR")]
    output: Option<PathBuf>,

    /// Decompile only the named function(s) (repeatable: -F main -F helper)
    #[arg(short = 'F', long = "function", value_name = "NAME")]
    functions: Vec<String>,

    /// List detected function entry points and exit (fast — skips full analysis)
    #[arg(long)]
    list: bool,

    /// Dump the lifted IR for each function instead of decompiled source
    #[arg(long)]
    emit_ir: bool,

    /// Increase log verbosity (-v = info, -vv = debug, -vvv = trace)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let args = Args::parse();
    init_logging(args.verbose);

    let obj = load_file(&args.binary)
        .with_context(|| format!("cannot load '{}'", args.binary.display()))?;

    if args.list {
        return cmd_list(&obj);
    }

    let module = analyse(&obj).context("analysis failed")?;

    if args.emit_ir {
        return cmd_emit_ir(&module, &args.functions);
    }

    cmd_decompile(&module, args.lang.into(), args.output.as_deref(), &args.functions)
}

// ── Sub-commands ──────────────────────────────────────────────────────────────

/// Fast function listing — only disassembles and runs the function-detection
/// pass; no CFG construction, no lifting.
fn cmd_list(obj: &rustdec_loader::BinaryObject) -> Result<()> {
    use rustdec_disasm::Disassembler;

    let disasm = Disassembler::for_arch(obj.arch)
        .context("unsupported architecture")?;

    let mut all_insns = Vec::new();
    for sec in obj.code_sections() {
        match disasm.disassemble(&sec.data, sec.virtual_addr) {
            Ok(insns) => all_insns.extend(insns),
            Err(e)    => tracing::warn!(section = %sec.name, error = %e, "disassembly failed"),
        }
    }
    all_insns.sort_by_key(|i| i.address);

    let funcs = rustdec_analysis::detect_functions(obj, &all_insns);
    for (addr, name) in &funcs {
        println!("{addr:#018x}  {name}");
    }
    Ok(())
}

/// Dump the lifted IR (useful during development and for debugging the analysis
/// pipeline before the codegen stage is complete for a given construct).
fn cmd_emit_ir(module: &rustdec_ir::IrModule, filter: &[String]) -> Result<()> {
    for func in &module.functions {
        if !filter.is_empty() && !filter.contains(&func.name) {
            continue;
        }
        println!("=== {} @ {:#x} ===", func.name, func.entry_addr);
        for block in func.blocks_sorted() {
            println!("  block {:?}  (start {:#x}):", block.id, block.start_addr);
            for stmt in &block.stmts {
                println!("    {stmt:?}");
            }
            println!("    term: {:?}", block.terminator);
        }
        println!();
    }
    Ok(())
}

/// Full decompilation — emits source in the chosen language.
fn cmd_decompile(
    module:   &rustdec_ir::IrModule,
    lang:     Language,
    output:   Option<&std::path::Path>,
    filter:   &[String],
) -> Result<()> {
    let mut results = emit_module(module, lang).context("code generation failed")?;

    if !filter.is_empty() {
        results.retain(|(name, _)| filter.contains(name));
        if results.is_empty() {
            anyhow::bail!(
                "none of the requested functions were found: {:?}",
                filter
            );
        }
    }

    match output {
        None      => print_to_stdout(&results),
        Some(dir) => write_to_dir(&results, dir, lang),
    }
}

// ── Output helpers ────────────────────────────────────────────────────────────

fn print_to_stdout(results: &[(String, String)]) -> Result<()> {
    let multi = results.len() > 1;
    for (name, src) in results {
        if multi {
            println!("/* ── {name} ── */");
        }
        println!("{src}");
    }
    Ok(())
}

fn write_to_dir(
    results: &[(String, String)],
    dir:     &std::path::Path,
    lang:    Language,
) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("cannot create output directory '{}'", dir.display()))?;

    let ext = match lang {
        Language::C    => "c",
        Language::Cpp  => "cpp",
        Language::Rust => "rs",
    };

    for (name, src) in results {
        let safe: String = name
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
            .collect();
        let path = dir.join(format!("{safe}.{ext}"));
        std::fs::write(&path, src)
            .with_context(|| format!("cannot write '{}'", path.display()))?;
        eprintln!("wrote {}", path.display());
    }
    Ok(())
}

// ── Logging ───────────────────────────────────────────────────────────────────

fn init_logging(verbose: u8) {
    // RUSTDEC_LOG env variable takes precedence over -v flags.
    let filter = EnvFilter::try_from_env("RUSTDEC_LOG").unwrap_or_else(|_| {
        let level = match verbose {
            0 => "warn",
            1 => "info",
            2 => "debug",
            _ => "trace",
        };
        EnvFilter::new(level)
    });

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}
