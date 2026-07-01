#!/usr/bin/env bash
# tests/golden/drivers/node.sh — Node-binding driver for the parity harness.
#
# Implements the §1 driver protocol (tests/golden/parity_harness.md) by
# invoking the BUILT @snapdir/snapdir binding (bindings/node/) through a small
# JS helper (node_driver.mjs). It mirrors the oracle reference driver
# (tests/golden/drivers/rust.sh) subcommand dispatch shape exactly, EXCEPT every
# subcommand calls the Node binding's own code (napi) — never the oracle.
#
# Protocol:
#   node.sh manifest <path> [--no-follow] [--absolute] [--exclude <RE>]...
#   node.sh id       <path> [--no-follow] [--absolute] [--exclude <RE>]...
#   node.sh push     <path> <store_uri> [--jobs N]
#   node.sh fetch    <id>   <store_uri>
#   node.sh checkout <id>   <store_uri> <dest>
#
# stdout: byte-exact per spec §1; diagnostics to stderr; exit 0 = success.
# The harness sets LC_ALL=C, SNAPDIR_NO_PROGRESS, SNAPDIR_CACHE_DIR,
# SNAPDIR_CATALOG_DB_PATH and scrubs SNAPDIR_STORE/SNAPDIR_OBJECTS_STORE/
# SNAPDIR_MANIFEST_CONTEXT before invoking this driver (§1.6). The Node binding
# wraps snapdir-api, which honors those env vars — we inherit the env verbatim
# and never override it.
#
# LANE NOTE: this file + node_driver.mjs live under tests/golden/ (adversary
# lane). They only CONSUME the built binding — they never edit bindings/node/src.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Pure pass-through: the JS helper implements the full §1 subprocess protocol
# (path/flag parsing, byte-exact stdout, the unsupported-flag diagnostic). We
# forward argv unchanged and inherit the harness-set/scrubbed environment.
exec node "${SCRIPT_DIR}/node_driver.mjs" "$@"
