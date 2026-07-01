#!/usr/bin/env bash
# tests/golden/drivers/zig.sh — Zig-binding driver for the parity harness.
#
# Implements the §1 driver protocol (tests/golden/parity_harness.md) by invoking
# a PRE-BUILT Zig driver binary (bindings/zig/src/parity_driver.zig, compiled by
# `zig build driver` to zig-out/bin/snapdir-parity-driver and copied to
# tests/golden/drivers/zig-driver-bin by the manifest-parity-zig verification
# BEFORE the harness runs — building per-call would be far too slow). The driver
# binary links the snapdir Zig binding (@cImport over the C ABI). Mirrors go.sh/
# cpp.sh's exec-the-prebuilt-binary shape; every subcommand calls the Zig binding.
#
# stdout: byte-exact per spec §1; diagnostics to stderr; exit 0 = success.
# The harness sets LC_ALL=C, SNAPDIR_NO_PROGRESS, SNAPDIR_CACHE_DIR,
# SNAPDIR_CATALOG_DB_PATH and scrubs SNAPDIR_STORE/OBJECTS_STORE/MANIFEST_CONTEXT
# (§1.6); the Zig binding wraps snapdir-api (via the C ABI) which honors those.
#
# LANE NOTE: this file + src/parity_driver.zig live under tests/golden/ +
# bindings/zig. They only CONSUME the binding — never reimplement.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="${SCRIPT_DIR}/zig-driver-bin"

if [[ ! -x "${BIN}" ]]; then
  echo "[zig.sh] ERROR: driver binary not built (${BIN} missing) — the gate verification runs 'zig build driver' and copies the artifact to ${BIN} first" >&2
  exit 1
fi

exec "${BIN}" "$@"
