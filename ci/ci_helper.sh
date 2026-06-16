#!/usr/bin/env bash
# fixdecoder — unified CI helper for the Rust implementation

set -euo pipefail

# Resolve repository root (script lives in ci/).
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

function log() {
  printf "\n\033[1;32m%s\033[0m\n" "$1"
}

function warn() {
  printf "\n\033[38;5;214m%s\033[0m\n" "$1"
}

function ensure_llvm_tools_env() {
  if [[ -n "${LLVM_COV:-}" && -n "${LLVM_PROFDATA:-}" ]]; then
    return
  fi

  local sysroot host llvm_bin rustup_rustc toolchain_root
  sysroot="$(rustc --print sysroot)"
  host="$(rustc -vV | awk '/^host:/ {print $2}')"
  llvm_bin="${sysroot}/lib/rustlib/${host}/bin"

  if [[ -x "${llvm_bin}/llvm-cov" && -x "${llvm_bin}/llvm-profdata" ]]; then
    export LLVM_COV="${llvm_bin}/llvm-cov"
    export LLVM_PROFDATA="${llvm_bin}/llvm-profdata"
    return
  fi

  rustup_rustc="$(rustup which rustc 2>/dev/null || true)"
  if [[ -z "${rustup_rustc}" ]]; then
    return
  fi

  toolchain_root="$(cd "$(dirname "${rustup_rustc}")/.." && pwd)"
  shopt -s nullglob
  for llvm_bin in "${toolchain_root}"/lib/rustlib/*/bin; do
    if [[ -x "${llvm_bin}/llvm-cov" && -x "${llvm_bin}/llvm-profdata" ]]; then
      export LLVM_COV="${llvm_bin}/llvm-cov"
      export LLVM_PROFDATA="${llvm_bin}/llvm-profdata"
      shopt -u nullglob
      return
    fi
  done
  shopt -u nullglob
}

function ensure_sonar_token() {
  if [[ -n "${SONAR_TOKEN:-}" ]]; then
    return
  fi

  local token_file="${HOME}/.secrets/SONAR_TOKEN"
  if [[ -f "${token_file}" ]]; then
    SONAR_TOKEN="$(<"${token_file}")"
    export SONAR_TOKEN
    log ">> Loaded SONAR_TOKEN from ${token_file}"
    return
  fi

  warn "SONAR_TOKEN is not set and ${token_file} was not found."
  return 1
}

setup_done=false
function cmd_setup_environment() {
  if [[ "${setup_done}" == true ]]; then
    return
  fi
  log ">> Ensuring Rust toolchain and coverage tools"
  if ! command -v cargo >/dev/null 2>&1; then
    echo "cargo is not on PATH. Please install Rust (https://www.rust-lang.org/tools/install)." >&2
    exit 1
  fi
  if ! rustup component list --installed | grep -q 'llvm-tools-preview'; then
    log ">> Installing llvm-tools-preview component"
    rustup component add llvm-tools-preview >/dev/null
  fi
  if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
    log ">> Installing cargo-llvm-cov"
    # Avoid inheriting target-specific RUSTFLAGS (e.g., musl + crt-static) that break proc-macro builds.
    RUSTFLAGS="" cargo install cargo-llvm-cov --locked --quiet
  fi
  ensure_llvm_tools_env
  if ! command -v cargo-audit >/dev/null 2>&1; then
    log ">> Installing cargo-audit"
    cargo install cargo-audit --locked --quiet
  fi
  setup_done=true
}

function ensure_sonar_scanner() {
  if command -v sonar-scanner >/dev/null 2>&1; then
    return
  fi
  log ">> Installing sonar-scanner CLI locally"
  local tools_dir="${ROOT_DIR}/target/tools"
  local os="$(uname -s | tr '[:upper:]' '[:lower:]')"

  mkdir -p "${tools_dir}"

  # Try a set of known versions (newest first) because tags may disappear.
  local versions=("6.2.0.4872" "6.1.0.4477" "6.0.0.4380" "5.0.1.3006")
  local downloaded=""

  for version in "${versions[@]}"; do
    local archive urls=()
    case "${os}" in
      linux*)
        archive="/tmp/sonar-scanner-${version}-linux-x64.zip"
        urls+=(
          "https://binaries.sonarsource.com/Distribution/sonar-scanner-cli/sonar-scanner-cli-${version}-linux-x64.zip"
        )
        ;;
      darwin*)
        archive="/tmp/sonar-scanner-${version}-macosx.zip"
        urls+=(
          "https://binaries.sonarsource.com/Distribution/sonar-scanner-cli/sonar-scanner-cli-${version}-macosx.zip"
        )
        ;;
      msys*|mingw*|cygwin*)
        archive="/tmp/sonar-scanner-${version}-windows.zip"
        urls+=(
          "https://binaries.sonarsource.com/Distribution/sonar-scanner-cli/sonar-scanner-cli-${version}-windows.zip"
        )
        ;;
      *)
        warn "Unsupported OS for auto-installing sonar-scanner (${os}); please install manually."
        return 1
        ;;
    esac

    for url in "${urls[@]}"; do
      log "   attempting download: ${url}"
      if curl -fsSL -o "${archive}" "${url}"; then
        downloaded="${archive}"
        break
      fi
    done

    if [[ -n "${downloaded}" ]]; then
      break
    fi
  done

  if [[ -z "${downloaded}" ]]; then
    warn "Failed to download sonar-scanner; install manually or ensure it is on PATH."
    return 1
  fi

  unzip -qo "${downloaded}" -d "${tools_dir}"

  local candidate
  candidate="$(find "${tools_dir}" -maxdepth 3 -type f \( -name "sonar-scanner" -o -name "sonar-scanner.bat" \) | head -n 1 || true)"
  if [[ -z "${candidate}" ]]; then
    warn "sonar-scanner executable not found after extraction in ${tools_dir}"
    return 1
  fi

  local bin_dir
  bin_dir="$(dirname "${candidate}")"
  export PATH="${bin_dir}:${PATH}"
  log "   sonar-scanner installed locally at ${candidate}"
}

metadata_ready=false
function crate_version() {
  grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/'
}

function download_fix_specs() {
  log ">> Ensuring FIX XML specs are present"
  local resources_dir="${ROOT_DIR}/resources"
  mkdir -p "${resources_dir}"

  # Align with embedded dictionaries: 40,41,42,43,44,50,50SP1,50SP2,T11
  local specs=(
    "FIX40.xml"
    "FIX41.xml"
    "FIX42.xml"
    "FIX43.xml"
    "FIX44.xml"
    "FIX50.xml"
    "FIX50SP1.xml"
    "FIX50SP2.xml"
    "FIXT11.xml"
  )

  for spec in "${specs[@]}"; do
    local dest="${resources_dir}/${spec}"
    local url="https://raw.githubusercontent.com/quickfix/quickfix/master/spec/${spec}"
    if [[ -f "${dest}" ]]; then
      continue
    fi
    log "   downloading ${spec}"
    if ! curl -fsSL -o "${dest}" "${url}"; then
      echo "Failed to download ${spec} from ${url}" >&2
      exit 1
    fi
  done
}

function ensure_build_metadata() {
  if [[ "${metadata_ready}" == true ]]; then
    return
  fi

  local branch commit url
  branch=${FIXDECODER_BRANCH:-}
  commit=${FIXDECODER_COMMIT:-}
  url=${FIXDECODER_GIT_URL:-}

  if [[ -z "${branch}" ]]; then
    branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "main")
  fi
  if [[ -z "${commit}" ]]; then
    commit=$(git rev-parse --short HEAD 2>/dev/null || echo "0000000")
  fi
  if [[ -z "${url}" ]]; then
    url=$(git remote get-url origin 2>/dev/null || echo "https://github.com/stephenlclarke/fixdecoder.git")
  fi
  local version
  if [[ -n "${FIXDECODER_VERSION:-}" ]]; then
    version="${FIXDECODER_VERSION}"
  else
    version=$(git tag --list 'v[0-9]*' --sort=-version:refname | head -n 1 || true)
    if [[ -z "${version}" ]]; then
      local crate_ver
      if ! crate_ver=$(crate_version); then
        echo "Unable to determine crate version from Cargo.toml" >&2
        exit 1
      fi
      version="v${crate_ver}"
    fi
  fi
  if [[ -n "$(git status --porcelain 2>/dev/null || true)" && "${version}" != *-dirty ]]; then
    version="${version}-dirty"
  fi

  export FIXDECODER_BRANCH="${branch}"
  export FIXDECODER_COMMIT="${commit}"
  export FIXDECODER_GIT_URL="${url}"
  export FIXDECODER_VERSION="${version}"

  metadata_ready=true
}

function ensure_rustup_target() {
  local target="$1"
  local rustup_rustc toolchain_root
  if rustup target list --installed | grep -qx "${target}"; then
    return
  fi
  rustup_rustc="$(rustup which rustc 2>/dev/null || true)"
  if [[ -n "${rustup_rustc}" ]]; then
    toolchain_root="$(cd "$(dirname "${rustup_rustc}")/.." && pwd)"
    if [[ -d "${toolchain_root}/lib/rustlib/${target}" ]]; then
      return
    fi
  fi

  log ">> Installing Rust target ${target}"
  rustup target add "${target}"
}

function active_rustup_toolchain() {
  rustup show active-toolchain | awk '{print $1}'
}

function cargo_with_rustup() {
  local toolchain
  toolchain="$(active_rustup_toolchain)"
  if [[ -z "${toolchain}" ]]; then
    echo "Unable to determine the active rustup toolchain." >&2
    exit 1
  fi

  RUSTC="$(rustup which rustc)" rustup run "${toolchain}" cargo "$@"
}

function ensure_windows_cross_tools() {
  if ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
    echo "x86_64-w64-mingw32-gcc is required for make build-windows." >&2
    exit 1
  fi
  if ! command -v x86_64-w64-mingw32-windres >/dev/null 2>&1; then
    echo "x86_64-w64-mingw32-windres is required for Windows icon resources." >&2
    exit 1
  fi
}

# This script is intended to be sourced by the Makefile or ad-hoc bash
# invocations. Call helpers such as `cmd_setup_environment`, `ensure_build_metadata`,
# `download_fix_specs`, `ensure_sonar_scanner`, and `ensure_sonar_token` from targets.
