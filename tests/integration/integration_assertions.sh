#!/usr/bin/env bash
# .gatesmith/pending-tests/integration_assertions.sh
#
# ADVERSARY-authored assertion + byte-exact-oracle layer for the Phase-44
# cross-language integration relay (gate: integration-relay-spec-tests).
#
# CONTRACT (the only shared doc): .gatesmith/reviews/integration-relay.md (LOCKED).
# Authored BLACK-BOX from that contract + the frozen binding public surfaces
# ONLY. The 6 example apps DO NOT EXIST yet — this file defines WHAT correct
# looks like; the `integration-apps-impl` lane makes it pass. It is authored to
# FAIL without the apps, never weakened to pass.
#
# The examples lane wires this in by `git mv`-ing it under tests/integration/
# and sourcing it from tests/integration/run_relay.sh, which then calls
# `integration_relay_main`. THIS FILE IS NOT EDITED to add apps/Dockerfiles.
#
# ===========================================================================
# HARNESS CONTRACT — what run_relay.sh (the examples lane) must provide
# ===========================================================================
# run_relay.sh runs on the HOST and orchestrates docker (it already boots the
# `snapdir-relay` bridge network + the `snapdir-minio` service container + the
# `snapdir-integ` bucket — see integration-harness-scaffold). After staging the
# 6 packaged artifacts and building the 6 barebones app images it must:
#
#   1. export the relay parameters (or accept the defaults below):
#        RELAY_LANGS        : the relay languages, IN CHOREOGRAPHY ORDER
#                             (default: "node python go java cpp zig")
#        RELAY_STORE        : the shared store URI (default s3://snapdir-integ/relay)
#        RELAY_WORK         : host dir holding the harness-owned fixtures + scratch
#                             (default ${REPO_ROOT}/tests/integration/.relay-work)
#        RELAY_DEV_IMAGE    : the dev/build image the apps must NOT be (default
#                             snapdir-bindings:dev)
#        BINDINGS_IMAGE     : same dev image, used to compute the oracle
#        NETWORK_NAME       : the docker bridge network (default snapdir-relay)
#
#   2. mount ${RELAY_WORK} read-write into EVERY app container at /relay, pass
#      the RELAY_S3_ENV creds block, attach --network "${NETWORK_NAME}", and
#      define these hooks:
#
#        relay_app <lang> <subcmd> [args...]
#            Run lang's app image once; print ONLY the app's stdout; return the
#            app's exit code. The app is a tiny CLI over the BINDING API (NOT the
#            snapdir CLI). The app CLI the harness drives is EXACTLY:
#               app push <dir> <store>            -> prints the 64-hex id
#               app pull <id>  <store> <dest>     -> checks out; exit 0
#               app id   <dir>                    -> prints the 64-hex id
#               app diff <store@idA> <store@idB>  -> prints the porcelain diff
#            (dirs/dests are /relay-rooted paths inside the container.)
#
#        relay_leg_available <lang>   -> exit 0 if lang's image built/usable,
#                                        non-0 to DEFER that leg (§7 deferral).
#        relay_image <lang>           -> prints the built image tag for lang.
#        relay_build_log <lang>       -> prints the path to lang's image BUILD LOG
#                                        (host file) for the credential-leak scan.
#
#   3. source THIS file and call `integration_relay_main`. Its exit code is the
#      relay's exit code (0 only if every executed leg is byte-exact GREEN).
#
# A leg whose base image cannot be pulled is DEFERRED (logged, never silently
# skipped); a deferred PRODUCER is oracle-seeded so downstream legs still run.
# The relay FAILS if no app driver is wired or if ZERO legs actually execute —
# which is the correct state today (no apps).
#
# Standalone:  bash integration_assertions.sh --selfcheck-oracle
#   Builds the fixtures + computes ID_A/ID_B/DIFF_AB via the IN-TREE snapdir
#   (in BINDINGS_IMAGE) and prints them. The ONLY mode runnable before the apps
#   exist; proves the oracle logic. The full relay legitimately FAILS until then.
# ---------------------------------------------------------------------------

set -uo pipefail
LC_ALL=C
export LC_ALL

# ---------------------------------------------------------------------------
# Parameters (all overridable by run_relay.sh / the env).
# ---------------------------------------------------------------------------
_RELAY_SELF="${BASH_SOURCE[0]}"
: "${RELAY_REPO_ROOT:=${REPO_ROOT:-$(git -C "$(dirname "${_RELAY_SELF}")" rev-parse --show-toplevel 2>/dev/null || pwd)}}"
REPO_ROOT="${RELAY_REPO_ROOT}"

: "${BINDINGS_IMAGE:=snapdir-bindings:dev}"
: "${RELAY_DEV_IMAGE:=${BINDINGS_IMAGE}}"
: "${NETWORK_NAME:=snapdir-relay}"
: "${RELAY_STORE:=s3://snapdir-integ/relay}"
: "${RELAY_LANGS:=node python go java cpp zig}"
# Fixtures + scratch live UNDER the repo so a single `-v ${REPO_ROOT}:/w` mount
# (the established binding-gate pattern) exposes them to the in-image oracle as
# /w/${RELAY_WORK_REL}. run_relay.sh mounts the SAME host dir at /relay for apps.
RELAY_WORK_REL="tests/integration/.relay-work"
: "${RELAY_WORK:=${REPO_ROOT}/${RELAY_WORK_REL}}"

# Minio creds block (mirrors run_relay.sh RELAY_S3_ENV / docker/scripts/sidecars.env).
# THE SECRET that must NEVER appear in an image layer, a build log, or image Env.
RELAY_MINIO_USER="snapdir-test"
RELAY_MINIO_SECRET="snapdir-test-secret"
# The minio S3 endpoint (docker-DNS name on NETWORK_NAME). Hardcoded to match
# run_relay.sh's RELAY_S3_ENV + relay_oracle_seed below — the on-network address
# the "minio genuinely used" bucket-listing checks reach to PROVE the relay's
# pushes land in REAL S3 (not a local s3:/ file-path fallback).
RELAY_S3_ENDPOINT="http://snapdir-minio:9000"

# Oracle results (filled by relay_oracle_compute).
ORACLE_ID_A=""
ORACLE_ID_B=""
ORACLE_DIFF_FILE=""   # raw bytes of the expected diff (byte-exact compare target)

# Tallies.
RELAY_PASS=0
RELAY_FAIL=0
RELAY_DEFER=0
RELAY_EXECUTED=0   # count of real app invocations — 0 ⇒ "no apps ran" ⇒ FAIL

