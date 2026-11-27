#!/system/bin/sh
# On-device root helper for GameSir G7 Pro 8K daemon on AYN Odin 3 Pro (POSIX sh).
set -eu

BASE_DIR="${G7PRO8K_DIR:-/data/local/tmp/g7pro8k}"
BIN="${BASE_DIR}/g7pro8k-odin-daemon"
CONFIG="${BASE_DIR}/config.json"
PID_FILE="${BASE_DIR}/daemon.pid"
LOG_FILE="${BASE_DIR}/daemon.log"
WEB_BASE="${G7PRO8K_WEB_BASE:-http://127.0.0.1:8787}"

usage() {
    cat <<EOF
Usage: $(basename "$0") <command> [args]

Commands:
  start [xbox360|ds4]   Start daemon in background (optional profile override)
  stop                  Stop running daemon cleanly (releases virtual devices)
  status                Show daemon PID status and live Web API status if reachable
  pause                 Pause daemon via local API (releases USB & uinput)
  resume                Resume paused daemon via local API
  profile <xbox360|ds4> Restart daemon with specified profile
  probe                 Run USB descriptor probe
  doctor                Run system health & Odin 3 isolation check
  logs                  Print last 80 lines of daemon log
EOF
}

is_running() {
    if [ ! -f "${PID_FILE}" ]; then
        return 1
    fi
    pid="$(cat "${PID_FILE}" 2>/dev/null || true)"
    if [ -z "${pid}" ]; then
        return 1
    fi
    if [ -d "/proc/${pid}" ]; then
        return 0
    fi
    return 1
}

cmd_start() {
    profile_arg="${1:-}"
    if is_running; then
        echo "g7pro8k-odin-daemon is already running (pid $(cat "${PID_FILE}"))."
        exit 0
    fi
    if [ ! -x "${BIN}" ]; then
        echo "ERROR: Daemon binary not executable at ${BIN}" >&2
        exit 1
    fi
    mkdir -p "${BASE_DIR}"
    if [ -n "${profile_arg}" ]; then
        case "${profile_arg}" in
            xbox360|ds4) ;;
            *)
                echo "ERROR: profile must be 'xbox360' or 'ds4'" >&2
                exit 1
                ;;
        esac
        nohup "${BIN}" run --config "${CONFIG}" --profile "${profile_arg}" >>"${LOG_FILE}" 2>&1 &
    else
        nohup "${BIN}" run --config "${CONFIG}" >>"${LOG_FILE}" 2>&1 &
    fi
    sleep 1
    if is_running; then
        echo "Started g7pro8k-odin-daemon (pid $(cat "${PID_FILE}")). Web UI: ${WEB_BASE}"
    else
        echo "WARNING: Daemon exited early. Recent logs:" >&2
        tail -n 30 "${LOG_FILE}" 2>/dev/null || true
        exit 1
    fi
}

cmd_stop() {
    if ! is_running; then
        echo "g7pro8k-odin-daemon is not running."
        rm -f "${PID_FILE}"
        return 0
    fi
    pid="$(cat "${PID_FILE}")"
    kill -TERM "${pid}" 2>/dev/null || true
    i=0
    while [ "${i}" -lt 20 ]; do
        if [ ! -d "/proc/${pid}" ]; then
            rm -f "${PID_FILE}"
            echo "Stopped g7pro8k-odin-daemon cleanly."
            return 0
        fi
        sleep 1
        i=$((i + 1))
    done
    echo "Force-stopping pid ${pid}..."
    kill -KILL "${pid}" 2>/dev/null || true
    rm -f "${PID_FILE}"
}

cmd_status() {
    if is_running; then
        echo "Daemon: RUNNING (pid $(cat "${PID_FILE}"))"
        if command -v curl >/dev/null 2>&1; then
            curl -fsS "${WEB_BASE}/api/status" 2>/dev/null && echo "" || true
        fi
    else
        echo "Daemon: STOPPED"
        return 1
    fi
}

cmd_api_post() {
    endpoint="$1"
    if ! command -v curl >/dev/null 2>&1; then
        echo "ERROR: curl is required for API control; use the Web UI at ${WEB_BASE}" >&2
        exit 1
    fi
    curl -fsS -X POST "${WEB_BASE}${endpoint}" && echo ""
}

case "${1:-}" in
    start)
        cmd_start "${2:-}"
        ;;
    stop)
        cmd_stop
        ;;
    status)
        cmd_status
        ;;
    pause)
        cmd_api_post "/api/daemon/pause"
        ;;
    resume)
        cmd_api_post "/api/daemon/resume"
        ;;
    profile)
        if [ -z "${2:-}" ]; then
            echo "ERROR: specify profile 'xbox360' or 'ds4'" >&2
            exit 1
        fi
        cmd_stop
        cmd_start "$2"
        ;;
    probe)
        "${BIN}" probe --config "${CONFIG}"
        ;;
    doctor)
        "${BIN}" doctor --config "${CONFIG}"
        ;;
    logs)
        tail -n 80 "${LOG_FILE}" 2>/dev/null || echo "No log file at ${LOG_FILE}"
        ;;
    *)
        usage
        exit 1
        ;;
esac
