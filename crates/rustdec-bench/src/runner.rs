use crate::model::{BenchCase, CaseResult, Metrics};
use std::time::Instant;

/// Run the full decompilation pipeline on one corpus entry.
pub fn run_case(case: &BenchCase) -> CaseResult {
    let start = Instant::now();

    let result: anyhow::Result<Metrics> = (|| {
        let obj    = rustdec_loader::load_file(&case.path)?;
        let module = rustdec_analysis::analyse(&obj)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let outputs = rustdec_codegen::emit_module(&module, rustdec_codegen::Language::C)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        Ok(crate::metrics::compute(&module, &outputs))
    })();

    let elapsed_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(metrics) => CaseResult {
            case:       case.name.clone(),
            success:    true,
            error:      None,
            elapsed_ms,
            metrics,
        },
        Err(e) => CaseResult {
            case:       case.name.clone(),
            success:    false,
            error:      Some(e.to_string()),
            elapsed_ms,
            metrics:    Metrics::default(),
        },
    }
}
