#!/usr/bin/env bash
# size_parity.sh — cross-language SIZE parity harness (Phase 46).
#
# Non-negotiable correctness contract for `snapdir size`: the CLI oracle,
# snapdir-api, and ALL SIX bindings (node/python/go/cpp/java/zig) MUST report
# byte-identical logical_bytes / physical_bytes / files / objects for the same
# store, AND physical_bytes MUST equal the real on-disk byte total of the
# file:// store's `.objects/` directory (objects are stored uncompressed).
#
# ── The `size` driver subcommand contract (the -impl gate adds it to every
#    driver in tests/golden/drivers/<lang>.sh) ─────────────────────────────────
#   <driver> size <store_uri> [<snapshot_id>]
#     → emits EXACTLY four lines to stdout, in this order (decimal u64, no
#       labels, no separators):
#           <logical_bytes>
#           <physical_bytes>
#           <files>
#           <objects>
#     <snapshot_id> present ⇒ that snapshot; omitted ⇒ whole store (deduped).
#   Diagnostics to stderr only; exit 0 = success. Each binding's driver computes
#   via its OWN binding code (napi/PyO3/C-ABI/JDK-foreign/@cImport) — never the
#   oracle. The harness cannot tell them apart; it checks byte-parity + on-disk.
#
# EXPECTED-FAIL rationale (authoring gate): the drivers do not implement `size`
# yet, so this harness FAILS today. That is correct. The -impl gate wires the
# `size` subcommand into every driver and moves this file to tests/golden/.
#
# Mirrors tests/golden/run_parity.sh conventions: LC_ALL=C, per-run temp
# SNAPDIR_CACHE_DIR/SNAPDIR_CATALOG_DB_PATH, store-env scrubbed before each
# driver call, target/{release,debug} on PATH so the `snapdir` oracle resolves.

set -euo pipefail

LC_ALL=C
export LC_ALL

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Locate the golden dir + workspace root whether run from the drop-zone
# (.gatesmith/pending-tests/) or the landed location (tests/golden/).
if [[ -d "${SCRIPT_DIR}/drivers" ]]; then
    GOLDEN_DIR="${SCRIPT_DIR}"
    WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
else
    WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
    GOLDEN_DIR="${WORKSPACE_ROOT}/tests/golden"
fi
DRIVERS_DIR="${GOLDEN_DIR}/drivers"

# Put the oracle binary on PATH (release preferred) so `snapdir size --json`
# and the reference driver resolve.
if [[ -x "${WORKSPACE_ROOT}/target/release/snapdir" ]]; then
    export PATH="${WORKSPACE_ROOT}/target/release:${PATH}"
elif [[ -x "${WORKSPACE_ROOT}/target/debug/snapdir" ]]; then
    export PATH="${WORKSPACE_ROOT}/target/debug:${PATH}"
else
    echo "[size_parity] building oracle (cargo build -p snapdir --locked)..." >&2
    (cd "${WORKSPACE_ROOT}" && cargo build -p snapdir --locked) >&2
    export PATH="${WORKSPACE_ROOT}/target/debug:${PATH}"
fi

# ── per-run hermetic cache/catalog + cleanup ──────────────────────────────────
RUN_ROOT="$(mktemp -d)"
export SNAPDIR_CACHE_DIR="${RUN_ROOT}/cache"
export SNAPDIR_CATALOG_DB_PATH="${RUN_ROOT}/catalog.db"
export SNAPDIR_NO_PROGRESS=true
mkdir -p "${SNAPDIR_CACHE_DIR}"
cleanup() { rm -rf "${RUN_ROOT}" 2>/dev/null || true; }
trap cleanup EXIT

# The reference driver = the oracle (tests/golden/drivers/rust.sh).
REF_DRIVER="${DRIVERS_DIR}/rust.sh"

# The bindings under parity test. A driver is included only if its .sh exists.
ALL_DRIVERS=(rust node python go cpp java zig)

fail() { echo "[size_parity] FAIL: $*" >&2; exit 1; }

# Invoke a driver with the store-routing env scrubbed (flags/args are the sole
# source of truth), mirroring run_parity.sh §1.6.
run_driver() {
    local drv="$1"; shift
    env -u SNAPDIR_STORE -u SNAPDIR_OBJECTS_STORE -u SNAPDIR_MANIFEST_CONTEXT \
        "${drv}" "$@"
}

