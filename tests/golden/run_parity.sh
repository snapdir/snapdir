#!/usr/bin/env bash
# tests/golden/run_parity.sh — Language-agnostic manifest-parity harness.
#
# CONTRACT: .gatesmith/pending-tests/parity_harness.md (now at
#           tests/golden/parity_harness.md after the -impl git mv).
#
# Usage:
#   run_parity.sh [--driver <path-or-cmd>]
#
# The driver defaults to tests/golden/drivers/rust.sh (the oracle reference).
# Env overrides:
#   PARITY_DRIVER             — path or command for the driver executable
#   SNAPDIR_PARITY_NIGHTLY=1  — enable nightly legs (b2, ssh, large-tree×net, live gs)
#
# Exit: 0 iff FAILED==0 (skips allowed); non-zero on ANY mismatch OR missing
#       baseline/driver (per §4).
#
# Output: one PASS|FAIL|SKIP line per (fixture, backend, assertion) cell + a
#         final SUMMARY line.
#
# Design: bash-only, no external tools beyond snapdir binary + coreutils.

set -uo pipefail
LC_ALL=C
export LC_ALL

# ---------------------------------------------------------------------------
# Parse flags
# ---------------------------------------------------------------------------
SELFTEST=0
SELFTEST_FAST=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --driver)
            PARITY_DRIVER="$2"; shift 2 ;;
        --driver=*)
            PARITY_DRIVER="${1#--driver=}"; shift ;;
        --selftest)
            # Negative-guard self-check: assert the oracle driver PASSES and the
            # deliberately-WRONG adversary driver FAILS in every mutation mode.
            # If the harness ever PASSES a wrong driver, the parity baseline is
            # worthless → --selftest exits non-zero. Repeatable guard (§ tests-review).
            SELFTEST=1; shift ;;
        --selftest-fast)
            # Same negative guard but the oracle leg is skipped and the bad-driver
            # legs disable the slow network sidecars + large-tree round-trips. The
            # file:// manifest/id mutations are what the negative test asserts, and
            # they run on every fixture, so the catch is fully exercised far faster.
            SELFTEST=1; SELFTEST_FAST=1; shift ;;
        --)
            shift; break ;;
        *)
            echo "run_parity.sh: unknown flag '$1'" >&2; exit 2 ;;
    esac
done

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GOLDEN_DIR="${SCRIPT_DIR}"
EXPECTED_DIR="${GOLDEN_DIR}/expected"
WORK_DIR="${GOLDEN_DIR}/work"
WORKSPACE_ROOT="$(cd "${GOLDEN_DIR}/../.." && pwd)"
DOCKER_SCRIPTS="$(cd "${GOLDEN_DIR}/../../docker/scripts" && pwd)"

# Add workspace debug + release binary dirs to PATH so `snapdir`, `snapdir-sftp-store`,
# `snapdir-ssh-store` etc. are findable by the snapdir CLI for network backends.
# release takes precedence over debug.
if [[ -d "${WORKSPACE_ROOT}/target/release" ]]; then
    export PATH="${WORKSPACE_ROOT}/target/release:${PATH}"
fi
if [[ -d "${WORKSPACE_ROOT}/target/debug" ]]; then
    export PATH="${WORKSPACE_ROOT}/target/debug:${PATH}"
fi

# ---------------------------------------------------------------------------
# SELF-TEST (negative guard) — must come BEFORE driver selection, because it
# re-invokes THIS script with the oracle + the deliberately-wrong driver.
#
# Asserts BOTH halves of the contract:
#   1. the oracle reference driver PASSES (exit 0), and
#   2. the adversary "_adversary_wrong.sh" driver FAILS (exit non-zero, a FAIL
#      line) in EVERY mutation mode.
# A harness that passes ANY wrong driver is worthless → this exits non-zero.
# ---------------------------------------------------------------------------
if [[ "${SELFTEST}" == "1" ]]; then
    SELF="${BASH_SOURCE[0]}"
    ORACLE_REF="${GOLDEN_DIR}/drivers/rust.sh"
    BAD_REF="${GOLDEN_DIR}/drivers/_adversary_wrong.sh"
    st_fail=0

    if [[ ! -x "${BAD_REF}" ]]; then
        echo "SELFTEST FAIL — bad driver not found/executable: ${BAD_REF}" >&2
        exit 2
    fi

    # In fast mode, suppress the slow legs for the bad-driver runs: the negative
    # assertion lives entirely in the file:// manifest/id cells, which always run.
    st_env=()
    if [[ "${SELFTEST_FAST}" == "1" ]]; then
        st_env=(PARITY_SELFTEST_SKIP_NET=1 PARITY_SELFTEST_SKIP_ROUNDTRIP=1)
    fi

    # --- 1. oracle must PASS (skipped in fast mode; proven separately by the
    #        canonical `--driver oracle` verification the PM re-runs) ---
    if [[ "${SELFTEST_FAST}" != "1" ]]; then
        echo "[selftest] oracle driver must PASS ..." >&2
        if "${SELF}" --driver "${ORACLE_REF}" >/tmp/selftest_oracle.out 2>&1; then
            echo "SELFTEST oracle=PASS (exit 0)"
        else
            echo "SELFTEST oracle=FAIL — oracle driver did NOT exit 0 (see /tmp/selftest_oracle.out)" >&2
            tail -5 /tmp/selftest_oracle.out >&2 || true
            st_fail=1
        fi
    fi

    # --- 2. bad driver must FAIL in every mutation mode ---
    BAD_MODES=(flip-id-hex drop-file perm-byte no-trailing-nl extra-trailing-nl id-only-correct)
    for m in "${BAD_MODES[@]}"; do
        bad_out="$(env "${st_env[@]+"${st_env[@]}"}" BAD_MODE="${m}" "${SELF}" --driver "${BAD_REF}" 2>/dev/null)"
        bad_rc=$?
        nfail="$(printf '%s\n' "${bad_out}" | grep -c '^FAIL' || true)"
        if [[ "${bad_rc}" -ne 0 && "${nfail}" -ge 1 ]]; then
            echo "SELFTEST bad[${m}]=CAUGHT (exit ${bad_rc}, ${nfail} FAIL line(s))"
        else
            echo "SELFTEST bad[${m}]=ESCAPED — harness PASSED a wrong driver (exit ${bad_rc}, ${nfail} FAILs) — PARITY BASELINE IS WORTHLESS" >&2
            printf '%s\n' "${bad_out}" | grep '^SUMMARY' >&2 || true
            st_fail=1
        fi
    done

    if [[ "${st_fail}" -eq 0 ]]; then
        echo "SELFTEST OVERALL=PASS — oracle passes AND every wrong driver is caught"
        exit 0
    else
        echo "SELFTEST OVERALL=FAIL — see lines above" >&2
        exit 1
    fi