# ---------------------------------------------------------------------------
# Reporters (PASS/FAIL/DEFER/NOTE — mirrors tests/golden/run_parity.sh).
# ---------------------------------------------------------------------------
_pass()  { echo "PASS   $*"; RELAY_PASS=$((RELAY_PASS+1)); }
_fail()  { echo "FAIL   $*" >&2; RELAY_FAIL=$((RELAY_FAIL+1)); }
_defer() { echo "DEFER  $*"; RELAY_DEFER=$((RELAY_DEFER+1)); }
_note()  { echo "NOTE   $*"; }

# Byte-exact 64-hex id assertion (names the lang+step, fails LOUDLY).
assert_id() {
    local label="$1" got="$2" want="$3"
    got="${got//$'\n'/}"; got="${got//$'\r'/}"; got="${got// /}"
    if [[ ! "${want}" =~ ^[0-9a-f]{64}$ ]]; then
        _fail "${label} — ORACLE id is not 64-hex: '${want}' (oracle bug)"; return 1
    fi
    if [[ "${got}" == "${want}" ]]; then
        _pass "${label} == ${want}"
    else
        _fail "${label} — expected id '${want}', got '${got}'"
    fi
}

# Byte-exact diff assertion: compare RAW bytes (incl. the trailing newline +
# the literal TAB) against the oracle diff file. A normalized $() capture would
# silently hide a trailing-newline divergence, so the caller passes a FILE of
# the app's raw stdout.
assert_diff_file() {
    local label="$1" got_file="$2" want_file="$3"
    if cmp -s "${got_file}" "${want_file}"; then
        _pass "${label} == oracle DIFF_AB ($(wc -c <"${want_file}") bytes)"
    else
        _fail "${label} — diff bytes differ from oracle DIFF_AB"
        {
            echo "--- expected (oracle DIFF_AB) ---"; cat -vet "${want_file}"
            echo "--- got (app) ---"; cat -vet "${got_file}"
        } >&2
    fi
}

# ===========================================================================
# Deterministic, harness-owned fixtures (§2 of the locked contract).
# Created ONCE on the shared volume so the oracle and ALL apps see IDENTICAL
# bytes + perms. No mtimes affect the manifest (path/type/perms/checksum/size
# only), so the ids are reproducible across hosts. seed2 = seed + ONE file.
# ===========================================================================
relay_make_fixtures() {
    local seed="${RELAY_WORK}/seed" seed2="${RELAY_WORK}/seed2"
    rm -rf "${RELAY_WORK}/seed" "${RELAY_WORK}/seed2" "${RELAY_WORK}/scratch"
    mkdir -p "${seed}/data/notes" "${seed}/scripts" "${RELAY_WORK}/scratch"

    printf 'hello relay\n'        > "${seed}/greeting.txt"
    printf '1\n2\n3\n'            > "${seed}/data/numbers.txt"
    printf '# snapdir relay\n'    > "${seed}/data/notes/readme.md"
    printf '#!/bin/sh\necho hi\n' > "${seed}/scripts/run.sh"

    # Deterministic perms (the executable bit is load-bearing for the manifest).
    chmod 0644 "${seed}/greeting.txt" "${seed}/data/numbers.txt" "${seed}/data/notes/readme.md"
    chmod 0755 "${seed}/scripts/run.sh"
    chmod 0755 "${seed}" "${seed}/data" "${seed}/data/notes" "${seed}/scripts"

    # seed2 = byte-identical copy of seed + exactly one added file at the root.
    cp -a "${seed}" "${seed2}"
    printf 'added by the relay\n' > "${seed2}/added.txt"
    chmod 0644 "${seed2}/added.txt"

    mkdir -p "${RELAY_WORK}/scratch"
}

# ===========================================================================
# Byte-exact ORACLE (§3): compute ID_A, ID_B, DIFF_AB ONCE via the IN-TREE
# snapdir reference inside BINDINGS_IMAGE. tests/golden/drivers/rust.sh is the
# established oracle driver (rust.sh id/manifest/push); `snapdir diff` is run
# through the SAME resolved in-tree binary rust.sh wraps (rust.sh has no diff
# verb). DIFF_AB is derived from the two single-manifest file stores, which is
# byte-identical to a pinned-id diff over one store (classify() is a pure
# map-diff over the two manifests).
# ===========================================================================
relay_oracle_compute() {
    relay_make_fixtures

    local out
    # A fresh container starts with a clean env (no host SNAPDIR_STORE etc.), so
    # the deterministic baseline needs only LC_ALL=C + SNAPDIR_NO_PROGRESS.
    out="$(docker run --rm \
        -v "${REPO_ROOT}":/w -w /w \
        -e LC_ALL=C -e SNAPDIR_NO_PROGRESS=true \
        "${BINDINGS_IMAGE}" sh -lc '
            set -eu
            REL="'"${RELAY_WORK_REL}"'"
            # Resolve the in-tree oracle binary exactly as rust.sh does.
            if   [ -x target/release/snapdir ]; then SNAP=target/release/snapdir
            elif [ -x target/debug/snapdir ];   then SNAP=target/debug/snapdir
            else cargo build -p snapdir --locked >&2; SNAP=target/debug/snapdir
            fi
            # ID_A / ID_B via the established rust.sh id driver.
            IDA="$(bash tests/golden/drivers/rust.sh id "$REL/seed")"
            IDB="$(bash tests/golden/drivers/rust.sh id "$REL/seed2")"
            # DIFF_AB: push each seed to its own file store, then diff FROM=A TO=B.
            A="$(mktemp -d)"; B="$(mktemp -d)"
            bash tests/golden/drivers/rust.sh push "$REL/seed"  "file://$A" >/dev/null
            bash tests/golden/drivers/rust.sh push "$REL/seed2" "file://$B" >/dev/null
            DIFF_FILE="$(mktemp)"
            "$SNAP" diff --no-progress --from "file://$A" --to "file://$B" > "$DIFF_FILE"
            printf "ORACLE_ID_A=%s\n" "$IDA"
            printf "ORACLE_ID_B=%s\n" "$IDB"
            printf "ORACLE_DIFF_B64=%s\n" "$(base64 < "$DIFF_FILE" | tr -d "\n")"
        ' 2>/dev/null)" || { _fail "oracle — docker run failed (image ${BINDINGS_IMAGE} / docker unavailable)"; return 1; }

    ORACLE_ID_A="$(printf '%s\n' "${out}" | sed -n 's/^ORACLE_ID_A=//p')"
    ORACLE_ID_B="$(printf '%s\n' "${out}" | sed -n 's/^ORACLE_ID_B=//p')"
    local diff_b64; diff_b64="$(printf '%s\n' "${out}" | sed -n 's/^ORACLE_DIFF_B64=//p')"

    if [[ ! "${ORACLE_ID_A}" =~ ^[0-9a-f]{64}$ || ! "${ORACLE_ID_B}" =~ ^[0-9a-f]{64}$ ]]; then
        _fail "oracle — ID_A/ID_B not computed (A='${ORACLE_ID_A}' B='${ORACLE_ID_B}')"; return 1
    fi
    if [[ "${ORACLE_ID_A}" == "${ORACLE_ID_B}" ]]; then
        _fail "oracle — ID_A == ID_B (seed2 mutation produced no change; fixture bug)"; return 1
    fi

    ORACLE_DIFF_FILE="${RELAY_WORK}/oracle.diff"
    printf '%s' "${diff_b64}" | base64 -d > "${ORACLE_DIFF_FILE}" 2>/dev/null \
        || { _fail "oracle — could not decode DIFF_AB"; return 1; }

    # The contract pins DIFF_AB to EXACTLY one `A\t./added.txt` row (no other
    # A/D/M). Validate the oracle output itself, byte-for-byte.
    local expect_diff; expect_diff="$(printf 'A\t./added.txt\n')"
    if [[ "$(cat "${ORACLE_DIFF_FILE}")" != "${expect_diff}" ]]; then
        _fail "oracle — DIFF_AB is not exactly one 'A<TAB>./added.txt' row:"
        cat -vet "${ORACLE_DIFF_FILE}" >&2
        return 1
    fi
    return 0
}

