#!/usr/bin/env bash
# tests/golden/drivers/cpp.sh — C++-binding driver for the parity harness.
#
# Implements the §1 driver protocol (tests/golden/parity_harness.md) by invoking
# a PRE-BUILT C++ driver binary (bindings/cpp/cmd/parity_driver.cpp, compiled to
# tests/golden/drivers/cpp-driver-bin by the manifest-parity-cpp verification
# BEFORE the harness runs — building per-call would be far too slow). The driver
# binary links the snapdir C++ RAII binding (snapdir.hpp over the C ABI). Mirrors
# go.sh's exec-the-prebuilt-binary shape; every subcommand calls the C++ binding.
#
# stdout: byte-exact per spec §1; diagnostics to stderr; exit 0 = success.
# The harness sets LC_ALL=C, SNAPDIR_NO_PROGRESS, SNAPDIR_CACHE_DIR,
# SNAPDIR_CATALOG_DB_PATH and scrubs SNAPDIR_STORE/OBJECTS_STORE/MANIFEST_CONTEXT
# (§1.6); the C++ binding wraps snapdir-api (via the C ABI) which honors those.
#
# LANE NOTE: this file + cmd/parity_driver.cpp live under tests/golden/ +
# bindings/cpp. They only CONSUME the binding — never reimplement.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="${SCRIPT_DIR}/cpp-driver-bin"

if [[ ! -x "${BIN}" ]]; then
  echo "[cpp.sh] ERROR: driver binary not built (${BIN} missing) — the gate verification compiles bindings/cpp/cmd/parity_driver.cpp to ${BIN} first" >&2
  exit 1
fi

exec "${BIN}" "$@"
