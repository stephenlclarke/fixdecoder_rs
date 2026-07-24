#!/usr/bin/env bash
set -euo pipefail

# Print command usage.
usage() {
  cat <<'USAGE'
Usage: capture_and_decode.sh <ssh_user@host> <tcpdump_host> <port>

Example:
  ./scripts/capture_and_decode.sh user@integration.example.com 192.168.1.10 1234

Notes:
  - <port> is used in both the tcpdump filter and the pcap2fix --port argument.
  - Assumes fixdecoder and pcap2fix binaries are available at ./target/release/.
USAGE
}

if [[ $# -lt 3 ]]; then
  usage
  exit 1
fi

SSH_TARGET="$1"
TCP_HOST="$2"
PORT="$3"
shift 3
FIXDECODER_ARGS=("$@")

if [[ ! "${SSH_TARGET}" =~ ^[A-Za-z0-9._@:%+-]+$ || "${SSH_TARGET}" == -* ]]; then
  printf 'error: invalid SSH target: %s\n' "${SSH_TARGET}" >&2
  exit 1
fi
if [[ ! "${TCP_HOST}" =~ ^[A-Za-z0-9._:%+-]+$ || "${TCP_HOST}" == -* ]]; then
  printf 'error: invalid tcpdump host: %s\n' "${TCP_HOST}" >&2
  exit 1
fi
if [[ ! "${PORT}" =~ ^[0-9]+$ ]]; then
  printf 'error: port must be between 1 and 65535: %s\n' "${PORT}" >&2
  exit 1
fi
PORT=$((10#${PORT}))
if ((PORT < 1 || PORT > 65535)); then
  printf 'error: port must be between 1 and 65535: %s\n' "${PORT}" >&2
  exit 1
fi

printf -v REMOTE_FILTER '%q' "(host ${TCP_HOST} and tcp port ${PORT})"
REMOTE_CMD="sudo tcpdump -U -n -s0 -i any -w - ${REMOTE_FILTER}"

# Resolve binaries: prefer explicit env override, then PATH, then local release build.
# Resolve one executable without evaluating user-provided values.
resolve_bin() {
  local env_path="$1"
  local name="$2"
  local local_fallback="$3"

  if [[ -n "${env_path}" ]]; then
    printf '%s\n' "${env_path}"
    return
  fi

  if command -v "${name}" >/dev/null 2>&1; then
    command -v "${name}"
    return
  fi

  printf '%s\n' "${local_fallback}"
}

PCAP2FIX_BIN="$(resolve_bin "${PCAP2FIX_BIN:-}" pcap2fix ./target/release/pcap2fix)"
FIXDECODER_BIN="$(resolve_bin "${FIXDECODER_BIN:-}" fixdecoder ./target/release/fixdecoder)"

if [[ ! -x "${PCAP2FIX_BIN}" || ! -x "${FIXDECODER_BIN}" ]]; then
  printf 'error: expected binaries at %s and %s. Build them first (make build-release).\n' \
    "${PCAP2FIX_BIN}" "${FIXDECODER_BIN}" >&2
  exit 1
fi

ssh -- "${SSH_TARGET}" "${REMOTE_CMD}" \
  | "${PCAP2FIX_BIN}" --strict --port "${PORT}" \
  | "${FIXDECODER_BIN}" --follow "${FIXDECODER_ARGS[@]}"
