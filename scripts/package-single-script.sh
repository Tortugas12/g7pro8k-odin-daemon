#!/usr/bin/env bash
set -euo pipefail

# Packages the statically compiled aarch64-unknown-linux-musl binary, default config.json,
# and g7pro8k-ctl.sh into a single self-extracting Android root script for AYN Odin 3 Pro's
# built-in "Run script as root" picker (zero ADB or terminal needed).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BIN_PATH="${1:-${PROJECT_ROOT}/target/aarch64-unknown-linux-musl/release/g7pro8k-odin-daemon}"
OUT_DIR="${2:-${PROJECT_ROOT}/dist/odin3-arm64}"
ONECLICK_OUT="${OUT_DIR}/g7pro8k-odin3-oneclick.sh"
STOP_OUT="${OUT_DIR}/g7pro8k-odin3-stop.sh"

if [ ! -f "${BIN_PATH}" ]; then
  echo "ERROR: Static arm64 binary not found at ${BIN_PATH}" >&2
  exit 1
fi

mkdir -p "${OUT_DIR}"

BIN_SHA256="$(sha256sum "${BIN_PATH}" | awk '{print $1}')"
BIN_B64_TMP="$(mktemp)"
trap 'rm -f "${BIN_B64_TMP}"' EXIT
gzip -9 -c "${BIN_PATH}" | base64 -w 76 > "${BIN_B64_TMP}"

cat > "${ONECLICK_OUT}" <<EOF
#!/system/bin/sh
# ==============================================================================
# GameSir G7 Pro 8K — AYN Odin 3 Pro All-in-One Root Installer & Launcher
# ==============================================================================
# Select and run this single script as root directly from AYN Odin 3 Settings.
# No ADB, PC, or terminal shell required!
#
# What this script does safely:
#   1. Stops any previous g7pro8k-odin-daemon instance cleanly (releases uinput).
#   2. Extracts the static arm64 daemon + embedded Companion Web App into
#      /data/local/tmp/g7pro8k/ (verifies SHA-256).
#   3. Preserves your existing config.json if you already customized it.
#   4. Launches g7pro8k-odin-daemon cleanly detached in the background.
#   5. Opens the offline Companion Web App at http://127.0.0.1:8787.
#
# Safety guarantees:
#   - NEVER calls EVIOCGRAB on the Odin 3 Pro's built-in controller.
#   - ONLY claims allowlisted GameSir (0x3537) G7 Pro 8K USB devices.
#   - Web UI binds strictly to 127.0.0.1:8787 (localhost only).
# ==============================================================================
set -eu

BASE_DIR="\${G7PRO8K_DIR:-/data/local/tmp/g7pro8k}"
BIN="\${BASE_DIR}/g7pro8k-odin-daemon"
CONFIG="\${BASE_DIR}/config.json"
CTL="\${BASE_DIR}/g7pro8k-ctl.sh"
PID_FILE="\${BASE_DIR}/daemon.pid"
LOG_FILE="\${BASE_DIR}/daemon.log"
WEB_URL="http://127.0.0.1:8787"
EXPECTED_SHA256="${BIN_SHA256}"

echo "==> GameSir G7 Pro 8K Odin 3 Pro Installer & Launcher"

mkdir -p "\${BASE_DIR}"

# 1. Cleanly stop any previously running daemon so virtual devices release and binary updates cleanly.
if [ -f "\${PID_FILE}" ]; then
    OLD_PID="\$(cat "\${PID_FILE}" 2>/dev/null || true)"
    if [ -n "\${OLD_PID}" ] && [ -d "/proc/\${OLD_PID}" ]; then
        echo "Stopping previous daemon (pid \${OLD_PID})..."
        kill -TERM "\${OLD_PID}" 2>/dev/null || true
        i=0
        while [ "\${i}" -lt 15 ]; do
            if [ ! -d "/proc/\${OLD_PID}" ]; then
                break
            fi
            sleep 1
            i=\$((i + 1))
        done
        if [ -d "/proc/\${OLD_PID}" ]; then
            kill -KILL "\${OLD_PID}" 2>/dev/null || true
        fi
    fi
    rm -f "\${PID_FILE}"
fi

# Helper wrappers that fall back to toybox if needed on minimal Android shells.
decode_b64() {
    if command -v base64 >/dev/null 2>&1; then
        base64 -d
    else
        toybox base64 -d
    fi
}

decompress_gz() {
    if command -v gzip >/dev/null 2>&1; then
        gzip -d
    elif command -v gunzip >/dev/null 2>&1; then
        gunzip
    else
        toybox gzip -d
    fi
}

# 2. Extract static arm64 binary only if missing or checksum differs.
NEED_EXTRACT=1
if [ -x "\${BIN}" ] && command -v sha256sum >/dev/null 2>&1; then
    CUR_SHA="\$(sha256sum "\${BIN}" | awk '{print \$1}')"
    if [ "\${CUR_SHA}" = "\${EXPECTED_SHA256}" ]; then
        NEED_EXTRACT=0
        echo "Binary already up to date (sha256 verified)."
    fi
