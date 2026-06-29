#!/usr/bin/env bash
# tests/golden/drivers/go.sh — Go-binding driver for the parity harness.
#
# Implements the §1 driver protocol (tests/golden/parity_harness.md) by invoking
# a PRE-BUILT Go driver binary (bindings/go/cmd/parity-driver, compiled to
# tests/golden/drivers/go-driver-bin by the manifest-parity-go verification
# BEFORE the harness runs — building per-call would be far too slow). The driver
# binary links the snapdir Go binding (CGo over the C ABI). Mirrors rust.sh's
# subcommand dispatch shape but every subcommand calls the Go binding.
#
# stdout: byte-exact per spec §1; diagnostics to stderr; exit 0 = success.
# The harness sets LC_ALL=C, SNAPDIR_NO_PROGRESS, SNAPDIR_CACHE_DIR,
# SNAPDIR_CATALOG_DB_PATH and scrubs SNAPDIR_STORE/OBJECTS_STORE/MANIFEST_CONTEXT
# (§1.6); the Go binding wraps snapdir-api which honors those — env inherited.
#
# LANE NOTE: this file + cmd/parity-driver live under tests/golden/ + bindings/go
# (adversary/go lanes). They only CONSUME the binding — never reimplement.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="${SCRIPT_DIR}/go-driver-bin"

if [[ ! -x "${BIN}" ]]; then
  echo "[go.sh] ERROR: driver binary not built (${BIN} missing) — the gate verification builds it via 'go build -o ${BIN} ./cmd/parity-driver' first" >&2
  exit 1
fi

exec "${BIN}" "$@"
