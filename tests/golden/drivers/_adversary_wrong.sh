#!/usr/bin/env bash
# tests/golden/drivers/_adversary_wrong.sh — DELIBERATELY-WRONG driver.
#
# ADVERSARY NEGATIVE TEST (parity-harness-tests-review, Phase 36).
#
# This driver is a thin wrapper over the oracle reference driver (rust.sh) that
# applies a SUBTLE, single-mutation corruption to its output, controlled by the
# env var BAD_MODE. The whole point of the parity harness is to be the
# correctness baseline EVERY binding is measured against — so it MUST reject a
# wrong driver. If `run_parity.sh --driver <this>` ever PASSES, the parity
# baseline is worthless (false confidence) and parity-harness-impl must reopen.
#
# A correct harness MUST exit non-zero (a FAIL line) for EVERY BAD_MODE below.
#
# BAD_MODE values (default: flip-id-hex):
#   flip-id-hex     — flip one hex digit of the reported id (id mismatch +
#                     id-self-consistency mismatch). Manifest is left correct.
#   drop-file       — delete the LAST entry line from the manifest (and the id
#                     becomes BLAKE3 of the truncated manifest, so id ALSO
#                     diverges from expected). Models a binding that drops a file.
#   perm-byte       — change one octal perm byte in the manifest root D-line
#                     (755 -> 750). Subtle one-byte manifest divergence.
#   no-trailing-nl  — emit the manifest WITHOUT its trailing newline. Tests the
#                     §2.1 "missing/extra trailing newline is a FAIL" clause —
#                     the assertion most prone to being normalized away by
#                     command substitution.
#   extra-trailing-nl — emit the manifest with an EXTRA trailing newline.
#   id-only-correct — emit a CORRECT (expected) id but a CORRUPTED manifest
#                     (perm byte). This is the anti-cheat target: a driver that
#                     hardcodes the right hex while emitting a divergent
#                     manifest MUST be caught by the id-self-consistency check.
#
# All other subcommands (push/fetch/checkout) pass straight through to the
# oracle so the round-trip legs still exercise real transport; the corruption
# is confined to manifest/id so the negative result is unambiguous.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ORACLE_DRIVER="${SCRIPT_DIR}/rust.sh"
BAD_MODE="${BAD_MODE:-flip-id-hex}"

SUBCMD="${1:-}"

case "${SUBCMD}" in
    manifest)
        case "${BAD_MODE}" in
            flip-id-hex)
                # Manifest is left untouched (only the id is corrupted).
                exec "${ORACLE_DRIVER}" "$@"
                ;;
            drop-file)
                # Drop the LAST manifest line (a dropped file/entry).
                "${ORACLE_DRIVER}" "$@" | sed '$d'
                ;;
            perm-byte|id-only-correct)
                # Corrupt one perm byte on the root D-line: "D 755 " -> "D 750 ".
                "${ORACLE_DRIVER}" "$@" | sed '1s/^D 755 /D 750 /'
                ;;
            no-trailing-nl)
                # Strip the trailing newline (printf %s of the captured bytes
                # would re-add one; here we explicitly emit NO trailing newline).
                out="$("${ORACLE_DRIVER}" "$@")"
                printf '%s' "${out}"
                ;;
            extra-trailing-nl)
                "${ORACLE_DRIVER}" "$@"
                printf '\n'
                ;;
            *)
                echo "[_adversary_wrong] unknown BAD_MODE '${BAD_MODE}'" >&2
                exit 2
                ;;
        esac
        ;;

    id)
        case "${BAD_MODE}" in
            flip-id-hex)
                # Flip one hex digit of the id: change the first char.
                # 0<->1, a<->b, etc. (any single-digit change makes it wrong).
                real="$("${ORACLE_DRIVER}" "$@")"
                real="${real%$'\n'}"
                first="${real:0:1}"
                case "${first}" in
                    0) flip=1 ;; 1) flip=0 ;;
                    a) flip=b ;; b) flip=a ;;
                    *) flip=0 ;;  # if first isn't 0/1/a/b, force a 0 (still a flip
                                  # unless it already is 0, handled above)
                esac
                # Guarantee a change even in the fallback branch.
                [[ "${flip}" == "${first}" ]] && flip=f
                printf '%s\n' "${flip}${real:1}"
                ;;
            drop-file)
                # Re-derive the id from the CORRUPTED (file-dropped) manifest, so
                # the id is internally self-consistent but != expected.
                # Mirror rust.sh's flag-parsing to get the path + manifest flags.
                shift  # drop "id"
                manifest_flags=()
                path_arg=""
                while [[ $# -gt 0 ]]; do
                    case "$1" in
                        --no-follow|--absolute) manifest_flags+=("$1"); shift ;;
                        --exclude) manifest_flags+=("$1" "$2"); shift 2 ;;
                        --exclude=*) manifest_flags+=("$1"); shift ;;
                        -*) manifest_flags+=("$1"); shift ;;
                        *) path_arg="$1"; shift ;;
                    esac
                done
                BAD_MODE=drop-file "$0" manifest "${manifest_flags[@]+"${manifest_flags[@]}"}" "${path_arg}" \
                    | "${ORACLE_DRIVER}" id 2>/dev/null || true
                ;;
            perm-byte)
                # Re-derive id from the perm-corrupted manifest (self-consistent
                # but != expected). Same path-parse as drop-file.
                shift
                manifest_flags=()
                path_arg=""
                while [[ $# -gt 0 ]]; do
                    case "$1" in
                        --no-follow|--absolute) manifest_flags+=("$1"); shift ;;
                        --exclude) manifest_flags+=("$1" "$2"); shift 2 ;;
                        --exclude=*) manifest_flags+=("$1"); shift ;;
                        -*) manifest_flags+=("$1"); shift ;;
                        *) path_arg="$1"; shift ;;
                    esac
                done
                BAD_MODE=perm-byte "$0" manifest "${manifest_flags[@]+"${manifest_flags[@]}"}" "${path_arg}" \
                    | "${ORACLE_DRIVER}" id 2>/dev/null || true
                ;;
            id-only-correct|no-trailing-nl|extra-trailing-nl)
                # Emit the CORRECT id while the manifest is corrupted (id-only-correct)
                # OR while only the trailing newline differs (the nl modes leave the id
                # text byte-identical, so the correct id is the honest output). This
                # targets the anti-cheat: harness must catch id-only-correct via the
                # id-self-consistency (manifest-derived) check.
                exec "${ORACLE_DRIVER}" "$@"
                ;;
            *)
                echo "[_adversary_wrong] unknown BAD_MODE '${BAD_MODE}'" >&2
                exit 2
                ;;
        esac
        ;;

    push|fetch|checkout)
        # Pass transport subcommands straight through to the oracle (real round-trip).
        exec "${ORACLE_DRIVER}" "$@"
        ;;

    *)
        echo "[_adversary_wrong] ERROR: unknown subcommand '${SUBCMD}'" >&2
        exit 1
        ;;
esac