fi

# ---------------------------------------------------------------------------
# Driver selection (§0, §4)
# The keyword "oracle" (or "rust") selects tests/golden/drivers/rust.sh.
# Any other value is treated as a path/command to the driver executable.
# ---------------------------------------------------------------------------
PARITY_DRIVER="${PARITY_DRIVER:-${GOLDEN_DIR}/drivers/rust.sh}"

# Resolve the "oracle" keyword to the oracle reference driver path.
if [[ "${PARITY_DRIVER}" == "oracle" || "${PARITY_DRIVER}" == "rust" ]]; then
    PARITY_DRIVER="${GOLDEN_DIR}/drivers/rust.sh"
fi

if [[ ! -x "${PARITY_DRIVER}" ]]; then
    echo "run_parity.sh: driver not found or not executable: '${PARITY_DRIVER}'" >&2
    exit 2
fi

NIGHTLY="${SNAPDIR_PARITY_NIGHTLY:-0}"

# ---------------------------------------------------------------------------
# Per-run hermetic scratch (§1.6)
# ---------------------------------------------------------------------------
RUN_CACHE="$(mktemp -d)"
RUN_CATALOG_DIR="$(mktemp -d)"
RUN_CATALOG="${RUN_CATALOG_DIR}/catalog.db"

FILE_STORE_BASE=""   # set in step 3; cleaned up in cleanup()

cleanup() {
    rm -rf "${RUN_CACHE}" "${RUN_CATALOG_DIR}"
    [[ -n "${FILE_STORE_BASE}" ]] && rm -rf "${FILE_STORE_BASE}" 2>/dev/null || true
    # Attempt sidecars-down (best-effort, only if we started them)
    if [[ "${_SIDECARS_UP:-0}" == "1" ]]; then
        "${DOCKER_SCRIPTS}/sidecars-down.sh" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# The oracle reference driver (always tests/golden/drivers/rust.sh) is used
# for independent re-walks in the anti-cheat id check and round-trip §2.3.
# ---------------------------------------------------------------------------
ORACLE_DRIVER="${GOLDEN_DIR}/drivers/rust.sh"

# Locate the raw oracle binary (snapdir) for the anti-cheat id stdin pipe.
# The oracle_id_from_stdin function pipes a manifest text to `snapdir id` (stdin
# mode, no path argument — per §1.2: the harness pipes the driver's own manifest
# through the oracle `snapdir id` to compute the BLAKE3 independently).
locate_oracle_bin() {
    local workspace
    workspace="$(cd "${GOLDEN_DIR}/../.." && pwd)"
    local release_bin="${workspace}/target/release/snapdir"
    local debug_bin="${workspace}/target/debug/snapdir"
    if [[ -x "${release_bin}" ]]; then
        echo "${release_bin}"; return
    fi
    if [[ -x "${debug_bin}" ]]; then
        echo "${debug_bin}"; return
    fi
    # Build if absent
    (cd "${workspace}" && cargo build -p snapdir --locked) >&2
    echo "${debug_bin}"
}
ORACLE_BIN="$(locate_oracle_bin)"

# ---------------------------------------------------------------------------
# Driver invocation (§1.6): env-scrubbed, deterministic baseline
# ---------------------------------------------------------------------------
driver() {
    env -u SNAPDIR_STORE \
        -u SNAPDIR_OBJECTS_STORE \
        -u SNAPDIR_MANIFEST_CONTEXT \
        LC_ALL=C \
        SNAPDIR_NO_PROGRESS=true \
        SNAPDIR_CACHE_DIR="${RUN_CACHE}" \
        SNAPDIR_CATALOG_DB_PATH="${RUN_CATALOG}" \
        "${PARITY_DRIVER}" "$@"
}

# oracle_id_from_stdin: pipe a manifest through the raw oracle `snapdir id`
# (stdin mode). Used by the anti-cheat id-self-consistency check (§2.2).
# This NEVER goes through the driver — it always uses the frozen oracle binary.
oracle_id_from_stdin() {
    env -u SNAPDIR_STORE \
        -u SNAPDIR_OBJECTS_STORE \
        -u SNAPDIR_MANIFEST_CONTEXT \
        LC_ALL=C \
        SNAPDIR_NO_PROGRESS=true \
        "${ORACLE_BIN}" id
}

# oracle_id: run oracle id on a path (for round-trip re-walk §2.3).
# ALWAYS uses the oracle driver, regardless of which driver is under test.
oracle_id() {
    env -u SNAPDIR_STORE \
        -u SNAPDIR_OBJECTS_STORE \
        -u SNAPDIR_MANIFEST_CONTEXT \
        LC_ALL=C \
        SNAPDIR_NO_PROGRESS=true \
        SNAPDIR_CACHE_DIR="${RUN_CACHE}" \
        SNAPDIR_CATALOG_DB_PATH="${RUN_CATALOG}" \
        "${ORACLE_DRIVER}" id "$@"
}

# ---------------------------------------------------------------------------
# Pass/Fail/Skip tallies and reporters (§4)
# ---------------------------------------------------------------------------
PASSED=0; FAILED=0; SKIPPED=0

pass() { echo "PASS  $*"; PASSED=$((PASSED+1)); }
fail() { echo "FAIL  $*"; FAILED=$((FAILED+1)); }
skip() { echo "SKIP  $*"; SKIPPED=$((SKIPPED+1)); }

# Print a diff to stderr on byte mismatch (§4)
show_diff() {
    local got="$1" expected_file="$2" label="$3"
    echo "--- expected: ${label}" >&2
    echo "+++ got from driver" >&2
    diff "${expected_file}" <(printf '%s' "${got}") | head -40 >&2 || true
}

# ---------------------------------------------------------------------------
# Fixture list and their fixture-dir/flag mappings (§2.1, §3.1)
# symlinks contributes TWO captures: symlinks-follow and symlinks-nofollow.
# ---------------------------------------------------------------------------
# All 9 logical captures (8 fixtures × one of which yields 2 captures):
FIXTURES=(
    empty
    single-file
    nested
    unicode-paths
    symlinks-follow
    symlinks-nofollow
    identical-content
    large-tree
    permissions
)

# Small-fixture set for per-PR network round-trips (§3.2).
# large-tree is nightly-only; symlinks-nofollow is manifest/id only (§appendix A).
SMALL_RT=(
    empty
    single-file
    nested
    unicode-paths
    symlinks-follow
    identical-content
    permissions
)

# Map a fixture capture name to (workdir-path extra-flag...) for driver invocation.
fixture_path_and_flags() {
    case "$1" in
        symlinks-follow)   echo "${WORK_DIR}/symlinks" ;;
        symlinks-nofollow) echo "${WORK_DIR}/symlinks --no-follow" ;;
        *)                 echo "${WORK_DIR}/$1" ;;
    esac
}