# Sum the byte size of every regular file under <store>/.objects/ (uncompressed
# objects → this equals physical_bytes). 0 if the dir is absent.
objects_on_disk_bytes() {
    local store_dir="$1"
    local objdir="${store_dir}/.objects"
    [[ -d "${objdir}" ]] || { echo 0; return; }
    local total=0 sz
    while IFS= read -r sz; do
        total=$(( total + sz ))
    done < <(find "${objdir}" -type f -printf '%s\n')
    echo "${total}"
}

# Extract one integer field from `snapdir size --json` output.
json_field() {
    local json="$1" key="$2"
    # keys: logical_bytes physical_bytes files objects (u64) — tolerate spaces.
    printf '%s' "${json}" | grep -oE "\"${key}\"[[:space:]]*:[[:space:]]*[0-9]+" \
        | grep -oE '[0-9]+$' | head -1
}

# ── build fixtures ────────────────────────────────────────────────────────────
# Fixture 1 (dedup): a duplicated file + a unique file → physical < logical.
#   a/x.txt = "hello world\n" (12), b/dup.txt = "hello world\n" (12 dup),
#   a/y.txt = "unique data here\n" (17)  ⇒ files=3 objects=2 logical=41 physical=29
# Fixture 2 (no-dup): two distinct files → physical == logical (dedup must NOT
#   over-collapse; catches a broken dedup that always merges).
FIX_ROOT="${RUN_ROOT}/fixtures"
mkdir -p "${FIX_ROOT}/dup/a" "${FIX_ROOT}/dup/b" "${FIX_ROOT}/nodup"
printf 'hello world\n'       > "${FIX_ROOT}/dup/a/x.txt"
printf 'hello world\n'       > "${FIX_ROOT}/dup/b/dup.txt"
printf 'unique data here\n'  > "${FIX_ROOT}/dup/a/y.txt"
printf 'alpha\n'             > "${FIX_ROOT}/nodup/one.txt"
printf 'a different thing\n' > "${FIX_ROOT}/nodup/two.txt"

FIXTURES=(dup nodup)

