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
