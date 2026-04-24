#!/usr/bin/env bash
# Regenerate repository icon assets from resources/icons/marvin.png.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ICON_DIR="${ROOT_DIR}/resources/icons"
SOURCE_PNG="${ICON_DIR}/marvin.png"
ICONSET_DIR="${ROOT_DIR}/target/marvin.iconset"

if [[ ! -f "${SOURCE_PNG}" ]]; then
  echo "Source icon not found: ${SOURCE_PNG}" >&2
  exit 1
fi

if ! command -v magick >/dev/null 2>&1; then
  echo "magick is required to generate ${ICON_DIR}/marvin.ico" >&2
  exit 1
fi

echo "Generating Windows icon from ${SOURCE_PNG}"
magick "${SOURCE_PNG}" \
  -define icon:auto-resize=16,24,32,48,64,128,256 \
  "${ICON_DIR}/marvin.ico"

if command -v sips >/dev/null 2>&1 && command -v iconutil >/dev/null 2>&1; then
  echo "Generating macOS icon from ${SOURCE_PNG}"
  rm -rf "${ICONSET_DIR}"
  mkdir -p "${ICONSET_DIR}"

  for size in 16 32 128 256 512; do
    sips -z "${size}" "${size}" "${SOURCE_PNG}" \
      --out "${ICONSET_DIR}/icon_${size}x${size}.png" >/dev/null
    sips -z "$((size * 2))" "$((size * 2))" "${SOURCE_PNG}" \
      --out "${ICONSET_DIR}/icon_${size}x${size}@2x.png" >/dev/null
  done

  iconutil -c icns "${ICONSET_DIR}" -o "${ICON_DIR}/marvin.icns"
else
  echo "sips/iconutil not available; leaving ${ICON_DIR}/marvin.icns unchanged"
fi