# ---------------------------------------------------------------------------
# STEP 1: Hermetic fixture generation (§5)
# Re-run gen_fixtures.sh to materialize the work/ tree + expected/* freshly.
# ---------------------------------------------------------------------------
echo "[run_parity] Generating fixtures (hermetic)..." >&2
if ! "${GOLDEN_DIR}/gen_fixtures.sh" >/dev/null 2>&1; then
    echo "FAIL  gen_fixtures — gen_fixtures.sh exited non-zero" >&2
    "${GOLDEN_DIR}/gen_fixtures.sh" >&2 || true   # re-run to show errors
    exit 2
fi
echo "[run_parity] Fixtures generated." >&2

# Verify all expected baselines exist (§4: missing baseline = hard error, not skip).
for F in "${FIXTURES[@]}"; do
    exp_manifest="${EXPECTED_DIR}/${F}.manifest"
    exp_id="${EXPECTED_DIR}/${F}.id"
    if [[ ! -f "${exp_manifest}" ]]; then
        echo "run_parity.sh: ERROR — missing baseline ${exp_manifest}" >&2
        exit 2
    fi
    if [[ ! -f "${exp_id}" ]]; then
        echo "run_parity.sh: ERROR — missing baseline ${exp_id}" >&2
        exit 2
    fi
done

# ---------------------------------------------------------------------------
# STEP 2: manifest + id parity on file:// (ALWAYS — §2.1, §2.2, §3.1)
# ---------------------------------------------------------------------------
echo "[run_parity] === manifest + id parity (file/local, all fixtures) ===" >&2

for F in "${FIXTURES[@]}"; do
    read -r fpath extra_flags_str <<<"$(fixture_path_and_flags "${F}")"
    # Split extra_flags_str into an array (may be empty or "--no-follow")
    extra_flags=()
    if [[ -n "${extra_flags_str:-}" ]]; then
        # shellcheck disable=SC2206
        extra_flags=(${extra_flags_str})
    fi

    exp_manifest="${EXPECTED_DIR}/${F}.manifest"
    exp_id_file="${EXPECTED_DIR}/${F}.id"
    exp_id="$(cat "${exp_id_file}")"

    # (a) manifest parity — BYTE-FOR-BYTE (§2.1)
    # The driver emits the manifest to stdout. We capture its stdout to a temp
    # file with REDIRECTION (not $() command substitution) so NOT A SINGLE BYTE
    # is normalized — in particular the trailing newline is preserved exactly as
    # the driver emitted it. §2.1 names "a missing/extra trailing newline" as a
    # FAIL, so we MUST compare the raw bytes; capturing through "$(driver …)"
    # would strip ALL trailing newlines and then re-add exactly one, silently
    # normalizing a real divergence (a no-/extra-trailing-newline driver would
    # wrongly PASS). Redirect to a file and cmp byte-for-byte against expected/.
    got_manifest_tmp="$(mktemp)"
    if driver manifest "${fpath}" "${extra_flags[@]+"${extra_flags[@]}"}" >"${got_manifest_tmp}" 2>/dev/null; then
        if cmp -s "${got_manifest_tmp}" "${exp_manifest}"; then
            pass "${F} file manifest"
        else
            fail "${F} file manifest — byte mismatch vs expected/${F}.manifest"
            diff "${exp_manifest}" "${got_manifest_tmp}" | head -40 >&2 || true
            # If the only difference is a trailing-newline divergence, diff above
            # may render nothing useful; surface byte sizes for the record.
            echo "    (expected ${exp_manifest} = $(wc -c <"${exp_manifest}") bytes; got = $(wc -c <"${got_manifest_tmp}") bytes)" >&2
        fi
    else
        fail "${F} file manifest — driver exited non-zero"
    fi
    rm -f "${got_manifest_tmp}"

    # (b) id parity — 64-hex exact (§2.2)
    if got_id="$(driver id "${fpath}" "${extra_flags[@]+"${extra_flags[@]}"}" 2>/dev/null)"; then
        # Strip any trailing newline from the captured id (bash command substitution
        # strips trailing newlines, so this is already done, but be explicit).
        got_id="${got_id%$'\n'}"
        got_id_trimmed="${got_id%%[[:space:]]}"
        exp_id_trimmed="${exp_id%%[[:space:]]}"
        if [[ "${got_id_trimmed}" == "${exp_id_trimmed}" ]]; then
            pass "${F} file id"
        else
            fail "${F} file id — got '${got_id_trimmed}', want '${exp_id_trimmed}'"
        fi

        # (c) self-consistency anti-cheat: id == BLAKE3(driver's own manifest) (§2.2)
        # Re-derive the id INDEPENDENTLY by piping the driver's OWN manifest bytes
        # through the FROZEN oracle `snapdir id` (stdin mode) — never through the
        # driver — and assert it equals the id the driver reported. This catches a
        # driver that hardcodes the expected hex while emitting a divergent
        # manifest. Capture the driver's manifest with redirection (raw bytes, no
        # $() normalization) so the BLAKE3 is over the EXACT bytes the driver
        # emitted, including any trailing-newline divergence.
        got_manifest2_tmp="$(mktemp)"
        if driver manifest "${fpath}" "${extra_flags[@]+"${extra_flags[@]}"}" >"${got_manifest2_tmp}" 2>/dev/null; then
            self_id="$(oracle_id_from_stdin <"${got_manifest2_tmp}" 2>/dev/null)"
            self_id="${self_id%$'\n'}"
            self_id="${self_id%%[[:space:]]}"
            if [[ "${got_id_trimmed}" == "${self_id}" ]]; then
                pass "${F} file id-self-consistent"
            else
                fail "${F} file id-self-consistent — driver id '${got_id_trimmed}' != BLAKE3(driver manifest) '${self_id}'"
            fi
        else
            fail "${F} file id-self-consistent — driver manifest re-invocation failed"
        fi
        rm -f "${got_manifest2_tmp}"
    else
        fail "${F} file id — driver exited non-zero"
        fail "${F} file id-self-consistent — skipped (id failed)"
    fi
