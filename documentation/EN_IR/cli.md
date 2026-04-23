# CLI reference

`rustdec-cli` provides a headless interface to the full analysis pipeline. The binary is named `corpo-fractum-cli`.

---

## Build

```bash
cargo build --release -p rustdec-cli
# → target/release/corpo-fractum-cli
```

---

## Synopsis

```
corpo-fractum-cli [OPTIONS] <BINARY>
```

---

## Options

| Flag | Type | Default | Description |
|---|---|---|---|
| `-l, --lang <LANG>` | `c` \| `cpp` \| `rust` | `c` | Output language |
| `-o, --output <DIR>` | path | — | Write one `.c`/`.cpp`/`.rs` file per function into DIR |
| `-F, --function <NAME>` | string (repeatable) | — | Only decompile the named function(s) |
| `--list` | flag | — | List detected functions and exit without lifting |
| `--emit-ir` | flag | — | Dump the lifted SSA IR instead of decompiled code |
| `-v` / `-vv` / `-vvv` | flag (stackable) | — | Verbosity: info / debug / trace |

---

## Subcommands (implicit)

The mode is selected by which flags are present:

| Flags present | Mode | What runs |
|---|---|---|
| `--list` | list | loader + disasm + function detection only |
| `--emit-ir` | IR dump | full pipeline up to and including lifting |
| *(neither)* | decompile | full pipeline including codegen |

---

## Examples

```bash
# List all detected functions with their entry addresses
corpo-fractum-cli --list ./target_binary

# Decompile a single function to C (printed to stdout)
corpo-fractum-cli -F main ./target_binary

# Decompile two functions to Rust (printed to stdout)
corpo-fractum-cli -l rust -F main -F compute ./target_binary

# Decompile everything and write one file per function
corpo-fractum-cli -o ./out ./target_binary

# Dump the raw SSA IR for debugging a specific function
corpo-fractum-cli --emit-ir -F parse_args ./target_binary

# Verbose analysis log (debug level)
corpo-fractum-cli -vv ./target_binary

# Override log filter via environment variable
RUSTDEC_LOG=rustdec_lift=trace corpo-fractum-cli ./target_binary
```

---

## Output format

### `--list` mode

```
0x00401180  main
0x00401240  compute
0x004012f0  parse_args
0x00401390  sub_401390
```

### Default (decompile) mode, stdout

```c
// ── main ──
uint64_t main(int argc, char **argv) {
    uint64_t local_0;
    …
}

// ── compute ──
uint64_t compute(uint64_t arg_0) {
    …
}
```

### `--output <DIR>` mode

One file per function, named after the function:

```
out/
├── main.c
├── compute.c
└── parse_args.c
```

Functions from the CRT filter list (e.g. `_start`, `__libc_csu_init`) are always excluded.

---

## Logging

Log output goes to stderr and is independent of decompiled code on stdout. This means stdout can be piped cleanly:

```bash
corpo-fractum-cli ./binary 2>/dev/null | grep "uint64_t"
```

The `RUSTDEC_LOG` environment variable accepts the same filter syntax as `RUST_LOG` (from the `tracing-subscriber` crate):

```bash
RUSTDEC_LOG=debug                         # all crates at debug
RUSTDEC_LOG=rustdec_lift=trace,info       # lift at trace, rest at info
RUSTDEC_LOG=rustdec_analysis=debug        # analysis only
```

---

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Analysis error (bad binary, unsupported format, etc.) |
| `2` | I/O error (file not found, output directory not writable) |