# Seed the REAL relay store with a fixture via the IN-TREE oracle. Used ONLY as
# a fallback when a PRODUCER leg (Node push / Python push) is DEFERRED, so the
# downstream consumer legs still have objects to pull. Logged as SEED; the
# deferred producer's OWN assertion is still recorded DEFER (never PASS).
relay_oracle_seed() {
    local which="$1"  # seed | seed2
    docker run --rm \
        --network "${NETWORK_NAME}" \
        -v "${REPO_ROOT}":/w -w /w \
        -e LC_ALL=C -e SNAPDIR_NO_PROGRESS=true \
        -e "SNAPDIR_S3_STORE_ENDPOINT_URL=http://snapdir-minio:9000" \
        -e "AWS_ACCESS_KEY_ID=${RELAY_MINIO_USER}" \
        -e "AWS_SECRET_ACCESS_KEY=${RELAY_MINIO_SECRET}" \
        -e "AWS_DEFAULT_REGION=us-east-1" \
        "${BINDINGS_IMAGE}" sh -lc '
            set -eu
            REL="'"${RELAY_WORK_REL}"'"
            bash tests/golden/drivers/rust.sh push "$REL/'"${which}"'" "'"${RELAY_STORE}"'" >/dev/null
        ' >/dev/null 2>&1
}

# ===========================================================================
# (STRENGTHEN, real-S3 — Phase-44 re-audit) The relay now runs over the REAL
# minio S3 emulator. These checks PROVE minio is genuinely the backend and
# pin the regression that slipped through before (a push writing to a local
# `s3:/` file tree instead of S3). The file-store relay COULD NOT have these.
# ===========================================================================

