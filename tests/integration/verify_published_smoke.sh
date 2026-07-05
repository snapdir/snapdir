#!/usr/bin/env bash
# ===========================================================================
# verify_published_smoke.sh — from-registry, blank-slate install-and-run smoke
# (Phase 45.E). Proves an INSTALLED snapdir binding actually works end-to-end
# by driving the language's example app through the SAME push/pull/id contract
# every example exposes, over a local file:// store — and asserts the snapshot
# id is byte-exact to the canonical Rust-oracle golden id.
#
# Reused by every job in .github/workflows/verify-published.yml. It is store-
# and network-agnostic: no minio, no AWS creds, no snapdir-bindings:dev image.
#
# Usage:  verify_published_smoke.sh <app-invoke-prefix...>
#   where "<app-invoke-prefix> push <dir> <store>" etc. runs the example app.
#   e.g.  verify_published_smoke.sh node /app/app.mjs
#         verify_published_smoke.sh python3 /w/examples/python/app.py
#         verify_published_smoke.sh /app/app
#
# The seed fixture + GOLDEN_ID_A are IDENTICAL to the Phase-44 relay oracle
# (tests/integration/integration_assertions.sh relay_make_fixtures): a fixed
# deterministic tree whose manifest depends only on path/type/perms/checksum/
# size (no mtimes), so the id is reproducible on any host/arch/libc.
# ===========================================================================
set -euo pipefail

GOLDEN_ID_A="${GOLDEN_ID_A:-9e12678a4ba37d1fb6864842b02e8b306c70ba1793479dc71bab12cc21f0703b}"

if [ "$#" -lt 1 ]; then
  echo "usage: verify_published_smoke.sh <app-invoke-prefix...>" >&2
  exit 2
fi

# The example app is invoked as: "$@" <verb> <args...>
app() { "$@"; }
APP=("$@")

W="$(mktemp -d)"
trap 'rm -rf "$W"' EXIT
seed="$W/seed"
store="$W/store"
out="$W/out"

# --- deterministic seed fixture (byte-identical to the relay oracle) ---------
mkdir -p "$seed/data/notes" "$seed/scripts"
printf 'hello relay\n'        > "$seed/greeting.txt"
printf '1\n2\n3\n'            > "$seed/data/numbers.txt"
printf '# snapdir relay\n'    > "$seed/data/notes/readme.md"
printf '#!/bin/sh\necho hi\n' > "$seed/scripts/run.sh"
chmod 0644 "$seed/greeting.txt" "$seed/data/numbers.txt" "$seed/data/notes/readme.md"
chmod 0755 "$seed/scripts/run.sh"
chmod 0755 "$seed" "$seed/data" "$seed/data/notes" "$seed/scripts"

hex64() { printf '%s' "$1" | tr -d '[:space:]'; }
assert_id() {
  local label="$1" got; got="$(hex64 "$2")"
  if ! printf '%s' "$got" | grep -Eq '^[0-9a-f]{64}$'; then
    echo "FAIL[$label]: not a 64-hex id: '${got}'" >&2; exit 1
  fi
  if [ "$got" != "$GOLDEN_ID_A" ]; then
    echo "FAIL[$label]: id mismatch" >&2
    echo "  expected (golden): $GOLDEN_ID_A" >&2
    echo "  got:               $got" >&2
    exit 1
  fi
  echo "ok[$label]: $got"
}

echo "== verify-published smoke: ${APP[*]} =="

# 1) id(seed) must equal the canonical golden id (pure hashing, no store).
id_seed="$("${APP[@]}" id "$seed")"
assert_id "id(seed)" "$id_seed"

# 2) push(seed -> file store) must return the golden id.
pushed="$("${APP[@]}" push "$seed" "file://$store")"
assert_id "push(seed)" "$pushed"

# 3) pull(id) then re-id(dest) — full round-trip determinism through the store.
"${APP[@]}" pull "$pushed" "file://$store" "$out"
id_out="$("${APP[@]}" id "$out")"
assert_id "id(pulled)" "$id_out"

echo "SMOKE_OK ${APP[*]}"
