#!/usr/bin/env bash
# tests/golden/drivers/java.sh — Java-binding driver for the parity harness.
#
# Implements the §1 driver protocol (tests/golden/parity_harness.md) by invoking
# io.snapdir.ParityDriver (tests/golden/drivers/ParityDriver.java) against the built
# `io.snapdir` JDK-Foreign binding. The driver class + the binding's main classes are
# compiled into bindings/java/build/classes by the manifest-parity-java verification
# BEFORE the harness runs (compiling per-call would be far too slow); this wrapper
# just exec's the JVM. Mirrors python.sh / the prebuilt go/zig/cpp drivers' shape.
#
# stdout: byte-exact per spec §1; diagnostics to stderr; exit 0 = success. The harness
# sets LC_ALL=C, SNAPDIR_NO_PROGRESS, SNAPDIR_CACHE_DIR, SNAPDIR_CATALOG_DB_PATH and
# scrubs SNAPDIR_STORE/OBJECTS_STORE/MANIFEST_CONTEXT (§1.6); the binding wraps
# snapdir-api (via the C ABI) which honors those — we inherit the env verbatim.
#
# LANE NOTE: this file + ParityDriver.java live under tests/golden/. They only CONSUME
# the binding — never reimplement it. Java is native arm64 (no qemu emulation).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
CLASSES="${WORKSPACE_ROOT}/bindings/java/build/classes"

if [[ ! -d "${CLASSES}" ]] || [[ ! -f "${CLASSES}/io/snapdir/ParityDriver.class" ]]; then
  echo "[java.sh] ERROR: classes not built (${CLASSES}/io/snapdir/ParityDriver.class missing) — the gate verification compiles src/main/java + ParityDriver.java into build/classes and vendors the .so first" >&2
  exit 1
fi

exec java \
  --add-modules jdk.incubator.foreign \
  --enable-native-access=ALL-UNNAMED \
  -cp "${CLASSES}" \
  io.snapdir.ParityDriver "$@"