# Parse "<bucket> <prefix>" from RELAY_STORE (s3://<bucket>/<prefix...>). The
# prefix may be empty (bare bucket). All on-network listing uses these.
_relay_store_bucket_prefix() {
    local rest="${RELAY_STORE#s3://}"
    local bucket="${rest%%/*}"
    local prefix=""
    [[ "${rest}" == */* ]] && prefix="${rest#*/}"
    printf '%s %s' "${bucket}" "${prefix}"
}

# relay_s3_list_keys <key-prefix> : print every S3 key under
# <bucket>/<key-prefix> in the relay bucket, ONE PER LINE. Runs a throwaway
# on-network container (so docker-DNS resolves snapdir-minio) and signs a real
# ListObjectsV2 with SigV4 — the SAME signing run_relay.sh uses for the bucket
# create/smoke. Exits non-zero (and prints nothing) if the LIST request errors,
# so callers can tell "minio unreachable" from "genuinely empty".
relay_s3_list_keys() {
    local key_prefix="$1"
    local bp bucket; bp="$(_relay_store_bucket_prefix)"; bucket="${bp%% *}"
    docker run --rm -i \
        --network "${NETWORK_NAME}" \
        "${BINDINGS_IMAGE}" \
        python3 - \
            "${RELAY_S3_ENDPOINT}" "${bucket}" \
            "${RELAY_MINIO_USER}" "${RELAY_MINIO_SECRET}" \
            "${key_prefix}" \
        <<'PYEOF'
import hashlib, hmac, datetime, urllib.request, urllib.parse, sys, re
endpoint, bucket, access, secret, prefix = sys.argv[1:6]
region, service = "us-east-1", "s3"
host = endpoint.split("//", 1)[1]
payload_hash = hashlib.sha256(b"").hexdigest()

def list_page(token):
    now = datetime.datetime.utcnow()
    date = now.strftime("%Y%m%d"); amz = now.strftime("%Y%m%dT%H%M%SZ")
    params = {"list-type": "2", "prefix": prefix, "max-keys": "1000"}
    if token:
        params["continuation-token"] = token
    cq = "&".join(f"{urllib.parse.quote(k, safe='')}={urllib.parse.quote(v, safe='')}"
                  for k, v in sorted(params.items()))
    can_hdrs = f"host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz}\n"
    signed = "host;x-amz-content-sha256;x-amz-date"
    can_req = f"GET\n/{bucket}\n{cq}\n{can_hdrs}\n{signed}\n{payload_hash}"
    scope = f"{date}/{region}/{service}/aws4_request"
    sts = f"AWS4-HMAC-SHA256\n{amz}\n{scope}\n" + hashlib.sha256(can_req.encode()).hexdigest()
    def sign(k, m): return hmac.new(k, m.encode(), hashlib.sha256).digest()
    k = sign(sign(sign(sign(f"AWS4{secret}".encode(), date), region), service), "aws4_request")
    sig = hmac.new(k, sts.encode(), hashlib.sha256).hexdigest()
    auth = (f"AWS4-HMAC-SHA256 Credential={access}/{scope}, "
            f"SignedHeaders={signed}, Signature={sig}")
    req = urllib.request.Request(f"{endpoint}/{bucket}?{cq}", method="GET")
    req.add_header("Authorization", auth); req.add_header("X-Amz-Date", amz)
    req.add_header("X-Amz-Content-Sha256", payload_hash)
    return urllib.request.urlopen(req).read().decode("utf-8", "replace")

token, keys = None, []
while True:
    body = list_page(token)
    keys += re.findall(r"<Key>([^<]*)</Key>", body)
    if "<IsTruncated>true</IsTruncated>" not in body:
        break
    m = re.search(r"<NextContinuationToken>([^<]*)</NextContinuationToken>", body)
    if not m:
        break
    token = m.group(1)
for kk in keys:
    print(kk)
PYEOF
}

# (PRECONDITION) The run's store prefix (a UNIQUE relay-$$ on a freshly booted
# minio) must be EMPTY before any push — so every object/manifest found later
# was provably written THIS run by a real app, not leftover state.
relay_assert_store_prefix_empty() {
    local bp prefix; bp="$(_relay_store_bucket_prefix)"; prefix="${bp#* }"
    local keys rc
    keys="$(relay_s3_list_keys "${prefix}/" 2>/dev/null)"; rc=$?
    if [[ ${rc} -ne 0 ]]; then
        _fail "minio precondition — could not LIST the relay bucket (minio unreachable / not real S3?) rc=${rc}"
        return 1
    fi
    if [[ -z "${keys//[$'\n']/}" ]]; then
        _pass "minio precondition — relay store prefix '${prefix}/' is empty before any push (fresh real S3)"
    else
        _fail "minio precondition — prefix '${prefix}/' is NOT empty before the relay (stale/wrong store): ${keys//$'\n'/ }"
    fi
}

# (THE check) After a producer push, the snapshot's MANIFEST — at its exact
# frozen sharded key — and at least one content OBJECT must ACTUALLY EXIST in
# the minio bucket under the run's prefix. This is what proves the relay is NOT
# silently falling back to a local store: a `s3:/`-as-file-path push would write
# nothing to S3, so the bucket would be empty here. Mirrors snapdir-core's
# store::manifest_path 3/3/3/rest sharding (`.manifests/<id0:3>/<id3:6>/<id6:9>/<id9:>`).
relay_assert_minio_used() {
    local id="$1" label="$2"
    if [[ ! "${id}" =~ ^[0-9a-f]{64}$ ]]; then
        _fail "${label} minio-used — id is not 64-hex '${id}' (cannot locate manifest key)"; return 1
    fi
    local bp prefix; bp="$(_relay_store_bucket_prefix)"; prefix="${bp#* }"
    local man_key="${prefix}/.manifests/${id:0:3}/${id:3:3}/${id:6:3}/${id:9}"
    local obj_prefix="${prefix}/.objects/"

    local man_hit rc
    man_hit="$(relay_s3_list_keys "${man_key}" 2>/dev/null)"; rc=$?
    if [[ ${rc} -ne 0 ]]; then
        _fail "${label} minio-used — LIST of the bucket failed (minio unreachable / not real S3?)"; return 1
    fi
    if printf '%s\n' "${man_hit}" | grep -qxF "${man_key}"; then
        _pass "${label} minio-used — manifest present in minio bucket at ${man_key}"
    else
        _fail "${label} minio-used — manifest ${man_key} ABSENT from minio ⇒ push did NOT reach real S3 (local s3:/ fallback?)"
    fi

    local obj_count
    obj_count="$(relay_s3_list_keys "${obj_prefix}" 2>/dev/null | grep -c . || true)"
    if [[ "${obj_count}" -ge 1 ]]; then
        _pass "${label} minio-used — ${obj_count} content object(s) present under ${obj_prefix} in minio"
    else
        _fail "${label} minio-used — NO objects under ${obj_prefix} in minio ⇒ push wrote nothing to real S3"
    fi
}

# (POST-CHOREOGRAPHY) Belt-and-suspenders: after BOTH producers (node→seed,
# python→seed2) have pushed, the bucket must hold content objects AND ≥2 distinct
# manifests (ID_A + ID_B) — proving BOTH producing languages reached real S3.
relay_assert_minio_populated() {
    local bp prefix; bp="$(_relay_store_bucket_prefix)"; prefix="${bp#* }"
    local objs mans
    objs="$(relay_s3_list_keys "${prefix}/.objects/"   2>/dev/null | grep -c . || true)"
    mans="$(relay_s3_list_keys "${prefix}/.manifests/" 2>/dev/null | grep -c . || true)"
    if [[ "${objs}" -ge 1 && "${mans}" -ge 2 ]]; then
        _pass "minio-populated — bucket holds ${objs} object(s) + ${mans} manifest(s) under ${prefix}/ (both snapshots in real S3)"
    else
        _fail "minio-populated — under ${prefix}/ objs=${objs} manifests=${mans} (expected ≥1 objects, ≥2 manifests A+B in real S3)"
    fi
}

# (REGRESSION GUARD) The file-path-misinterpretation symptom: a push that writes
# a literal `s3:/` (or other scheme) directory tree instead of using the network
# store. Assert NO such directory exists anywhere the relay can write to disk —
# the repo cwd (where the in-tree oracle/seed containers run with `-w /w`) and
# RELAY_WORK (mounted at /relay in EVERY app container). Its ABSENCE is the proof
# that no leg fell back to a local store. (Targeted name-match + pruned dirs so
# the scan stays fast and never false-flags real files.)
relay_assert_no_local_s3_dir() {
    local hits="" root found
    for root in "${REPO_ROOT}" "${RELAY_WORK}"; do
        [[ -d "${root}" ]] || continue
        found="$(find "${root}" -maxdepth 3 \
                    \( -path '*/.git' -o -path '*/target' -o -path '*/node_modules' \) -prune -o \
                    \( -name 's3:*' -o -name 'gs:*' -o -name 'b2:*' -o -name 'ssh:*' -o -name 'sftp:*' \) -print \
                    2>/dev/null)"
        [[ -n "${found}" ]] && hits="${hits}${found}"$'\n'
    done
    if [[ -z "${hits//[$'\n']/}" ]]; then
        _pass "no-local-store-dir — no literal s3:/gs:/b2:/ssh:/sftp: directory created (no file-path fallback)"
    else
        _fail "no-local-store-dir — a literal store-scheme directory exists (a push wrote to local disk, not real S3): ${hits//$'\n'/ }"
    fi
}

# ===========================================================================
# Leg execution (delegates the docker plumbing to run_relay.sh's relay_app).
# ===========================================================================
relay_leg_available() {
    # Default: every leg is DEFERRED (no apps yet). run_relay.sh overrides this
    # once the app images are built. Keeping the default at "deferred" means the
    # full relay cannot accidentally PASS before the apps exist.
    return 1
}

# Run an app step that prints nothing meaningful we assert (e.g. pull): returns
# the app exit code; counts a real execution.
relay_run() {
    RELAY_EXECUTED=$((RELAY_EXECUTED+1))
    relay_app "$@"
}

# Capture an app step's stdout to a file (raw bytes preserved).
relay_capture_to() {
    local out_file="$1"; shift
    RELAY_EXECUTED=$((RELAY_EXECUTED+1))
    relay_app "$@" > "${out_file}"
}

# ===========================================================================
# Blank-slate / packaging assertions (§5 — the whole point; be adversarial).
# For each available app image assert: it is NOT the dev/build image, carries
# NO Rust toolchain (so the native binding could not have been rebuilt in-tree),
# bakes NO snapdir CLI (apps use the BINDING API), and LEAKS NO credentials into
# its layers, its Config.Env, or its build log.
# ===========================================================================
_img_lacks() {
    # _img_lacks <image> <cmd> : PASS-contributing if `command -v <cmd>` is
    # empty in the image. Echoes "present" if found.
    #
    # CRITICAL: every app image sets its own ENTRYPOINT (e.g. ["node","app.mjs"]),
    # so a plain `docker run <image> sh -lc ...` is parsed as ARGS TO THE APP
    # (`node app.mjs sh -lc ...`) — the shell never runs, stdout is empty, and the
    # check passes VACUOUSLY for every image. We MUST override the entrypoint with
    # `--entrypoint sh` so the probe actually executes. We also use a NON-login
    # shell (`-c`, not `-lc`): a login shell sources /etc/profile which can RESET
    # PATH and hide a tool that is genuinely installed (e.g. /usr/local/go/bin),
    # producing a false "absent" PASS. `-c` inherits the image's ENV PATH.
    local image="$1" cmd="$2"
    docker run --rm --entrypoint sh "${image}" -c "command -v ${cmd} >/dev/null 2>&1 && echo present || true" 2>/dev/null
}

relay_assert_image_blank_slate() {
    local lang="$1" image="$2"

    if [[ -z "${image}" ]]; then
        _fail "${lang} packaging — relay_image '${lang}' returned no tag"; return 1
    fi

    # (a) NOT the dev/build image (compare resolved image IDs).
    local dev_id app_id
    dev_id="$(docker image inspect --format '{{.Id}}' "${RELAY_DEV_IMAGE}" 2>/dev/null || true)"
    app_id="$(docker image inspect --format '{{.Id}}' "${image}" 2>/dev/null || true)"
    if [[ -z "${app_id}" ]]; then
        _fail "${lang} packaging — image '${image}' does not exist"; return 1
    fi
    if [[ -n "${dev_id}" && "${app_id}" == "${dev_id}" ]]; then
        _fail "${lang} packaging — app image IS the dev image ${RELAY_DEV_IMAGE} (blank-slate violated)"
    else
        _pass "${lang} packaging — app image is not the dev image"
    fi

    # (b) NO Rust toolchain (no cargo/rustc) ⇒ the Rust binding could NOT have
    #     been rebuilt from source; it was installed prebuilt from the package.
    local has_cargo has_rustc has_minio
    has_cargo="$(_img_lacks "${image}" cargo)"
    has_rustc="$(_img_lacks "${image}" rustc)"
    has_minio="$(_img_lacks "${image}" minio)"
    if [[ -z "${has_cargo}" && -z "${has_rustc}" ]]; then
        _pass "${lang} packaging — no Rust toolchain (no in-tree binding rebuild)"
    else
        _fail "${lang} packaging — Rust toolchain present (cargo='${has_cargo}' rustc='${has_rustc}') ⇒ possible in-tree rebuild"
    fi
    # minio is a dev-image-only marker; a barebones base never carries it.
    if [[ -z "${has_minio}" ]]; then
        _pass "${lang} packaging — no minio binary (dev-image marker absent)"
    else
        _fail "${lang} packaging — minio binary present ⇒ image looks like the dev image"
    fi

    # (c) NO snapdir CLI baked in ⇒ the app drives the BINDING API, not the CLI.
    local has_cli
    has_cli="$(_img_lacks "${image}" snapdir)"
    if [[ -z "${has_cli}" ]]; then
        _pass "${lang} packaging — no snapdir CLI in image (app uses the binding API)"
    else
        _fail "${lang} packaging — a 'snapdir' CLI is on PATH ⇒ app may shell out to the CLI"
    fi

    # (d) NO snapdir workspace source copied in (an accidental `COPY . /w`). The
    #     packaged artifact — NOT an in-tree rebuild — is the only thing allowed.
    #     Probe the whole crate/binding source surface, not just the root manifest,
    #     and override the entrypoint so the shell actually runs (see _img_lacks).
    local has_src
    has_src="$(docker run --rm --entrypoint sh "${image}" -c '
        for p in /w/Cargo.toml /w/Cargo.lock /w/crates /w/bindings /w/src; do
            [ -e "$p" ] && { echo "$p"; }
        done
        # Also reject the source anchored anywhere obvious (a stray COPY . <dir>).
        for p in /app/Cargo.toml /build/Cargo.toml /snapdir/Cargo.toml; do
            [ -e "$p" ] && { echo "$p"; }
        done
        true' 2>/dev/null)"
    if [[ -z "${has_src}" ]]; then
        _pass "${lang} packaging — no snapdir workspace/crate source in image (packaged artifact only)"
    else
        _fail "${lang} packaging — snapdir source present in image (leaked: ${has_src//$'\n'/ }) ⇒ possible in-tree rebuild"
    fi

    # (e) NO credentials in image layers (docker history) or Config.Env.
    local hist env_blob
    hist="$(docker history --no-trunc "${image}" 2>/dev/null || true)"
    env_blob="$(docker image inspect --format '{{json .Config.Env}}' "${image}" 2>/dev/null || true)"
    if printf '%s\n%s\n' "${hist}" "${env_blob}" | grep -qF "${RELAY_MINIO_SECRET}"; then
        _fail "${lang} packaging — credential '${RELAY_MINIO_SECRET}' leaked into image layers/Env"
    elif printf '%s\n' "${env_blob}" | grep -qiE 'AWS_SECRET_ACCESS_KEY|AWS_ACCESS_KEY_ID'; then
        _fail "${lang} packaging — AWS credential baked into image Config.Env (must arrive only at runtime)"
    else
        _pass "${lang} packaging — no credentials in image layers/Env"
    fi
}

relay_assert_build_log_clean() {
    local lang="$1"
    declare -F relay_build_log >/dev/null || { _note "${lang} packaging — no relay_build_log hook; skipping log scan"; return 0; }
    local log; log="$(relay_build_log "${lang}" 2>/dev/null || true)"
    if [[ -z "${log}" || ! -f "${log}" ]]; then
        _note "${lang} packaging — build log not found (${log:-unset}); skipping log scan"
        return 0
    fi
    if grep -qF "${RELAY_MINIO_SECRET}" "${log}"; then
        _fail "${lang} packaging — credential '${RELAY_MINIO_SECRET}' leaked into the build log ${log}"
    else
        _pass "${lang} packaging — no credentials in the build log"
    fi
}

# ---------------------------------------------------------------------------
# (STRENGTHEN, review gate) Per-language IDENTITY proof — the relay must prove a
# GENUINELY cross-language collaboration, not one binding masquerading as six.
# Two independent angles:
#   * the app image's ENTRYPOINT matches THAT language's invocation form, and
#   * a positive runtime probe confirms THAT language's runtime/compiler is the
#     stack actually present (so the python leg really runs python, etc.).
# (Image-ID distinctness across all six is asserted once in the main loop.)
# ---------------------------------------------------------------------------
relay_assert_image_language() {
    local lang="$1" image="$2"
    [[ -z "${image}" ]] && return 0   # already FAILed upstream

    local ep want_ep probe
    ep="$(docker image inspect --format '{{json .Config.Entrypoint}}' "${image}" 2>/dev/null || true)"
    case "${lang}" in
        node)   want_ep='app.mjs';  probe='node --version' ;;
        python) want_ep='app.py';   probe='python3 --version' ;;
        java)   want_ep='App';      probe='java -version' ;;
        go)     want_ep='/app';     probe='go version' ;;
        cpp)    want_ep='/app/app'; probe='g++ --version' ;;
        # zig final image is bare debian:bookworm-slim — its identity is a
        # statically-linked /app binary plus the ABSENCE of every OTHER stack's
        # runtime (so it cannot secretly be another language's reused image).
        zig)    want_ep='/app';     probe='__zig_bare__' ;;
        *)      _fail "${lang} identity — unknown language (no fingerprint)"; return 1 ;;
    esac

    if [[ "${ep}" == *"${want_ep}"* ]]; then
        _pass "${lang} identity — entrypoint is this language's app (${want_ep})"
    else
        _fail "${lang} identity — entrypoint '${ep}' is not '${want_ep}' (wrong-language image?)"
    fi

    if [[ "${probe}" == "__zig_bare__" ]]; then
        # The zig leg's image must NOT carry any other language's runtime.
        local intruder
        intruder="$(docker run --rm --entrypoint sh "${image}" -c '
            for c in node python3 go g++ gcc javac java cargo rustc; do
                command -v "$c" >/dev/null 2>&1 && echo "$c"
            done; true' 2>/dev/null)"
        if [[ -z "${intruder}" ]]; then
            _pass "${lang} identity — bare runtime, no other-language toolchain (genuine zig leg)"
        else
            _fail "${lang} identity — foreign runtime in zig image: ${intruder//$'\n'/ }"
        fi
    else
        # `sh -c` (NON-login) so the image's ENV PATH is honoured (a login shell
        # drops e.g. /usr/local/go/bin and would false-fail the go probe).
        if docker run --rm --entrypoint sh "${image}" -c "${probe}" >/dev/null 2>&1; then
            _pass "${lang} identity — ${probe%% *} runtime present (genuine ${lang} leg)"
        else
            _fail "${lang} identity — '${probe}' did not run ⇒ image is not a real ${lang} stack"
        fi
    fi
}

