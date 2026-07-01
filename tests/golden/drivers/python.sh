#!/usr/bin/env bash
# tests/golden/drivers/python.sh — Python-binding driver for the parity harness.
#
# Implements the §1 driver protocol (tests/golden/parity_harness.md) by invoking
# the built `snapdir` PyO3 binding through python_driver.py, using the binding's
# own uv-managed virtualenv interpreter (where `maturin develop` installed it).
# stdout byte-exact; diagnostics to stderr; exit 0 = success. Mirrors the oracle
# reference driver (rust.sh) dispatch shape, but calls the Python binding.
#
# The harness sets LC_ALL=C, SNAPDIR_NO_PROGRESS, SNAPDIR_CACHE_DIR,
# SNAPDIR_CATALOG_DB_PATH and scrubs SNAPDIR_STORE/OBJECTS_STORE/MANIFEST_CONTEXT
# before invoking this driver (§1.6); the Python binding wraps snapdir-api which
# honors those vars — we inherit the env verbatim.
#
# LANE NOTE: this file + python_driver.py live under tests/golden/ (adversary
# lane). They only CONSUME the built binding — they never edit bindings/python/src.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
VENV_PY="${WORKSPACE_ROOT}/bindings/python/.venv/bin/python"

if [[ ! -x "${VENV_PY}" ]]; then
  echo "[python.sh] ERROR: venv python not found (${VENV_PY}); run 'uv run maturin develop' in bindings/python first" >&2
  exit 1
fi

exec "${VENV_PY}" "${SCRIPT_DIR}/python_driver.py" "$@"
