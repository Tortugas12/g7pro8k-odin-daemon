#!/usr/bin/env bash
set -euo pipefail

# Build a statically linked aarch64-unknown-linux-musl binary for AYN Odin 3 Pro (Snapdragon 8 Elite / arm64)
# and generate the all-in-one self-extracting root script (g7pro8k-odin3-oneclick.sh).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
TARGET="aarch64-unknown-linux-musl"
DIST_DIR="${PROJECT_ROOT}/dist/odin3-arm64"

cd "${PROJECT_ROOT}"

echo "==> Building g7pro8k-odin-daemon for ${TARGET} (static musl)"

if command -v cargo-zigbuild >/dev/null 2>&1; then
  echo "Using cargo-zigbuild..."
  cargo zigbuild --release --target "${TARGET}"
elif command -v cross >/dev/null 2>&1; then
  echo "Using cross..."
  cross build --release --target "${TARGET}"
else
  if ! rustup target list --installed | grep -qx "${TARGET}"; then
    echo "Installing rustup target ${TARGET}..."
    rustup target add "${TARGET}"
  fi
  if command -v aarch64-linux-musl-gcc >/dev/null 2>&1; then
    export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER="aarch64-linux-musl-gcc"
    export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static -C link-arg=-s"
  elif command -v aarch64-linux-gnu-gcc >/dev/null 2>&1; then
    export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER="aarch64-linux-gnu-gcc"
    export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static -C link-arg=-s"
  else
    echo "Using rustc's bundled rust-lld linker (zero external cross-compiler needed)..."
    export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static -C linker=rust-lld -C linker-flavor=ld.lld -C link-arg=-s"
  fi
  cargo build --release --target "${TARGET}"
fi

BIN_PATH="${PROJECT_ROOT}/target/${TARGET}/release/g7pro8k-odin-daemon"
if [ ! -f "${BIN_PATH}" ]; then
  echo "ERROR: Expected binary not found at ${BIN_PATH}" >&2
  exit 1
fi

mkdir -p "${DIST_DIR}"
cp "${BIN_PATH}" "${DIST_DIR}/g7pro8k-odin-daemon"
cp "${PROJECT_ROOT}/config.example.json" "${DIST_DIR}/config.json"
cp "${PROJECT_ROOT}/scripts/g7pro8k-ctl.sh" "${DIST_DIR}/g7pro8k-ctl.sh"
chmod +x "${DIST_DIR}/g7pro8k-odin-daemon" "${DIST_DIR}/g7pro8k-ctl.sh"

# Generate the single-file self-extracting Odin 3 Pro root script + stop script.
"${SCRIPT_DIR}/package-single-script.sh" "${BIN_PATH}" "${DIST_DIR}"

echo "==> Staged AYN Odin 3 Pro bundle in ${DIST_DIR}:"
ls -lh "${DIST_DIR}"
echo ""
echo "Option A (Easiest — No ADB/PC needed): Copy g7pro8k-odin3-oneclick.sh onto your Odin 3 Pro"
echo "and run it via Odin Settings -> Run script as root."
echo ""
echo "Option B (ADB push):"
echo "  adb shell 'su -c \"mkdir -p /data/local/tmp/g7pro8k\"'"
echo "  adb push \"${DIST_DIR}/.\" /data/local/tmp/g7pro8k/"