done

# ---------------------------------------------------------------------------
# STEP 3: file:// round-trip (self-check for the oracle driver — §2.3 note)
# The spec says file:// MAY round-trip; for the oracle driver this is a cheap
# self-check that the round-trip path (push→fetch→checkout) is wired correctly.
# We run it for the small fixture set (not large-tree, to stay within budget).
#
# SKIP rationale for symlinks-follow (§2.3, Appendix A):
#   The symlinks-follow fixture contains dangling symlinks (broken → ./nonexistent)
#   and escaping symlinks (escape → ../../etc/hostname). After checkout, the
#   dangling link's target still doesn't exist and the escaping link points to a
#   path that may resolve differently, so oracle re-manifest of the checkout dest
#   does not reproduce the same id. Per spec §2.3: "Fixtures whose checkout cannot
#   be losslessly re-walked ... MAY be excluded from the round-trip set when the
#   destination filesystem cannot reproduce them; the harness MUST document any
#   such exclusion as a SKIP line (never a silent drop)."
#
# Fixtures excluded from round-trip for the above reason (always):
ROUNDTRIP_SKIP_ALWAYS=(symlinks-follow)
# ---------------------------------------------------------------------------
echo "[run_parity] === file:// round-trip (small fixtures, self-check) ===" >&2

# Fast self-test mode: skip the slow round-trip + network legs. The negative
# guard's catch lives entirely in the file:// manifest/id cells above, which
# always run, so this does not weaken the negative assertion — it only trims the
# self-test wall-clock. NEVER set by a real harness run (only --selftest-fast).
if [[ "${PARITY_SELFTEST_SKIP_ROUNDTRIP:-0}" == "1" ]]; then
    echo "NOTE file:// round-trip + network legs skipped (PARITY_SELFTEST_SKIP_ROUNDTRIP=1, selftest-fast)" >&2
    echo ""
    echo "SUMMARY driver=${PARITY_DRIVER} PASSED=${PASSED} FAILED=${FAILED} SKIPPED=${SKIPPED}"
    if [[ "${FAILED}" -eq 0 ]]; then exit 0; else exit 1; fi
fi

FILE_STORE_BASE="$(mktemp -d)"

# Helper: check if a fixture is in the always-skip list.
is_roundtrip_skip_always() {
    local f="$1"
    for s in "${ROUNDTRIP_SKIP_ALWAYS[@]}"; do
        [[ "${f}" == "${s}" ]] && return 0
    done
    return 1
}

