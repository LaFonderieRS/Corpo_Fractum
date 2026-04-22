#!/usr/bin/env bash
set -euo pipefail

# Root directory containing test cases
ROOT_DIR="tests/tests_binaries"

# Output binary suffix
BIN_SUFFIX=".ELF_x8664"

#######################################
# Check required commands are present
#######################################
require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "[ERROR] Required command not found: $1"
    exit 1
  fi
}

#######################################
# Validate we are in an ELF64 environment
#######################################
check_elf64_environment() {
  echo "[INFO] Checking ELF64 environment..."

  local arch
  arch="$(uname -m)"

  if [[ "$arch" != "x86_64" && "$arch" != "amd64" ]]; then
    echo "[ERROR] Unsupported architecture: $arch"
    echo "[ERROR] Expected x86_64 environment."
    exit 1
  fi

  local tmpdir src out info
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT

  src="$tmpdir/test.c"
  out="$tmpdir/test_bin"

  cat > "$src" <<'EOF'
int main(void) { return 0; }
EOF

  gcc "$src" -o "$out"

  info="$(file "$out")"

  if [[ "$info" != *"ELF 64-bit"* ]]; then
    echo "[ERROR] Environment did not produce ELF 64-bit binary."
    echo "[ERROR] file output: $info"
    exit 1
  fi

  echo "[OK] ELF64 environment confirmed."
}

#######################################
# Remove previously generated binaries
#######################################
clean_old_binaries() {
  echo "[INFO] Cleaning previous binaries..."

  find "$ROOT_DIR" -type f -name "*${BIN_SUFFIX}" -print -delete || true

  echo "[OK] Cleanup done."
}

#######################################
# Compile a single C source into variants
#######################################
compile_source() {
  local src="$1"
  local dir base stem

  dir="$(dirname "$src")"
  base="$(basename "$src")"
  stem="${base%.c}"

  echo "[INFO] Compiling source: $src"

  # Compilation variants
  declare -a variants=(
    "O0|-O0 -g -fno-omit-frame-pointer"
    "O1|-O1 -g -fno-omit-frame-pointer"
    "O2|-O2 -g"
    "O3|-O3"
    "Os|-Os"
  )

  for entry in "${variants[@]}"; do
    local name flags out

    name="${entry%%|*}"
    flags="${entry#*|}"
    out="${dir}/${stem}_${name}${BIN_SUFFIX}"

    echo "[BUILD] -> $out"

    gcc -Wall -Wextra $flags "$src" -o "$out"

    local info
    info="$(file "$out")"

    if [[ "$info" != *"ELF 64-bit"* ]]; then
      echo "[ERROR] Output is not ELF64: $out"
      echo "[ERROR] file output: $info"
      exit 1
    fi
  done

  echo "[OK] Done: $src"
}

#######################################
# Main execution
#######################################
main() {
  require_cmd gcc
  require_cmd file
  require_cmd uname
  require_cmd find

  if [[ ! -d "$ROOT_DIR" ]]; then
    echo "[ERROR] Missing directory: $ROOT_DIR"
    exit 1
  fi

  check_elf64_environment
  clean_old_binaries

  echo "[INFO] Scanning for C sources..."

  local found=0

  while IFS= read -r -d '' src; do
    found=1
    compile_source "$src"
  done < <(find "$ROOT_DIR" -type f -name "*.c" -print0 | sort -z)

  if [[ "$found" -eq 0 ]]; then
    echo "[WARNING] No C files found."
    exit 1
  fi

  echo "[SUCCESS] All test binaries generated."
}

main "$@"
