#!/bin/bash
# sidecars-down.sh — stop minio and sshd parity sidecars, clean up data dirs.
#
# Reads PID files written by sidecars-up.sh; if a PID file is absent
# falls back to pkill by name (best effort). Always exits 0.

set -euo pipefail

SCRIPTS_DIR="$(cd "$(dirname "$0")" && pwd)"
ENV_FILE="${SCRIPTS_DIR}/sidecars.env"

if [ ! -f "${ENV_FILE}" ]; then
    echo "sidecars-down: ${ENV_FILE} not found — nothing to stop"
    exit 0
fi
# shellcheck source=/dev/null
. "${ENV_FILE}"

# ── stop_pid <name> <pid-file> <fallback-pkill-pattern> ──────────────────────
stop_pid() {
    local name="$1"
    local pid_file="$2"
    local pattern="$3"

    if [ -f "${pid_file}" ]; then
        local pid
        pid="$(cat "${pid_file}")"
        if kill -0 "${pid}" 2>/dev/null; then
            kill "${pid}" 2>/dev/null && echo "sidecars-down: ${name} (pid ${pid}) stopped"
        else
            echo "sidecars-down: ${name} pid ${pid} already gone"
        fi
        rm -f "${pid_file}"
    else
        # Fallback: pkill (best effort — don't fail if not found)
        if pkill -f "${pattern}" 2>/dev/null; then
            echo "sidecars-down: ${name} stopped via pkill"
        else
            echo "sidecars-down: ${name} not running (nothing to stop)"
        fi
    fi
}

# ── stop MinIO ────────────────────────────────────────────────────────────────
stop_pid "minio" "${MINIO_PID_FILE}" "minio server"

# ── stop sshd ────────────────────────────────────────────────────────────────
stop_pid "sshd" "${SSHD_PID_FILE}" "sshd.*sidecar-sshd"

# ── clean up data ─────────────────────────────────────────────────────────────
rm -rf "${MINIO_DATA_DIR}"
echo "sidecars-down: cleaned ${MINIO_DATA_DIR}"

echo "sidecars-down: done"