for F in "${SMALL_RT[@]}"; do
    read -r fpath extra_flags_str <<<"$(fixture_path_and_flags "${F}")"
    extra_flags=()
    if [[ -n "${extra_flags_str:-}" ]]; then
        # shellcheck disable=SC2206
        extra_flags=(${extra_flags_str})
    fi

    # SKIP fixtures that cannot be losslessly reproduced (per §2.3 + Appendix A).
    if is_roundtrip_skip_always "${F}"; then
        skip "${F} file roundtrip — dangling/escaping symlinks cannot be losslessly re-walked on checkout dest (§2.3)"
        continue
    fi

    exp_id_file="${EXPECTED_DIR}/${F}.id"
    exp_id="$(cat "${exp_id_file}")"
    exp_id="${exp_id%%[[:space:]]}"

    # Use a per-fixture isolated prefix to avoid cross-fixture collision.
    file_store_uri="file://${FILE_STORE_BASE}/${F}"
    mkdir -p "${FILE_STORE_BASE}/${F}"

    # Step 1: id_local
    id_local="$(driver id "${fpath}" "${extra_flags[@]+"${extra_flags[@]}"}" 2>/dev/null)" || {
        fail "${F} file roundtrip — id_local failed"; continue
    }
    id_local="${id_local%%[[:space:]]}"

    # Verify id_local == expected (should already be checked above, but be explicit)
    if [[ "${id_local}" != "${exp_id}" ]]; then
        fail "${F} file roundtrip — id_local '${id_local}' != expected '${exp_id}'"
        continue
    fi

    # Step 2: push → id_push must equal id_local
    id_push="$(driver push "${fpath}" "${file_store_uri}" 2>/dev/null)" || {
        fail "${F} file roundtrip — push failed"; continue
    }
    id_push="${id_push%%[[:space:]]}"
    if [[ "${id_push}" != "${id_local}" ]]; then
        fail "${F} file roundtrip — id_push '${id_push}' != id_local '${id_local}'"
        continue
    fi

    # Step 3: fetch (all objects + manifest retrievable, BLAKE3-verifying)
    if ! driver fetch "${id_push}" "${file_store_uri}" >/dev/null 2>&1; then
        fail "${F} file roundtrip — fetch failed"
        continue
    fi

    # Step 4: checkout → re-manifest via oracle → assert id_dest == id_push
    dest="$(mktemp -d)"
    if ! driver checkout "${id_push}" "${file_store_uri}" "${dest}" >/dev/null 2>&1; then
        rm -rf "${dest}"
        fail "${F} file roundtrip — checkout failed"
        continue
    fi

    id_dest="$(oracle_id "${dest}" 2>/dev/null)" || {
        rm -rf "${dest}"
        fail "${F} file roundtrip — oracle re-manifest of dest failed"
        continue
    }
    id_dest="${id_dest%%[[:space:]]}"
    rm -rf "${dest}"

    if [[ "${id_dest}" == "${id_push}" ]]; then
        pass "${F} file roundtrip"
    else
        fail "${F} file roundtrip — dest re-manifests to '${id_dest}', want '${id_push}'"
    fi
done

# ---------------------------------------------------------------------------
# STEP 4: per-PR network legs — s3:// + sftp:// (§3.2)
# Boot sidecars; SKIP-not-fail if sidecars are unavailable.
# Only the small fixtures (not large-tree — nightly only per §3.3).
# ---------------------------------------------------------------------------
echo "[run_parity] === per-PR network legs (s3 + sftp, small fixtures) ===" >&2

_SIDECARS_UP=0

# Source the sidecars env if available (for creds / port / key path).
SIDECARS_ENV="${DOCKER_SCRIPTS}/sidecars.env"

# Attempt to bring sidecars up (best-effort; if they fail → all net legs SKIP).
# Self-test-fast may skip the network legs entirely (booting sidecars is the
# slowest part); the negative guard does not depend on them.
SIDECARS_HEALTHY=false
if [[ "${PARITY_SELFTEST_SKIP_NET:-0}" == "1" ]]; then
    echo "NOTE network legs skipped (PARITY_SELFTEST_SKIP_NET=1, selftest-fast)" >&2
    SIDECARS_HEALTHY=skip
elif "${DOCKER_SCRIPTS}/sidecars-up.sh" >/dev/null 2>&1; then
    _SIDECARS_UP=1
    if "${DOCKER_SCRIPTS}/sidecars-health.sh" >/dev/null 2>&1; then
        SIDECARS_HEALTHY=true
    fi
fi

