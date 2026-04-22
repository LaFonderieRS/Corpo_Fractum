# rustdec-bench

Benchmark CLI for the RustDec decompilation pipeline.

Runs the full pipeline (loader → analysis → codegen) against a corpus of ELF64
binaries, records metrics, writes JSON, and compares against a saved baseline so
regressions are caught before they ship.

## Quick start

```bash
# First run — execute corpus and save as the new baseline
cargo run -p rustdec-bench -- run --save-baseline

# After making changes — compare against the baseline
cargo run -p rustdec-bench -- run
cargo run -p rustdec-bench -- compare
```

## Commands

### `run` — execute the corpus

```bash
cargo run -p rustdec-bench -- run [OPTIONS]
```

Options:

| Flag | Default | Description |
|---|---|---|
| `--output` | `bench/results/current.json` | Where to write the JSON report |
| `--corpus` | `tests/tests_binaries` | Root directory of the test corpus |
| `--save-baseline` | off | Also overwrite `bench/baselines/latest.json` |

Example output:

```
Corpus: 4 cases from tests/tests_binaries

  hello_world                     ok  (2 ms)
  retrieve_argc                   ok  (1 ms)
  simple_if                       ok  (1 ms)
  simple_if_argv                  ok  (1 ms)

Totals — functions:9 slots:4 if:3 loops:0 temps:12 goto:0

Results written to bench/results/current.json
```

---

### `compare` — diff current vs baseline

```bash
cargo run -p rustdec-bench -- compare [OPTIONS]
```

Options:

| Flag | Default | Description |
|---|---|---|
| `--baseline` | `bench/baselines/latest.json` | Baseline to compare against |
| `--current` | `bench/results/current.json` | Current run to evaluate |

Example output:

```
Baseline : 2026-04-20T10:00:00Z (abc1234)
Current  : 2026-04-22T14:00:00Z (def5678)

case                     funcs   slots     if   loop   vars   goto
------------------------------------------------------------------
hello_world                  2   0→ 2      0       0    8→5       0
retrieve_argc                2   0→ 2      0       0   27→18      0
simple_if                    2   0→ 2      0       0    8→5       0
simple_if_argv               3   0→ 3    1→2       0   25→16      0
------------------------------------------------------------------
TOTALS                       9   0→ 9    1→3       0   68→44      0

Verdict: IMPROVED
```

**Metric conventions** — `+` means the direction improved, `-` means it regressed:

| Metric | Better when |
|---|---|
| `slots` | higher (more stack variables recognised) |
| `if` | higher (more structured branches) |
| `loops` | higher (more loops detected) |
| `vars` | lower (fewer SSA temporaries in output) |
| `goto` | lower (less fallback spaghetti) |

---

### `report` — summary of a result file

```bash
cargo run -p rustdec-bench -- report [OPTIONS]
```

Options:

| Flag | Default | Description |
|---|---|---|
| `--input` | `bench/results/current.json` | Report file to display |

Example output:

```
Report: 2026-04-22T14:00:00Z (def5678)

  Cases: 4 ok, 0 failed

case                        ms    fns  slots    if  loops   vars
------------------------------------------------------------
hello_world                  2      2      2     0      0      5
retrieve_argc                1      2      2     0      0     18
simple_if                    1      2      2     0      0      5
simple_if_argv               1      3      3     2      0     16

Totals — functions:9 slots:9 if:2 loops:0 temps:44 goto:0
```

---

## Corpus layout

Each subdirectory of `tests/tests_binaries/` is one test case.
The directory name is used as the case identifier.
The first `*.ELF_x8664` file found in the directory is the binary under test.

```
tests/tests_binaries/
  hello_world/
    hello_world.ELF_x8664   ← analysed
    hello_world.c           ← reference source (not used by bench)
  simple_if/
    simple_if.ELF_x8664
    simple_if.c
  ...
```

To add a new case: create a subdirectory and drop a compiled ELF64 binary in it.

---

## Baseline workflow

```bash
# 1. Establish a clean baseline on main
cargo run -p rustdec-bench -- run --save-baseline
git add bench/baselines/latest.json
git commit -m "bench: update baseline"

# 2. Develop a fix or feature on a branch

# 3. Measure the impact
cargo run -p rustdec-bench -- run
cargo run -p rustdec-bench -- compare
# → Verdict: IMPROVED / REGRESSED / UNCHANGED / MIXED

# 4. If the improvement is intentional, promote to new baseline
cargo run -p rustdec-bench -- run --save-baseline
```

`bench/results/` is gitignored (transient outputs).
`bench/baselines/` is committed (reference snapshots).

---

## JSON output format

```json
{
  "timestamp": "2026-04-22T14:00:00Z",
  "git_hash": "def5678",
  "cases": [
    {
      "case": "hello_world",
      "success": true,
      "elapsed_ms": 2,
      "metrics": {
        "functions":   2,
        "stack_slots": 2,
        "if_count":    0,
        "loop_count":  0,
        "temp_vars":   5,
        "goto_count":  0
      }
    }
  ],
  "totals": { "functions": 9, "stack_slots": 9, ... }
}
```

Failed cases include an `"error"` field with the pipeline error message.

---

## Dependencies

- [`rustdec-loader`](../rustdec-loader)
- [`rustdec-analysis`](../rustdec-analysis)
- [`rustdec-codegen`](../rustdec-codegen)
- [`clap`](https://crates.io/crates/clap) — CLI argument parsing
- [`serde_json`](https://crates.io/crates/serde_json) — JSON serialisation
