use crate::model::BenchCase;
use std::path::Path;

/// Discover all `*.ELF_x8664` files under `root/*/<name>.ELF_x8664`.
///
/// Each subdirectory of `root` is treated as one test case; the directory
/// name becomes `BenchCase::name`.  Cases are returned sorted by name.
pub fn discover(root: &Path) -> anyhow::Result<Vec<BenchCase>> {
    let mut cases = Vec::new();

    let entries = std::fs::read_dir(root)
        .map_err(|e| anyhow::anyhow!("cannot read corpus directory {}: {e}", root.display()))?;

    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() { continue; }

        let case_name = dir.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        // Find the first .ELF_x8664 file in the subdirectory.
        let elf = std::fs::read_dir(&dir)
            .ok()
            .and_then(|mut rd| rd.find(|e| {
                e.as_ref().ok().map_or(false, |e| {
                    e.path().extension()
                        .map_or(false, |ext| ext == "ELF_x8664")
                })
            }))
            .and_then(|e| e.ok())
            .map(|e| e.path());

        if let Some(path) = elf {
            cases.push(BenchCase { name: case_name, path });
        }
    }

    cases.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(cases)
}