# ---------------------------------------------------------------------------
# (STRENGTHEN, security) Credentials must arrive ONLY at runtime via `-e`. The
# blank-slate check already scans `docker history` + Config.Env; this also scans
# the IMAGE FILESYSTEM (where a baked-in config/source secret would hide). The
# entrypoint is overridden so the grep actually runs.
# ---------------------------------------------------------------------------
relay_assert_image_fs_no_creds() {
    local lang="$1" image="$2"
    [[ -z "${image}" ]] && return 0
    local hit
    hit="$(docker run --rm --entrypoint sh "${image}" -c '
        grep -rIl "'"${RELAY_MINIO_SECRET}"'" /app /root /home /etc /usr/local 2>/dev/null | head -3
        true' 2>/dev/null)"
    if [[ -z "${hit}" ]]; then
        _pass "${lang} packaging — credential not baked into the image filesystem"
    else
        _fail "${lang} packaging — credential '${RELAY_MINIO_SECRET}' found in image filesystem: ${hit//$'\n'/ }"
    fi
}

# ---------------------------------------------------------------------------
# (STRENGTHEN, host-side static) The Dockerfile must build FROM the contract's
# declared BAREBONES base (§5) — never snapdir-bindings:dev / a toolchain image.
# Reads the in-tree Dockerfile (host artifact the adversary owns reviewing).
# ---------------------------------------------------------------------------
relay_assert_dockerfile_base() {
    local lang="$1"
    local df="${REPO_ROOT}/examples/${lang}/Dockerfile"
    if [[ ! -f "${df}" ]]; then
        _fail "${lang} packaging — Dockerfile missing at ${df}"; return 1
    fi
    local want_base
    case "${lang}" in
        node)   want_base='node:22-slim' ;;
        python) want_base='python:3.12-slim' ;;
        java)   want_base='eclipse-temurin:17-jdk' ;;
        go)     want_base='golang:1.24-bookworm' ;;
        cpp)    want_base='gcc:13' ;;
        zig)    want_base='debian:bookworm-slim' ;;
        *)      _fail "${lang} packaging — unknown language (no base)"; return 1 ;;
    esac

    # No stage may build from the dev/build image (would smuggle the toolchain +
    # source in, defeating the blank-slate proof).
    if grep -qiE '^[[:space:]]*FROM[[:space:]]+snapdir-bindings' "${df}"; then
        _fail "${lang} packaging — Dockerfile FROM snapdir-bindings (blank-slate base violated)"
        return 1
    fi
    # The FINAL stage (last FROM) must be the declared barebones base.
    local final_from
    final_from="$(grep -iE '^[[:space:]]*FROM[[:space:]]' "${df}" | tail -1)"
    if [[ "${final_from}" == *"${want_base}"* ]]; then
        _pass "${lang} packaging — final image FROM declared barebones base (${want_base})"
    else
        _fail "${lang} packaging — final base '${final_from}' is not the declared '${want_base}'"
    fi
}

