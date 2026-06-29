#!/usr/bin/env bash
# tests/golden/drivers/rust.sh — ORACLE reference driver for the parity harness.
#
# This is the reference driver (§0 of parity_harness.md): a thin wrapper over
# the workspace `snapdir` binary (frozen 1.10.0 oracle). It implements the
# §1 driver protocol exactly.
#
# Protocol:
#   rust.sh manifest <path> [--no-follow] [--absolute] [--exclude <RE>]...
#   rust.sh id       <path> [--no-follow] [--absolute] [--exclude <RE>]...
#   rust.sh push     <path> <store_uri> [--jobs N]
#   rust.sh fetch    <id>   <store_uri>
#   rust.sh checkout <id>   <store_uri> <dest>
#
# stdout: byte-exact per spec §1; diagnostics to stderr; exit 0 = success.
# The harness sets LC_ALL=C, SNAPDIR_NO_PROGRESS=1, SNAPDIR_CACHE_DIR,
# SNAPDIR_CATALOG_DB_PATH, and scrubs SNAPDIR_STORE/SNAPDIR_OBJECTS_STORE/
# SNAPDIR_MANIFEST_CONTEXT before invoking this driver (§1.6).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"

# ---------------------------------------------------------------------------
# Locate the oracle binary (prefer release > debug; build if absent).
# ---------------------------------------------------------------------------
locate_oracle() {
    local release_bin="${WORKSPACE_ROOT}/target/release/snapdir"
    local debug_bin="${WORKSPACE_ROOT}/target/debug/snapdir"
    if [[ -x "${release_bin}" ]]; then
        echo "${release_bin}"; return
    fi
    if [[ -x "${debug_bin}" ]]; then
        echo "${debug_bin}"; return
    fi
    echo "[rust.sh] Oracle not found — building (cargo build -p snapdir --locked)..." >&2
    (cd "${WORKSPACE_ROOT}" && cargo build -p snapdir --locked) >&2
    if [[ ! -x "${debug_bin}" ]]; then
        echo "[rust.sh] ERROR: build succeeded but ${debug_bin} not found" >&2
        exit 1
    fi
    echo "${debug_bin}"
}

ORACLE="$(locate_oracle)"

# ---------------------------------------------------------------------------
# Dispatch subcommand.
# ---------------------------------------------------------------------------
SUBCMD="${1:-}"
shift

case "${SUBCMD}" in
    manifest)
        # rust.sh manifest <path> [--no-follow] [--absolute] [--exclude <RE>]...
        # Maps 1:1 to: snapdir manifest [flags] <path>
        # The path is the LAST non-flag argument; flags pass through verbatim.
        exec "${ORACLE}" manifest --no-progress "$@"
        ;;

    id)
        # rust.sh id <path> [--no-follow] [--absolute] [--exclude <RE>]...
        # Per §1.2: id == BLAKE3(manifest text for the same flags).
        # The oracle `snapdir id <path>` works WITHOUT extra flags (simple case).
        # But `snapdir id` does NOT support --no-follow / --absolute / --exclude.
        # When those flags are present, compute via: manifest [flags] <path> | snapdir id.
        # This is exactly how the oracle obtains the option-variant id.
        #
        # Parse args: extract the path (last non-flag arg) and manifest flags.
        manifest_flags=()
        path_arg=""
        while [[ $# -gt 0 ]]; do
            case "$1" in
                --no-follow|--absolute)
                    manifest_flags+=("$1"); shift ;;
                --exclude)
                    manifest_flags+=("$1" "$2"); shift 2 ;;
                --exclude=*)
                    manifest_flags+=("$1"); shift ;;
                -*)
                    # Unknown flag: pass to manifest (it will error if unsupported)
                    manifest_flags+=("$1"); shift ;;
                *)
                    path_arg="$1"; shift ;;
            esac
        done

        if [[ -z "${path_arg}" ]]; then
            echo "[rust.sh] ERROR: id subcommand requires a path argument" >&2
            exit 1
        fi

        if [[ ${#manifest_flags[@]} -eq 0 ]]; then
            # No extra flags: use `snapdir id <path>` directly (fast path).
            exec "${ORACLE}" id --no-progress "${path_arg}"
        else
            # Extra flags present: pipe manifest through snapdir id (stdin mode).
            "${ORACLE}" manifest --no-progress "${manifest_flags[@]}" "${path_arg}" \
                | "${ORACLE}" id --no-progress
        fi
        ;;

    push)
        # rust.sh push <path> <store_uri> [--jobs N ...]
        # Emits the 64-hex snapshot id to stdout.
        # Maps to: snapdir push --store <store_uri> <path>
        local_path="$1"
        store_uri="$2"
        shift 2
        # Remaining args may include --jobs N; pass them through as snapdir flags.
        # Build extra flags from remaining args (--jobs N → -j N style).
        extra_flags=()
        while [[ $# -gt 0 ]]; do
            case "$1" in
                --jobs)
                    extra_flags+=(-j "$2"); shift 2 ;;
                *)
                    extra_flags+=("$1"); shift ;;
            esac
        done
        exec "${ORACLE}" push --no-progress --store "${store_uri}" "${extra_flags[@]+"${extra_flags[@]}"}" "${local_path}"
        ;;

    fetch)
        # rust.sh fetch <id> <store_uri>
        # Maps to: snapdir fetch --store <store_uri> --id <id>
        snap_id="$1"
        store_uri="$2"
        exec "${ORACLE}" fetch --no-progress --store "${store_uri}" --id "${snap_id}"
        ;;

    checkout)
        # rust.sh checkout <id> <store_uri> <dest>
        # Maps to: snapdir pull --store <store_uri> --id <id> <dest>
        # (fetch-then-checkout; equivalently the pull subcommand).
        snap_id="$1"
        store_uri="$2"
        dest="$3"
        exec "${ORACLE}" pull --no-progress --store "${store_uri}" --id "${snap_id}" "${dest}"
        ;;

    *)
        echo "[rust.sh] ERROR: unknown subcommand '${SUBCMD}'" >&2
        echo "Usage: rust.sh {manifest|id|push|fetch|checkout} <args...>" >&2
        exit 1
        ;;
esac
