# corpo-fractum-cli

Command-line front-end for [Corpo Fractum](../README.md) — decompiles x86-64 ELF, PE and Mach-O binaries to C, C++ or Rust pseudo-code.

## Installation

```sh
# from the workspace root
cargo install --path corpo-fractum-cli
```

Or build without installing:

```sh
cargo build --bin corpo-fractum-cli
# binary at: target/release/corpo-fractum-cli
```

## Usage

```
corpo-fractum-cli [OPTIONS] <BINARY>
```

### Options

|           Flag          |                            Description                           |
|-------------------------|------------------------------------------------------------------|
| `-l, --lang <LANG>`     | Output language: `c` (default), `cpp`, `rust`                    |
| `-o, --output <DIR>`    | Write one `<function>.<ext>` file per function into DIR          |
| `-F, --function <NAME>` | Decompile only this function (repeatable)                        |
| `--list`                | List detected functions and exit — fast, no full analysis        |
| `--emit-ir`             | Dump the lifted IR instead of decompiled source (debug)          |
| `-v / -vv / -vvv`       | Log verbosity: info / debug / trace                              |
| `RUSTDEC_LOG=<filter>`  | Override verbosity via env variable (takes precedence over `-v`) |

### Examples

```sh
# Decompile everything to stdout
corpo-fractum-cli ./target_binary

# Decompile to a directory, one file per function
corpo-fractum-cli ./target_binary -o decompiled/

# Only decompile `main`
corpo-fractum-cli ./target_binary -F main

# List functions without a full analysis pass
corpo-fractum-cli ./target_binary --list

# Rust output for two specific functions
corpo-fractum-cli ./target_binary -l rust -F parse_args -F run

# Show the lifted IR (useful when hacking on the analysis pipeline)
corpo-fractum-cli ./target_binary --emit-ir -F my_func

# Debug-level logging
corpo-fractum-cli ./target_binary -vv
# or
RUSTDEC_LOG=debug corpo-fractum-cli ./target_binary
```

## Exit codes

| Code |                         Meaning                           |
|------|-----------------------------------------------------------|
| 0    | Success                                                   |
| 1    | Any error (load failure, unsupported arch, codegen error) |

Error messages are written to stderr; decompiled source goes to stdout (or to files when `-o` is used).