# ---------------------------------------------------------------------------
# (STRENGTHEN, host-side static) The example app must drive the BINDING API, not
# shell out to a `snapdir` CLI. Assert it imports/links the binding AND contains
# NO subprocess-spawn of an external snapdir process.
# ---------------------------------------------------------------------------
relay_assert_app_source_uses_binding() {
    local lang="$1"
    local src import_re forbid_re
    case "${lang}" in
        node)   src="${REPO_ROOT}/examples/node/app.mjs"; import_re="@snapdir/snapdir";              forbid_re='child_process|execSync|spawnSync|execFile' ;;
        python) src="${REPO_ROOT}/examples/python/app.py"; import_re='^import snapdir|^from snapdir'; forbid_re='subprocess|os\.system|os\.popen' ;;
        go)     src="${REPO_ROOT}/examples/go/app.go";      import_re='snapdir/bindings/go';           forbid_re='os/exec' ;;
        cpp)    src="${REPO_ROOT}/examples/cpp/app.cpp";    import_re='snapdir\.hpp';                  forbid_re='\bsystem\(|popen\(|execv|execl' ;;
        zig)    src="${REPO_ROOT}/examples/zig/app.zig";    import_re='@import\("snapdir"\)';          forbid_re='std\.process\.Child|ChildProcess' ;;
        java)   src="${REPO_ROOT}/examples/java/App.java";  import_re='io\.snapdir\.';                 forbid_re='ProcessBuilder|Runtime\.getRuntime\(\)\.exec' ;;
        *)      _fail "${lang} api-not-cli — unknown language"; return 1 ;;
    esac
    if [[ ! -f "${src}" ]]; then
        _fail "${lang} api-not-cli — app source missing at ${src}"; return 1
    fi
    if grep -qE "${import_re}" "${src}"; then
        _pass "${lang} api-not-cli — app imports/links the binding (${import_re})"
    else
        _fail "${lang} api-not-cli — app does NOT import the binding (${import_re} absent in ${src})"
    fi
    if grep -qE "${forbid_re}" "${src}"; then
        _fail "${lang} api-not-cli — app shells out to a subprocess (matched /${forbid_re}/) ⇒ may invoke the CLI"
    else
        _pass "${lang} api-not-cli — app spawns no external process (uses the binding API only)"
    fi
}