if [[ "${SIDECARS_HEALTHY}" == "true" && -f "${SIDECARS_ENV}" ]]; then
    # shellcheck source=/dev/null
    . "${SIDECARS_ENV}"

    # -- S3 round-trips via minio ------------------------------------------------
    # The snapdir S3 store reads:
    #   SNAPDIR_S3_TEST_ENDPOINT  — the minio endpoint URL
    #   AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY — minio root creds
    # We also need a bucket. Create one via mc (minio client) or the mc alias.
    # The S3 URI for snapdir is: s3://<bucket>/<prefix>
    # The CLI wires the endpoint via env SNAPDIR_S3_TEST_ENDPOINT (per s3_store.rs §952)
    # when no endpoint is baked into the URI.
    #
    # Check whether `mc` (the MinIO client) or `aws` is available to create the bucket.

    S3_BUCKET="snapdir-parity-pr"
    S3_ENDPOINT="${MINIO_ADDR:-127.0.0.1:9000}"
    S3_ENDPOINT_URL="http://${S3_ENDPOINT}"
    MINIO_USER="${MINIO_ROOT_USER:-snapdir-test}"
    MINIO_PASS="${MINIO_ROOT_PASSWORD:-snapdir-test-secret}"

    # Create the S3 bucket (idempotent). Tries mc, aws, then falls back to
    # Python stdlib AWS SigV4 (available everywhere Python 3 is, no external deps).
    create_s3_bucket() {
        local endpoint_url="$1" bucket="$2" access="$3" secret="$4"
        if command -v mc >/dev/null 2>&1; then
            mc alias set parity-minio "${endpoint_url}" "${access}" "${secret}" >/dev/null 2>&1 && \
            mc mb --ignore-existing "parity-minio/${bucket}" >/dev/null 2>&1 && return 0
        fi
        if command -v aws >/dev/null 2>&1; then
            AWS_ACCESS_KEY_ID="${access}" AWS_SECRET_ACCESS_KEY="${secret}" \
            AWS_DEFAULT_REGION=us-east-1 \
            aws s3 mb "s3://${bucket}" --endpoint-url "${endpoint_url}" \
                --no-verify-ssl >/dev/null 2>&1 && return 0
        fi
        # Fallback: Python stdlib AWS SigV4 PUT-bucket (works with MinIO).
        python3 - "${endpoint_url}" "${bucket}" "${access}" "${secret}" <<'PYEOF' 2>/dev/null
import hashlib, hmac, datetime, urllib.request, sys
endpoint, bucket, access, secret = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
region = 'us-east-1'; service = 's3'
now = datetime.datetime.utcnow()
date = now.strftime('%Y%m%d'); amz_dt = now.strftime('%Y%m%dT%H%M%SZ')
host = endpoint.split('//')[1]
payload_hash = hashlib.sha256(b'').hexdigest()
can_hdrs = f'host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_dt}\n'
signed_hdrs = 'host;x-amz-content-sha256;x-amz-date'
can_req = f'PUT\n/{bucket}\n\n{can_hdrs}\n{signed_hdrs}\n{payload_hash}'
cred_scope = f'{date}/{region}/{service}/aws4_request'
sts = f'AWS4-HMAC-SHA256\n{amz_dt}\n{cred_scope}\n' + hashlib.sha256(can_req.encode()).hexdigest()
def sign(key, msg): return hmac.new(key, msg.encode(), hashlib.sha256).digest()
k = sign(sign(sign(sign(f'AWS4{secret}'.encode(), date), region), service), 'aws4_request')
sig = hmac.new(k, sts.encode(), hashlib.sha256).hexdigest()
auth = f'AWS4-HMAC-SHA256 Credential={access}/{cred_scope}, SignedHeaders={signed_hdrs}, Signature={sig}'
req = urllib.request.Request(f'{endpoint}/{bucket}', data=b'', method='PUT')
req.add_header('Authorization', auth); req.add_header('X-Amz-Date', amz_dt)
req.add_header('X-Amz-Content-Sha256', payload_hash)
try:
    urllib.request.urlopen(req); sys.exit(0)
except urllib.error.HTTPError as e:
    sys.exit(0 if e.code == 409 else 1)
PYEOF
    }

    S3_AVAILABLE=false
    if create_s3_bucket "${S3_ENDPOINT_URL}" "${S3_BUCKET}" "${MINIO_USER}" "${MINIO_PASS}"; then
        S3_AVAILABLE=true
    fi

    if [[ "${S3_AVAILABLE}" == "true" ]]; then
        for F in "${SMALL_RT[@]}"; do
            # SKIP dangling/escaping symlink fixtures (per §2.3 + Appendix A).
            if is_roundtrip_skip_always "${F}"; then
                skip "${F} s3 roundtrip — dangling/escaping symlinks cannot be losslessly re-walked on checkout dest (§2.3)"
                continue
            fi

            read -r fpath extra_flags_str <<<"$(fixture_path_and_flags "${F}")"
            extra_flags=()
            if [[ -n "${extra_flags_str:-}" ]]; then
                # shellcheck disable=SC2206
                extra_flags=(${extra_flags_str})
            fi

            exp_id_file="${EXPECTED_DIR}/${F}.id"
            exp_id="$(cat "${exp_id_file}")"
            exp_id="${exp_id%%[[:space:]]}"

            # Isolated prefix per fixture per run (avoid collision on repeats).
            s3_prefix="parity-run-$$/${F}"
            s3_uri="s3://${S3_BUCKET}/${s3_prefix}"

            # id_local (already verified above; re-check for the round-trip assertion)
            id_local="$(env -u SNAPDIR_STORE -u SNAPDIR_OBJECTS_STORE -u SNAPDIR_MANIFEST_CONTEXT \
                LC_ALL=C SNAPDIR_NO_PROGRESS=true \
                SNAPDIR_CACHE_DIR="${RUN_CACHE}" SNAPDIR_CATALOG_DB_PATH="${RUN_CATALOG}" \
                SNAPDIR_S3_STORE_ENDPOINT_URL="${S3_ENDPOINT_URL}" \
                AWS_ACCESS_KEY_ID="${MINIO_USER}" AWS_SECRET_ACCESS_KEY="${MINIO_PASS}" \
                AWS_DEFAULT_REGION=us-east-1 \
                "${PARITY_DRIVER}" id "${fpath}" "${extra_flags[@]+"${extra_flags[@]}"}" 2>/dev/null)" || {
                fail "${F} s3 roundtrip — id_local failed"; continue
            }
            id_local="${id_local%%[[:space:]]}"

            # push
            id_push="$(env -u SNAPDIR_STORE -u SNAPDIR_OBJECTS_STORE -u SNAPDIR_MANIFEST_CONTEXT \
                LC_ALL=C SNAPDIR_NO_PROGRESS=true \
                SNAPDIR_CACHE_DIR="${RUN_CACHE}" SNAPDIR_CATALOG_DB_PATH="${RUN_CATALOG}" \
                SNAPDIR_S3_STORE_ENDPOINT_URL="${S3_ENDPOINT_URL}" \
                AWS_ACCESS_KEY_ID="${MINIO_USER}" AWS_SECRET_ACCESS_KEY="${MINIO_PASS}" \
                AWS_DEFAULT_REGION=us-east-1 \
                "${PARITY_DRIVER}" push "${fpath}" "${s3_uri}" 2>/dev/null)" || {
                fail "${F} s3 roundtrip — push failed"; continue
            }
            id_push="${id_push%%[[:space:]]}"
            if [[ "${id_push}" != "${id_local}" ]]; then
                fail "${F} s3 roundtrip — id_push '${id_push}' != id_local '${id_local}'"
                continue
            fi

            # fetch
            env -u SNAPDIR_STORE -u SNAPDIR_OBJECTS_STORE -u SNAPDIR_MANIFEST_CONTEXT \
                LC_ALL=C SNAPDIR_NO_PROGRESS=true \
                SNAPDIR_CACHE_DIR="${RUN_CACHE}" SNAPDIR_CATALOG_DB_PATH="${RUN_CATALOG}" \
                SNAPDIR_S3_STORE_ENDPOINT_URL="${S3_ENDPOINT_URL}" \
                AWS_ACCESS_KEY_ID="${MINIO_USER}" AWS_SECRET_ACCESS_KEY="${MINIO_PASS}" \
                AWS_DEFAULT_REGION=us-east-1 \
                "${PARITY_DRIVER}" fetch "${id_push}" "${s3_uri}" >/dev/null 2>&1 || {
                fail "${F} s3 roundtrip — fetch failed"; continue
            }

            # checkout + oracle re-manifest
            s3_dest="$(mktemp -d)"
            env -u SNAPDIR_STORE -u SNAPDIR_OBJECTS_STORE -u SNAPDIR_MANIFEST_CONTEXT \
                LC_ALL=C SNAPDIR_NO_PROGRESS=true \
                SNAPDIR_CACHE_DIR="${RUN_CACHE}" SNAPDIR_CATALOG_DB_PATH="${RUN_CATALOG}" \
                SNAPDIR_S3_STORE_ENDPOINT_URL="${S3_ENDPOINT_URL}" \
                AWS_ACCESS_KEY_ID="${MINIO_USER}" AWS_SECRET_ACCESS_KEY="${MINIO_PASS}" \
                AWS_DEFAULT_REGION=us-east-1 \
                "${PARITY_DRIVER}" checkout "${id_push}" "${s3_uri}" "${s3_dest}" >/dev/null 2>&1 || {
                rm -rf "${s3_dest}"
                fail "${F} s3 roundtrip — checkout failed"; continue
            }
            id_dest="$(oracle_id "${s3_dest}" 2>/dev/null)" || {
                rm -rf "${s3_dest}"
                fail "${F} s3 roundtrip — oracle re-manifest failed"; continue
            }
            id_dest="${id_dest%%[[:space:]]}"
            rm -rf "${s3_dest}"

            if [[ "${id_dest}" == "${id_push}" ]]; then
                pass "${F} s3 roundtrip"
            else
                fail "${F} s3 roundtrip — dest re-manifests to '${id_dest}', want '${id_push}'"
            fi
        done
    else
        for F in "${SMALL_RT[@]}"; do
            skip "${F} s3 roundtrip — mc/aws not available; cannot create minio bucket"
        done
    fi

    # -- SFTP round-trips via sshd -----------------------------------------------
    # The snapdir sftp store reads:
    #   SNAPDIR_SFTP_STORE_IDENTITY_FILE — path to the SSH private key
    #   SNAPDIR_SFTP_STORE_KNOWN_HOSTS   — known_hosts file
    #   SNAPDIR_SFTP_STORE_PORT          — SSH port
    # The sftp:// URI: sftp://<host>/<absolute-remote-path>
    # The sidecar sshd uses the current user (not "snapdir-parity") on the loopback.

    SFTP_PORT="${SSHD_PORT:-2222}"
    SFTP_KEY="${SSH_KEY:-/workspace/scripts/sidecar_ssh_key}"
    SFTP_KNOWN_HOSTS="${SSH_KNOWN_HOSTS:-/workspace/scripts/sidecar_known_hosts}"

    SFTP_AVAILABLE=false
    if [[ -f "${SFTP_KEY}" && -f "${SFTP_KNOWN_HOSTS}" ]]; then
        SFTP_AVAILABLE=true
    fi

    if [[ "${SFTP_AVAILABLE}" == "true" ]]; then
        # Remote base dir — use a temp dir on the sidecar host (same machine = /tmp).
        sftp_remote_base="$(mktemp -d)"
        chmod 0700 "${sftp_remote_base}"

        for F in "${SMALL_RT[@]}"; do
            # SKIP dangling/escaping symlink fixtures (per §2.3 + Appendix A).
            if is_roundtrip_skip_always "${F}"; then
                skip "${F} sftp roundtrip — dangling/escaping symlinks cannot be losslessly re-walked on checkout dest (§2.3)"
                continue
            fi

            read -r fpath extra_flags_str <<<"$(fixture_path_and_flags "${F}")"
            extra_flags=()
            if [[ -n "${extra_flags_str:-}" ]]; then
                # shellcheck disable=SC2206
                extra_flags=(${extra_flags_str})
            fi

            exp_id_file="${EXPECTED_DIR}/${F}.id"
            exp_id="$(cat "${exp_id_file}")"
            exp_id="${exp_id%%[[:space:]]}"

            # Isolated remote dir per fixture
            sftp_remote_dir="${sftp_remote_base}/${F}"
            mkdir -p "${sftp_remote_dir}"
            sftp_uri="sftp://127.0.0.1${sftp_remote_dir}"

            # id_local
            id_local="$(env -u SNAPDIR_STORE -u SNAPDIR_OBJECTS_STORE -u SNAPDIR_MANIFEST_CONTEXT \
                LC_ALL=C SNAPDIR_NO_PROGRESS=true \
                SNAPDIR_CACHE_DIR="${RUN_CACHE}" SNAPDIR_CATALOG_DB_PATH="${RUN_CATALOG}" \
                SNAPDIR_SFTP_STORE_IDENTITY_FILE="${SFTP_KEY}" \
                SNAPDIR_SFTP_STORE_KNOWN_HOSTS="${SFTP_KNOWN_HOSTS}" \
                SNAPDIR_SFTP_STORE_PORT="${SFTP_PORT}" \
                "${PARITY_DRIVER}" id "${fpath}" "${extra_flags[@]+"${extra_flags[@]}"}" 2>/dev/null)" || {
                fail "${F} sftp roundtrip — id_local failed"; continue
            }
            id_local="${id_local%%[[:space:]]}"

            # push
            id_push="$(env -u SNAPDIR_STORE -u SNAPDIR_OBJECTS_STORE -u SNAPDIR_MANIFEST_CONTEXT \
                LC_ALL=C SNAPDIR_NO_PROGRESS=true \
                SNAPDIR_CACHE_DIR="${RUN_CACHE}" SNAPDIR_CATALOG_DB_PATH="${RUN_CATALOG}" \
                SNAPDIR_SFTP_STORE_IDENTITY_FILE="${SFTP_KEY}" \
                SNAPDIR_SFTP_STORE_KNOWN_HOSTS="${SFTP_KNOWN_HOSTS}" \
                SNAPDIR_SFTP_STORE_PORT="${SFTP_PORT}" \
                "${PARITY_DRIVER}" push "${fpath}" "${sftp_uri}" 2>/dev/null)" || {
                fail "${F} sftp roundtrip — push failed"; continue
            }
            id_push="${id_push%%[[:space:]]}"
            if [[ "${id_push}" != "${id_local}" ]]; then
                fail "${F} sftp roundtrip — id_push '${id_push}' != id_local '${id_local}'"
                continue
            fi

            # fetch
            env -u SNAPDIR_STORE -u SNAPDIR_OBJECTS_STORE -u SNAPDIR_MANIFEST_CONTEXT \
                LC_ALL=C SNAPDIR_NO_PROGRESS=true \
                SNAPDIR_CACHE_DIR="${RUN_CACHE}" SNAPDIR_CATALOG_DB_PATH="${RUN_CATALOG}" \
                SNAPDIR_SFTP_STORE_IDENTITY_FILE="${SFTP_KEY}" \
                SNAPDIR_SFTP_STORE_KNOWN_HOSTS="${SFTP_KNOWN_HOSTS}" \
                SNAPDIR_SFTP_STORE_PORT="${SFTP_PORT}" \
                "${PARITY_DRIVER}" fetch "${id_push}" "${sftp_uri}" >/dev/null 2>&1 || {
                fail "${F} sftp roundtrip — fetch failed"; continue
            }

            # checkout + oracle re-manifest
            sftp_dest="$(mktemp -d)"
            env -u SNAPDIR_STORE -u SNAPDIR_OBJECTS_STORE -u SNAPDIR_MANIFEST_CONTEXT \
                LC_ALL=C SNAPDIR_NO_PROGRESS=true \
                SNAPDIR_CACHE_DIR="${RUN_CACHE}" SNAPDIR_CATALOG_DB_PATH="${RUN_CATALOG}" \
                SNAPDIR_SFTP_STORE_IDENTITY_FILE="${SFTP_KEY}" \
                SNAPDIR_SFTP_STORE_KNOWN_HOSTS="${SFTP_KNOWN_HOSTS}" \
                SNAPDIR_SFTP_STORE_PORT="${SFTP_PORT}" \
                "${PARITY_DRIVER}" checkout "${id_push}" "${sftp_uri}" "${sftp_dest}" >/dev/null 2>&1 || {
                rm -rf "${sftp_dest}"
                fail "${F} sftp roundtrip — checkout failed"; continue
            }
            id_dest="$(oracle_id "${sftp_dest}" 2>/dev/null)" || {
                rm -rf "${sftp_dest}"
                fail "${F} sftp roundtrip — oracle re-manifest failed"; continue
            }
            id_dest="${id_dest%%[[:space:]]}"
            rm -rf "${sftp_dest}"

            if [[ "${id_dest}" == "${id_push}" ]]; then
                pass "${F} sftp roundtrip"
            else
                fail "${F} sftp roundtrip — dest re-manifests to '${id_dest}', want '${id_push}'"
            fi
        done
        rm -rf "${sftp_remote_base}" 2>/dev/null || true
    else
        for F in "${SMALL_RT[@]}"; do
            skip "${F} sftp roundtrip — SSH key or known_hosts not found (sidecar key not generated yet)"
        done
    fi

    # Shut down sidecars (always, after all network legs).
    "${DOCKER_SCRIPTS}/sidecars-down.sh" >/dev/null 2>&1 || true
    _SIDECARS_UP=0