fi

if [ "\${NEED_EXTRACT}" -eq 1 ]; then
    echo "Extracting static arm64 g7pro8k-odin-daemon to \${BIN}..."
    decode_b64 <<'__G7PRO8K_BIN_B64__' | decompress_gz > "\${BIN}.tmp"
EOF

cat "${BIN_B64_TMP}" >> "${ONECLICK_OUT}"

cat >> "${ONECLICK_OUT}" <<'EOF'
__G7PRO8K_BIN_B64__
    chmod 755 "${BIN}.tmp"
    if command -v sha256sum >/dev/null 2>&1; then
        GOT_SHA="$(sha256sum "${BIN}.tmp" | awk '{print $1}')"
        if [ "${GOT_SHA}" != "${EXPECTED_SHA256}" ]; then
            rm -f "${BIN}.tmp"
            echo "ERROR: Binary checksum mismatch (expected ${EXPECTED_SHA256}, got ${GOT_SHA})" >&2
            exit 1
        fi
    fi
    mv -f "${BIN}.tmp" "${BIN}"
fi

# 3. Install default config.json only if user does not already have one.
if [ ! -f "${CONFIG}" ]; then
    echo "Writing default Xbox 360 config to ${CONFIG}..."
    cat > "${CONFIG}" <<'__G7PRO8K_CONFIG_JSON__'
EOF

cat "${PROJECT_ROOT}/config.example.json" >> "${ONECLICK_OUT}"

cat >> "${ONECLICK_OUT}" <<'EOF'
__G7PRO8K_CONFIG_JSON__
    chmod 644 "${CONFIG}"
else
    echo "Preserving existing ${CONFIG}."
fi

# 4. Install on-device control helper.
cat > "${CTL}" <<'__G7PRO8K_CTL_SH__'
EOF

cat "${PROJECT_ROOT}/scripts/g7pro8k-ctl.sh" >> "${ONECLICK_OUT}"

cat >> "${ONECLICK_OUT}" <<'EOF'
__G7PRO8K_CTL_SH__
chmod 755 "${CTL}"

# Skip auto-start when running offline extraction verification (`G7PRO8K_EXTRACT_ONLY=1`).
if [ "${G7PRO8K_EXTRACT_ONLY:-0}" = "1" ]; then
    echo "Extract-only mode complete: ${BIN}"
    exit 0
fi

# 5. Start daemon cleanly detached from Odin 3 script runner stdio pipes.
echo "Starting g7pro8k-odin-daemon in background..."
if command -v setsid >/dev/null 2>&1; then
    setsid "${BIN}" run --config "${CONFIG}" </dev/null >>"${LOG_FILE}" 2>&1 &
else
    nohup "${BIN}" run --config "${CONFIG}" </dev/null >>"${LOG_FILE}" 2>&1 &
fi

sleep 1

if [ -f "${PID_FILE}" ] && [ -d "/proc/$(cat "${PID_FILE}" 2>/dev/null || echo 0)" ]; then
    PID_VAL="$(cat "${PID_FILE}")"
    echo "SUCCESS: g7pro8k-odin-daemon is running (pid ${PID_VAL})."
    echo "Companion Web App: ${WEB_URL}"
    if command -v am >/dev/null 2>&1; then
        am start --user 0 -a android.intent.action.VIEW -d "${WEB_URL}" >/dev/null 2>&1 || true
    fi
else
    echo "WARNING: Daemon did not report an active PID yet. Recent log lines:" >&2
    tail -n 30 "${LOG_FILE}" 2>/dev/null || true
    exit 1
fi
EOF

cat > "${STOP_OUT}" <<'EOF'
#!/system/bin/sh
# Stops g7pro8k-odin-daemon cleanly from AYN Odin 3 Pro's "Run script as root" picker.
set -eu

BASE_DIR="${G7PRO8K_DIR:-/data/local/tmp/g7pro8k}"
PID_FILE="${BASE_DIR}/daemon.pid"

if [ ! -f "${PID_FILE}" ]; then
    echo "g7pro8k-odin-daemon is not running."
    exit 0
fi

PID="$(cat "${PID_FILE}" 2>/dev/null || true)"
if [ -n "${PID}" ] && [ -d "/proc/${PID}" ]; then
    echo "Stopping g7pro8k-odin-daemon (pid ${PID})..."
    kill -TERM "${PID}" 2>/dev/null || true
    i=0
    while [ "${i}" -lt 15 ]; do
        if [ ! -d "/proc/${PID}" ]; then
            rm -f "${PID_FILE}"
            echo "Stopped cleanly (virtual devices released)."
            exit 0
        fi
        sleep 1
        i=$((i + 1))
    done
    kill -KILL "${PID}" 2>/dev/null || true
fi

rm -f "${PID_FILE}"
echo "Stopped g7pro8k-odin-daemon."
EOF

chmod +x "${ONECLICK_OUT}" "${STOP_OUT}"
echo "==> Packaged single-file Odin 3 Pro root scripts:"
ls -lh "${ONECLICK_OUT}" "${STOP_OUT}"