relay_assert_packaging() {
    local lang="$1"
    local image; image="$(relay_image "${lang}" 2>/dev/null || true)"
    relay_assert_image_blank_slate "${lang}" "${image}"
    relay_assert_build_log_clean "${lang}"
    relay_assert_image_language "${lang}" "${image}"
    relay_assert_image_fs_no_creds "${lang}" "${image}"
    relay_assert_dockerfile_base "${lang}"
    relay_assert_app_source_uses_binding "${lang}"
}

# ---------------------------------------------------------------------------
# (STRENGTHEN) The whole relay is meaningless if two legs share one image — that
# would let a SINGLE binding masquerade as the whole cross-language chain. Assert
# every available app image resolves to a DISTINCT image id.
# ---------------------------------------------------------------------------
relay_assert_images_distinct() {
    local seen_langs="" seen_ids="" lang id dup=0
    for lang in ${RELAY_LANGS}; do
        relay_leg_available "${lang}" || continue
        id="$(docker image inspect --format '{{.Id}}' "$(relay_image "${lang}")" 2>/dev/null || true)"
        [[ -z "${id}" ]] && continue
        local prev_lang
        for prev_lang in ${seen_langs}; do
            if [[ " ${seen_ids} " == *" ${prev_lang}:${id} "* ]]; then
                _fail "distinct-images — ${lang} shares image id with ${prev_lang} (single-binding masquerade)"
                dup=1
            fi
        done
        seen_ids="${seen_ids} ${lang}:${id}"
        seen_langs="${seen_langs} ${lang}"
    done
    # Robust pairwise check: collapse ids and compare unique vs total count.
    local ids total uniq
    ids="$(printf '%s\n' ${seen_ids} | sed 's/^[^:]*://')"
    total="$(printf '%s\n' "${ids}" | grep -c . || true)"
    uniq="$(printf '%s\n' "${ids}" | sort -u | grep -c . || true)"
    if [[ "${dup}" -eq 0 && "${total}" -ge 2 && "${total}" == "${uniq}" ]]; then
        _pass "distinct-images — all ${total} built app images have distinct image ids"
    elif [[ "${total}" -lt 2 ]]; then
        _note "distinct-images — fewer than 2 legs available; distinctness trivially holds"
    elif [[ "${total}" != "${uniq}" && "${dup}" -eq 0 ]]; then
        _fail "distinct-images — ${total} images collapse to ${uniq} unique ids (image reuse across legs)"
    fi
}

# ===========================================================================
# The relay choreography (§4). Each step asserts BYTE-EXACT vs the oracle. A
# deferred leg is logged; a deferred PRODUCER oracle-seeds the store so the
# downstream legs still run. Re-id steps pull into a per-lang scratch dir.
# ===========================================================================
# Push <which-fixture> from <lang>, asserting the printed id == <expected oracle>.
# On defer, oracle-seed the store with <which> so consumers can still pull.
_step_push() {
    local lang="$1" fixture="$2" want="$3" seedname="$4" label="$5"
    if relay_leg_available "${lang}"; then
        local f; f="$(mktemp)"
        if relay_capture_to "${f}" "${lang}" push "/relay/${fixture}" "${RELAY_STORE}"; then
            assert_id "${label}" "$(cat "${f}")" "${want}"
            # (STRENGTHEN, real-S3) prove THIS app's push genuinely reached minio:
            # its manifest + objects must now exist in the bucket (not a local s3:/).
            relay_assert_minio_used "${want}" "${label}"
        else
            _fail "${label} — ${lang} app 'push' exited non-zero"
        fi
        rm -f "${f}"
    else
        _defer "${label} — ${lang} leg unavailable; oracle-seeding ${seedname} so downstream legs run"
        relay_oracle_seed "${seedname}" || _fail "${label} — oracle-seed of ${seedname} failed (downstream legs would break)"
    fi
}

# Pull <id> from <lang> into a scratch dir, then re-id it, asserting == <id>.
_step_pull_reid() {
    local lang="$1" id="$2" label="$3"
    if ! relay_leg_available "${lang}"; then
        _defer "${label} — ${lang} leg unavailable"
        return 0
    fi
    local dest="/relay/scratch/${lang}"
    rm -rf "${RELAY_WORK}/scratch/${lang}"
    if ! relay_run "${lang}" pull "${id}" "${RELAY_STORE}" "${dest}"; then
        _fail "${label} — ${lang} app 'pull' exited non-zero"; return 1
    fi
    local f; f="$(mktemp)"
    if relay_capture_to "${f}" "${lang}" id "${dest}"; then
        assert_id "${label}" "$(cat "${f}")" "${id}"
    else
        _fail "${label} — ${lang} app 're-id' exited non-zero"
    fi
    rm -f "${f}"
}

