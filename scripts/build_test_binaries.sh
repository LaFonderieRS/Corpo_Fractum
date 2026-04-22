#!/usr/bin/env bash
set -euo pipefail

# -----------------------------------------------------------------------------
# Build test binaries for the decompiler benchmark corpus.
#
# Supported inputs:
#   - C sources:         *.c
#   - GAS asm sources:   *.s, *.S
#   - NASM sources:      *.nasm.asm
#   - YASM sources:      *.yasm.asm
#
# Generated variants for C sources:
#   - direct GCC builds
#   - GCC -> GAS assembly builds
#   - GCC Intel syntax -> GAS builds
#   - GCC Intel syntax -> YASM (gas parser) builds, if YASM is available
#
# Notes:
#   - GCC -S -masm=intel does NOT generate NASM syntax.
#   - Therefore NASM is only used for native NASM sources in this script.
# -----------------------------------------------------------------------------

ROOT_DIR="tests/tests_binaries"
BIN_SUFFIX=".ELF_x8664"

# -----------------------------------------------------------------------------
# Helpers
# -----------------------------------------------------------------------------

log_info()  { echo "[INFO] $*"; }
log_ok()    { echo "[OK]   $*"; }
log_warn()  { echo "[WARN] $*"; }
log_err()   { echo "[ERR]  $*" >&2; }

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    log_err "Required command not found: $1"
    exit 1
  fi
}

has_cmd() {
  command -v "$1" >/dev/null 2>&1
}

verify_elf64() {
  local bin="$1"
  local info
  info="$(file "$bin")"
  if [[ "$info" != *"ELF 64-bit"* ]]; then
    log_err "Output is not ELF64: $bin"
    log_err "file output: $info"
    exit 1
  fi
}

# -----------------------------------------------------------------------------
# Environment checks
# -----------------------------------------------------------------------------

check_elf64_environment() {
  log_info "Checking ELF64 x86_64 build environment..."

  local arch
  arch="$(uname -m)"

  if [[ "$arch" != "x86_64" && "$arch" != "amd64" ]]; then
    log_err "Unsupported architecture: $arch"
    log_err "Expected x86_64 / amd64."
    exit 1
  fi

  local tmpdir src out info
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT

  src="$tmpdir/check.c"
  out="$tmpdir/check_bin"

  cat > "$src" <<'EOF'
int main(void) { return 0; }
EOF

  gcc "$src" -o "$out"
  info="$(file "$out")"

  if [[ "$info" != *"ELF 64-bit"* ]]; then
    log_err "This environment did not produce an ELF 64-bit binary."
    log_err "file output: $info"
    exit 1
  fi

  log_ok "ELF64 environment confirmed."
}

# -----------------------------------------------------------------------------
# Cleanup
# -----------------------------------------------------------------------------

clean_generated_files() {
  log_info "Cleaning previously generated binaries, objects and assembly files..."

  find "$ROOT_DIR" -type f \( \
      -name "*${BIN_SUFFIX}" -o \
      -name "*.generated.o" -o \
      -name "*.generated.s" \
    \) -print -delete || true

  log_ok "Cleanup complete."
}

# -----------------------------------------------------------------------------
# Common build helper
# -----------------------------------------------------------------------------

link_object_with_gcc() {
  local obj="$1"
  local out="$2"

  gcc "$obj" -o "$out"
  verify_elf64 "$out"
}

# -----------------------------------------------------------------------------
# C source builds
# -----------------------------------------------------------------------------

compile_c_direct_variants() {
  local src="$1"
  local dir base stem

  dir="$(dirname "$src")"
  base="$(basename "$src")"
  stem="${base%.c}"

  log_info "Building C source (direct GCC variants): $src"

  declare -a variants=(
    "gcc_O0|-O0 -g -fno-omit-frame-pointer"
    "gcc_O1|-O1 -g -fno-omit-frame-pointer"
    "gcc_O2|-O2 -g"
    "gcc_O3|-O3"
    "gcc_Os|-Os"
  )

  for entry in "${variants[@]}"; do
    local name flags out
    name="${entry%%|*}"
    flags="${entry#*|}"
    out="${dir}/${stem}_${name}${BIN_SUFFIX}"

    log_info "Building $out"
    gcc -Wall -Wextra $flags "$src" -o "$out"
    verify_elf64 "$out"
  done

  log_ok "Done with direct GCC variants: $src"
}

compile_c_via_gas_att() {
  local src="$1"
  local dir base stem asm obj out

  dir="$(dirname "$src")"
  base="$(basename "$src")"
  stem="${base%.c}"

  asm="${dir}/${stem}_gas_att.generated.s"
  obj="${dir}/${stem}_gas_att.generated.o"
  out="${dir}/${stem}_gas_att${BIN_SUFFIX}"

  log_info "Building GCC -> GAS (AT&T syntax): $out"

  gcc -S -O2 "$src" -o "$asm"
  gcc -c "$asm" -o "$obj"
  link_object_with_gcc "$obj" "$out"

  log_ok "Built: $out"
}

