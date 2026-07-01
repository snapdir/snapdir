#!/bin/bash
# sidecars-health.sh — wait until both minio and sshd are accepting connections.
#
# Polls with bounded retries (max 30 × 1 s = 30 s each) then exits non-zero
# with a clear error message if either sidecar is not up in time.
# Intended to be chained: sidecars-up.sh && sidecars-health.sh

set -euo pipefail

SCRIPTS_DIR="$(cd "$(dirname "$0")" && pwd)"
ENV_FILE="${SCRIPTS_DIR}/sidecars.env"

if [ ! -f "${ENV_FILE}" ]; then
    echo "sidecars-health: ERROR — ${ENV_FILE} not found; run sidecars-up.sh first" >&2
    exit 1
fi
# shellcheck source=/dev/null
. "${ENV_FILE}"

MAX_RETRIES=30
SLEEP_SEC=1

# ── wait_port <name> <port> ───────────────────────────────────────────────────
wait_port() {
    local name="$1"
    local port="$2"
    local n=0
    while [ "${n}" -lt "${MAX_RETRIES}" ]; do
        if nc -z 127.0.0.1 "${port}" 2>/dev/null; then
            echo "sidecars-health: ${name} ready on port ${port} (attempt $((n+1)))"
            return 0
        fi
        n=$((n+1))
        sleep "${SLEEP_SEC}"
    done
    echo "sidecars-health: ERROR — ${name} not ready on port ${port} after ${MAX_RETRIES}s" >&2
    return 1
}

# ── wait_minio_live ───────────────────────────────────────────────────────────
# After the TCP port opens, wait for MinIO's /minio/health/live endpoint.
wait_minio_live() {
    local n=0
    while [ "${n}" -lt "${MAX_RETRIES}" ]; do
        if curl -fsS "http://127.0.0.1:${MINIO_PORT}/minio/health/live" >/dev/null 2>&1; then
            echo "sidecars-health: minio /minio/health/live OK (attempt $((n+1)))"
            return 0
        fi
        n=$((n+1))
        sleep "${SLEEP_SEC}"
    done
    echo "sidecars-health: ERROR — minio /minio/health/live not OK after ${MAX_RETRIES}s" >&2
    return 1
}

# ── checks ────────────────────────────────────────────────────────────────────
wait_port "minio-tcp" "${MINIO_PORT}"
wait_minio_live
wait_port "sshd" "${SSHD_PORT}"

echo "sidecars-health: all sidecars healthy"