# Java diff(store@ID_A, store@ID_B) — byte-exact vs the oracle DIFF_AB.
_step_diff() {
    local lang="$1" label="$2"
    if ! relay_leg_available "${lang}"; then
        _defer "${label} — ${lang} leg unavailable"
        return 0
    fi
    local f; f="$(mktemp)"
    if relay_capture_to "${f}" "${lang}" diff "${RELAY_STORE}@${ORACLE_ID_A}" "${RELAY_STORE}@${ORACLE_ID_B}"; then
        assert_diff_file "${label}" "${f}" "${ORACLE_DIFF_FILE}"
    else
        _fail "${label} — ${lang} app 'diff' exited non-zero"
    fi
    rm -f "${f}"
}

relay_summary() {
    echo ""
    echo "SUMMARY relay PASSED=${RELAY_PASS} FAILED=${RELAY_FAIL} DEFERRED=${RELAY_DEFER} EXECUTED_LEGS=${RELAY_EXECUTED}"
    if [[ "${RELAY_FAIL}" -ne 0 ]]; then
        echo "RELAY FAIL — ${RELAY_FAIL} assertion(s) failed" >&2
        return 1
    fi
    if [[ "${RELAY_EXECUTED}" -eq 0 ]]; then
        echo "RELAY FAIL — no relay leg executed (apps not wired / all deferred)" >&2
        return 1
    fi
    echo "RELAY PASS"
    return 0
}

# ===========================================================================
# Top-level entry point — run_relay.sh sources this file and calls it.
# ===========================================================================
integration_relay_main() {
    echo "[relay] === cross-language integration relay (oracle-driven) ==="

    if ! declare -F relay_app >/dev/null; then
        _fail "no app driver — run_relay.sh must define relay_app(); the example apps do not exist yet"
        relay_summary; return 1
    fi
    if ! declare -F relay_image >/dev/null; then
        _fail "no relay_image hook — run_relay.sh must expose the per-language image tags"
        relay_summary; return 1
    fi

    echo "[relay] computing byte-exact oracle (in ${BINDINGS_IMAGE}) ..."
    if ! relay_oracle_compute; then
        relay_summary; return 1
    fi
    _note "oracle ID_A   = ${ORACLE_ID_A}"
    _note "oracle ID_B   = ${ORACLE_ID_B}"
    _note "oracle DIFF_AB= $(cat -vet "${ORACLE_DIFF_FILE}")"

    # --- Blank-slate / packaging proof for every available app image ----------
    echo "[relay] --- packaging / blank-slate assertions ---"
    local lang
    for lang in ${RELAY_LANGS}; do
        if relay_leg_available "${lang}"; then
            relay_assert_packaging "${lang}"
        else
            _defer "${lang} packaging — leg unavailable (image not built)"
        fi
    done
    # Cross-language identity: no two legs may share one image (anti-masquerade).
    relay_assert_images_distinct

    # --- Real-S3 precondition: the run's prefix starts EMPTY (fresh minio) ----
    # Anything found in the bucket after the pushes was provably written THIS run.
    echo "[relay] --- real-S3 precondition (minio genuinely empty before pushes) ---"
    relay_assert_store_prefix_empty

    # --- Choreography (locked §4 table) --------------------------------------
    echo "[relay] --- relay choreography ---"
    # 1  Node  push(seed)              -> ID_A
    _step_push      node  seed   "${ORACLE_ID_A}" seed  "step1 node push seed -> ID_A"
    # 2a Python pull(ID_A) re-id       == ID_A
    _step_pull_reid python "${ORACLE_ID_A}"            "step2a python pull ID_A re-id == ID_A"
    # 2b Python push(seed2)            -> ID_B
    _step_push      python seed2  "${ORACLE_ID_B}" seed2 "step2b python push seed2 -> ID_B"
    # 3  Go    pull(ID_B) re-id        == ID_B
    _step_pull_reid go    "${ORACLE_ID_B}"             "step3 go pull ID_B re-id == ID_B"
    # 4  Java  diff(ID_A, ID_B)        == DIFF_AB
    _step_diff      java                               "step4 java diff(ID_A,ID_B) == DIFF_AB"
    # 5  C++   pull(ID_A) re-id        == ID_A
    _step_pull_reid cpp   "${ORACLE_ID_A}"             "step5 cpp pull ID_A re-id == ID_A"
    # 6  Zig   pull(ID_B) re-id        == ID_B
    _step_pull_reid zig   "${ORACLE_ID_B}"             "step6 zig pull ID_B re-id == ID_B"

    # --- Real-S3 proof: minio genuinely holds both snapshots; no local s3:/ ----
    echo "[relay] --- real-S3 proof (objects in minio + no local fallback dir) ---"
    relay_assert_minio_populated
    relay_assert_no_local_s3_dir

    relay_summary
}

# ===========================================================================
# --selfcheck-oracle : the ONLY mode runnable before the apps exist. Builds the
# fixtures + computes ID_A/ID_B/DIFF_AB and prints them. Proves the oracle logic.
# ===========================================================================
integration_relay_selfcheck_oracle() {
    echo "[selfcheck-oracle] repo=${REPO_ROOT}"
    echo "[selfcheck-oracle] image=${BINDINGS_IMAGE}  work=${RELAY_WORK}"
    if ! relay_oracle_compute; then
        echo "[selfcheck-oracle] FAILED to compute oracle" >&2
        return 1
    fi
    echo ""
    echo "ID_A    = ${ORACLE_ID_A}"
    echo "ID_B    = ${ORACLE_ID_B}"
    echo "DIFF_AB (cat -vet, '^I' is the literal TAB, '\$' is the newline):"
    cat -vet "${ORACLE_DIFF_FILE}"
    echo "DIFF_AB (hexdump):"
    od -An -c "${ORACLE_DIFF_FILE}"
    echo ""
    echo "[selfcheck-oracle] OK — oracle logic sound; the FULL relay FAILS until the 6 apps exist (correct)."
    return 0
}

# ---------------------------------------------------------------------------
# Dispatch only when EXECUTED directly (never when sourced by run_relay.sh).
# ---------------------------------------------------------------------------
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    case "${1:-}" in
        --selfcheck-oracle) integration_relay_selfcheck_oracle ;;
        --run)              integration_relay_main ;;
        *)
            echo "usage: ${0##*/} {--selfcheck-oracle|--run}" >&2
            echo "  --selfcheck-oracle  build fixtures + compute ID_A/ID_B/DIFF_AB (runnable now)" >&2
            echo "  --run               run the full relay (requires the 6 apps; sourced by run_relay.sh)" >&2
            exit 2
            ;;
    esac
fi