else
    # Sidecars unhealthy or failed to start → SKIP all per-PR network legs (§3.4).
    NET_PR_BACKENDS=(s3 sftp)
    for B in "${NET_PR_BACKENDS[@]}"; do
        for F in "${SMALL_RT[@]}"; do
            skip "${F} ${B} roundtrip — sidecar unavailable"
        done
    done
fi

# ---------------------------------------------------------------------------
# STEP 5: nightly legs (§3.3)
# b2 + ssh across all fixtures; large-tree×all-net; live gs (creds-gated).
# ---------------------------------------------------------------------------
if [[ "${NIGHTLY}" == "1" ]]; then
    echo "[run_parity] === nightly legs (b2 + ssh + large-tree×net + live gs) ===" >&2
    # TODO (Phases 37-43): implement b2, ssh, gs legs here following the same
    # pattern as the s3/sftp legs above, gated on sidecar health and GCS creds.
    # For now, emit explicit SKIP lines so nothing is silently dropped (§3.4).

    ALL_FIXTURES_RT=(
        empty single-file nested unicode-paths
        symlinks-follow identical-content permissions large-tree
    )

    for F in "${ALL_FIXTURES_RT[@]}"; do
        skip "${F} b2 roundtrip — nightly b2 leg not yet implemented (Phase 37+)"
        skip "${F} ssh roundtrip — nightly ssh leg not yet implemented (Phase 37+)"
    done

    # Live gs (GCS) — creds-gated (§3.4)
    if [[ -n "${GOOGLE_APPLICATION_CREDENTIALS:-}" || -n "${SNAPDIR_GCS_TEST_BUCKET:-}" ]]; then
        for F in "${ALL_FIXTURES_RT[@]}"; do
            skip "${F} gs roundtrip — nightly gs leg not yet implemented (Phase 37+)"
        done
    else
        for F in "${ALL_FIXTURES_RT[@]}"; do
            skip "${F} gs roundtrip — GCS creds absent"
        done
    fi
else
    echo "NOTE nightly legs deferred (SNAPDIR_PARITY_NIGHTLY unset)" >&2
fi

# ---------------------------------------------------------------------------
# SUMMARY (§4)
# ---------------------------------------------------------------------------
echo ""
echo "SUMMARY driver=${PARITY_DRIVER} PASSED=${PASSED} FAILED=${FAILED} SKIPPED=${SKIPPED}"

if [[ "${FAILED}" -eq 0 ]]; then
    exit 0
else
    exit 1
fi