run_fixture() {
    local fx="$1"
    local seed="${FIX_ROOT}/${fx}"
    local store_dir; store_dir="$(mktemp -d "${RUN_ROOT}/store-${fx}.XXXXXX")"
    local store_uri="file://${store_dir}"

    # Push the fixture into a fresh store with the reference (oracle) driver.
    local sid
    sid="$(run_driver "${REF_DRIVER}" push "${seed}" "${store_uri}")" \
        || fail "[${fx}] reference push failed"
    [[ -n "${sid}" ]] || fail "[${fx}] reference push produced no snapshot id"

    # Reference size figures (by-id) from the oracle driver.
    local ref_out
    ref_out="$(run_driver "${REF_DRIVER}" size "${store_uri}" "${sid}")" \
        || fail "[${fx}] reference 'size' failed"
    local n; n="$(printf '%s\n' "${ref_out}" | grep -c .)"
    [[ "${n}" -eq 4 ]] || fail "[${fx}] reference size must emit exactly 4 lines, got ${n}: ${ref_out}"

    local ref_logical ref_physical ref_files ref_objects
    { read -r ref_logical; read -r ref_physical; read -r ref_files; read -r ref_objects; } \
        <<< "${ref_out}"

    # ── PARITY: every binding driver must emit byte-identical 4 lines ──────────
    local drv drv_path drv_out
    for drv in "${ALL_DRIVERS[@]}"; do
        drv_path="${DRIVERS_DIR}/${drv}.sh"
        [[ -x "${drv_path}" ]] || { echo "[size_parity] skip ${drv} (no driver)" >&2; continue; }
        drv_out="$(run_driver "${drv_path}" size "${store_uri}" "${sid}")" \
            || fail "[${fx}] driver ${drv} 'size' failed (exit non-zero)"
        if [[ "${drv_out}" != "${ref_out}" ]]; then
            echo "[size_parity] MISMATCH fixture=${fx} driver=${drv}" >&2
            echo "  reference (rust):" >&2; printf '    %s\n' ${ref_out} >&2
            echo "  ${drv}:" >&2;           printf '    %s\n' ${drv_out} >&2
            fail "[${fx}] ${drv} size figures differ from the oracle"
        fi

        # Whole-store leg (no id): single snapshot ⇒ must equal the by-id figure.
        local whole_out
        whole_out="$(run_driver "${drv_path}" size "${store_uri}")" \
            || fail "[${fx}] driver ${drv} whole-store 'size' failed"
        [[ "${whole_out}" == "${ref_out}" ]] \
            || fail "[${fx}] ${drv} whole-store size != by-id size (single snapshot)"
    done

    # ── CLI cross-check: snapdir size --store --id --json ─────────────────────
    local cli_json
    cli_json="$(env -u SNAPDIR_STORE -u SNAPDIR_OBJECTS_STORE -u SNAPDIR_MANIFEST_CONTEXT \
        snapdir size --store "${store_uri}" --id "${sid}" --json)" \
        || fail "[${fx}] CLI 'snapdir size --json' failed"
    local cli_logical cli_physical cli_files cli_objects
    cli_logical="$(json_field "${cli_json}" logical_bytes)"
    cli_physical="$(json_field "${cli_json}" physical_bytes)"
    cli_files="$(json_field "${cli_json}" files)"
    cli_objects="$(json_field "${cli_json}" objects)"
    [[ "${cli_logical}"  == "${ref_logical}"  ]] || fail "[${fx}] CLI logical_bytes ${cli_logical} != ${ref_logical}"
    [[ "${cli_physical}" == "${ref_physical}" ]] || fail "[${fx}] CLI physical_bytes ${cli_physical} != ${ref_physical}"
    [[ "${cli_files}"    == "${ref_files}"    ]] || fail "[${fx}] CLI files ${cli_files} != ${ref_files}"
    [[ "${cli_objects}"  == "${ref_objects}"  ]] || fail "[${fx}] CLI objects ${cli_objects} != ${ref_objects}"

    # ── ACCEPTANCE: physical_bytes == on-disk .objects/ bytes (uncompressed) ──
    local disk; disk="$(objects_on_disk_bytes "${store_dir}")"
    [[ "${ref_physical}" == "${disk}" ]] \
        || fail "[${fx}] physical_bytes ${ref_physical} != on-disk .objects/ ${disk}"

    # ── dedup sanity per fixture ───────────────────────────────────────────────
    if [[ "${fx}" == "dup" ]]; then
        [[ "${ref_logical}" -gt "${ref_physical}" ]] || fail "[dup] expected logical>physical (dedup), got ${ref_logical}/${ref_physical}"
        [[ "${ref_objects}" -lt "${ref_files}"   ]] || fail "[dup] expected objects<files, got ${ref_objects}/${ref_files}"
        [[ "${ref_logical}"  == "41" ]] || fail "[dup] logical_bytes ${ref_logical} != 41"
        [[ "${ref_physical}" == "29" ]] || fail "[dup] physical_bytes ${ref_physical} != 29"
        [[ "${ref_files}"    == "3"  ]] || fail "[dup] files ${ref_files} != 3"
        [[ "${ref_objects}"  == "2"  ]] || fail "[dup] objects ${ref_objects} != 2"
    else
        # no-dup: no shared content ⇒ physical == logical, objects == files.
        [[ "${ref_logical}" == "${ref_physical}" ]] || fail "[nodup] expected logical==physical, got ${ref_logical}/${ref_physical}"
        [[ "${ref_objects}" == "${ref_files}"    ]] || fail "[nodup] expected objects==files, got ${ref_objects}/${ref_files}"
    fi

    echo "[size_parity] OK  fixture=${fx}  logical=${ref_logical} physical=${ref_physical} files=${ref_files} objects=${ref_objects}  (all drivers + CLI + on-disk)"
}

for fx in "${FIXTURES[@]}"; do
    run_fixture "${fx}"
done

echo "[size_parity] ALL PARITY CHECKS PASSED (${#FIXTURES[@]} fixtures × CLI + $(ls "${DRIVERS_DIR}"/*.sh 2>/dev/null | grep -vc _adversary_wrong) drivers)"