compile_c_via_gas_intel() {
  local src="$1"
  local dir base stem asm obj out

  dir="$(dirname "$src")"
  base="$(basename "$src")"
  stem="${base%.c}"

  asm="${dir}/${stem}_gas_intel.generated.s"
  obj="${dir}/${stem}_gas_intel.generated.o"
  out="${dir}/${stem}_gas_intel${BIN_SUFFIX}"

  log_info "Building GCC Intel syntax -> GAS: $out"

  gcc -S -O2 -masm=intel "$src" -o "$asm"
  gcc -c "$asm" -o "$obj"
  link_object_with_gcc "$obj" "$out"

  log_ok "Built: $out"
}

compile_c_via_yasm_gas_parser() {
  local src="$1"
  local dir base stem asm obj out

  if ! has_cmd yasm; then
    log_warn "YASM not found, skipping GCC Intel -> YASM(gas) for $src"
    return
  fi

  dir="$(dirname "$src")"
  base="$(basename "$src")"
  stem="${base%.c}"

  asm="${dir}/${stem}_yasm_gas.generated.s"
  obj="${dir}/${stem}_yasm_gas.generated.o"
  out="${dir}/${stem}_yasm_gas${BIN_SUFFIX}"

  log_info "Building GCC Intel syntax -> YASM (gas parser): $out"

  gcc -S -O2 -masm=intel "$src" -o "$asm"
  yasm -p gas -f elf64 -o "$obj" "$asm"
  link_object_with_gcc "$obj" "$out"

  log_ok "Built: $out"
}

compile_c_source() {
  local src="$1"

  compile_c_direct_variants "$src"
  compile_c_via_gas_att "$src"
  compile_c_via_gas_intel "$src"
  compile_c_via_yasm_gas_parser "$src"
}

# -----------------------------------------------------------------------------
# Native assembly builds
# -----------------------------------------------------------------------------

compile_gas_source() {
  local src="$1"
  local dir base stem out

  dir="$(dirname "$src")"
  base="$(basename "$src")"
  stem="${base%.*}"
  out="${dir}/${stem}_gas_native${BIN_SUFFIX}"

  log_info "Building native GAS source: $out"

  gcc "$src" -o "$out"
  verify_elf64 "$out"

  log_ok "Built: $out"
}

compile_nasm_source() {
  local src="$1"
  local dir base stem obj out

  if ! has_cmd nasm; then
    log_warn "NASM not found, skipping $src"
    return
  fi

  dir="$(dirname "$src")"
  base="$(basename "$src")"
  stem="${base%.nasm.asm}"

  obj="${dir}/${stem}_nasm.generated.o"
  out="${dir}/${stem}_nasm_native${BIN_SUFFIX}"

  log_info "Building native NASM source: $out"

  nasm -f elf64 "$src" -o "$obj"
  link_object_with_gcc "$obj" "$out"

  log_ok "Built: $out"
}

compile_yasm_source() {
  local src="$1"
  local dir base stem obj out

  if ! has_cmd yasm; then
    log_warn "YASM not found, skipping $src"
    return
  fi

  dir="$(dirname "$src")"
  base="$(basename "$src")"
  stem="${base%.yasm.asm}"

  obj="${dir}/${stem}_yasm.generated.o"
  out="${dir}/${stem}_yasm_native${BIN_SUFFIX}"

  log_info "Building native YASM source: $out"

  yasm -f elf64 "$src" -o "$obj"
  link_object_with_gcc "$obj" "$out"

  log_ok "Built: $out"
}

# -----------------------------------------------------------------------------
# Source dispatch
# -----------------------------------------------------------------------------

compile_source() {
  local src="$1"

  case "$src" in
    *.c)
      compile_c_source "$src"
      ;;
    *.s|*.S)
      compile_gas_source "$src"
      ;;
    *.nasm.asm)
      compile_nasm_source "$src"
      ;;
    *.yasm.asm)
      compile_yasm_source "$src"
      ;;
    *)
      log_warn "Unsupported source type, skipping: $src"
      ;;
  esac
}

# -----------------------------------------------------------------------------
# Main
# -----------------------------------------------------------------------------

main() {
  require_cmd gcc
  require_cmd file
  require_cmd uname
  require_cmd find

  if [[ ! -d "$ROOT_DIR" ]]; then
    log_err "Missing directory: $ROOT_DIR"
    exit 1
  fi

  check_elf64_environment
  clean_generated_files

  log_info "Scanning corpus sources under: $ROOT_DIR"

  local found=0

  while IFS= read -r -d '' src; do
    found=1
    compile_source "$src"
  done < <(
    find "$ROOT_DIR" -type f \( \
      -name "*.c" -o \
      -name "*.s" -o \
      -name "*.S" -o \
      -name "*.nasm.asm" -o \
      -name "*.yasm.asm" \
    \) -print0 | sort -z
  )

  if [[ "$found" -eq 0 ]]; then
    log_warn "No supported sources found."
    exit 1
  fi

  log_ok "All requested test binaries have been generated."
}

main "$@"
