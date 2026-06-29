#!/usr/bin/env bash
# run_relay.sh — host-side cross-language integration relay orchestrator.
#
# Runs on the HOST (orchestrating docker via the host socket, like `act`).
# Does NOT run inside snapdir-bindings:dev.
#
# Usage:
#   bash tests/integration/run_relay.sh --smoke    # network + minio + bucket + SMOKE_OK
#   bash tests/integration/run_relay.sh            # full relay (all 6 languages)
#
# --smoke: bring up docker bridge network + minio service container, create the
#          snapdir-integ bucket, prove S3 reach end-to-end, then teardown.
#          Exits 0 on success.
#
# Full relay: stages packaged artifacts → builds 6 barebones app images →
# runs oracle → runs blank-slate packaging proofs → runs relay choreography →
# sources integration_assertions.sh and calls integration_relay_main.
#
# Conventions adapted from docker/scripts/sidecars-up.sh.

set -euo pipefail

# ── constants ─────────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

# Docker image that carries all toolchains + minio server binary + Python 3.
BINDINGS_IMAGE="snapdir-bindings:dev"

# Names that the teardown trap must know.
NETWORK_NAME="snapdir-relay"
MINIO_CONTAINER="snapdir-minio"

# Bucket used across all relay legs.
INTEG_BUCKET="snapdir-integ"

# Minio credentials (matching docker/scripts/sidecars.env).
MINIO_ROOT_USER="snapdir-test"
MINIO_ROOT_PASSWORD="snapdir-test-secret"
MINIO_PORT="9000"

# Relay store URI (shared across all language relay legs).
# Uses the minio S3 service container on the snapdir-relay docker network;
# all 6 language bindings push/pull through real S3 (snapdir-api dispatches
# s3:// to S3Store, reading creds from RELAY_S3_ENV at runtime).
# A unique per-run prefix avoids object collisions on re-runs.
RELAY_STORE="s3://snapdir-integ/relay-$$"

# Work dir for harness-owned fixtures + oracle scratch.
RELAY_WORK="${REPO_ROOT}/tests/integration/.relay-work"

# Staging dir for packaged artifacts (one sub-dir per language).
ARTIFACTS_DIR="${REPO_ROOT}/tests/integration/artifacts"

# App containers started by relay legs (populated during relay run).
APP_CONTAINERS=()

# Map: lang → 1 if the app image was built successfully.
declare -A APP_BUILT=()

# ── S3 environment block ──────────────────────────────────────────────────────
# Relay legs inherit this block.  All 6 language bindings read these env vars.
# "snapdir-minio" resolves via docker DNS on the snapdir-relay bridge network.
RELAY_S3_ENV=(
    -e "SNAPDIR_S3_STORE_ENDPOINT_URL=http://${MINIO_CONTAINER}:${MINIO_PORT}"
    -e "AWS_ACCESS_KEY_ID=${MINIO_ROOT_USER}"
    -e "AWS_SECRET_ACCESS_KEY=${MINIO_ROOT_PASSWORD}"
    -e "AWS_DEFAULT_REGION=us-east-1"
)

# ── helpers ───────────────────────────────────────────────────────────────────

log() { printf '[run_relay] %s\n' "$*"; }
die() { printf '[run_relay] ERROR: %s\n' "$*" >&2; exit 1; }

# ── teardown (EXIT trap) ──────────────────────────────────────────────────────
teardown() {
    log "--- teardown ---"
    docker rm -f "${MINIO_CONTAINER}" 2>/dev/null || true
    for c in "${APP_CONTAINERS[@]}"; do
        docker rm -f "${c}" 2>/dev/null || true
    done
    docker network rm "${NETWORK_NAME}" 2>/dev/null || true
    log "teardown complete"
}
trap teardown EXIT

# ── pre-clean (idempotent) ────────────────────────────────────────────────────
pre_clean() {
    log "pre-clean: removing stale containers/network from any prior run"
    docker rm -f "${MINIO_CONTAINER}" 2>/dev/null || true
    for c in "${APP_CONTAINERS[@]}"; do
        docker rm -f "${c}" 2>/dev/null || true
    done
    docker network rm "${NETWORK_NAME}" 2>/dev/null || true
    # Clear fixtures from any prior relay run.  relay_app containers write
    # root-owned files into RELAY_WORK/scratch; a host rm -rf can't remove
    # those.  Use a throwaway container (which runs as root) to wipe them
    # first, then the host rm removes the now-empty tree.
    if [[ -d "${RELAY_WORK}" ]]; then
        docker run --rm \
            -v "${RELAY_WORK}":/relay \
            "${BINDINGS_IMAGE}" \
            sh -c 'rm -rf /relay/*' 2>/dev/null || true
    fi
    rm -rf "${RELAY_WORK}" 2>/dev/null || true
}

# ── 1. create docker network ──────────────────────────────────────────────────
create_network() {
    log "creating bridge network ${NETWORK_NAME}"
    docker network create "${NETWORK_NAME}"
}

# ── 2. start minio as a NETWORK SERVICE container ────────────────────────────
# Reuses the baked minio binary in snapdir-bindings:dev — no image pull needed.
# Other containers on the network reach it as http://snapdir-minio:9000.
start_minio() {
    log "starting minio container (${MINIO_CONTAINER}) on ${NETWORK_NAME}"
    docker run -d \
        --network "${NETWORK_NAME}" \
        --name "${MINIO_CONTAINER}" \
        -e "MINIO_ROOT_USER=${MINIO_ROOT_USER}" \
        -e "MINIO_ROOT_PASSWORD=${MINIO_ROOT_PASSWORD}" \
        "${BINDINGS_IMAGE}" \
        sh -lc "mkdir -p /tmp/minio-data && exec minio server /tmp/minio-data --address :${MINIO_PORT}"
}

# ── 3. wait for minio health ──────────────────────────────────────────────────
# Polls from a throwaway container ON the network so the docker-DNS name resolves.
wait_minio_healthy() {
    local max=30
    local n=0
    log "waiting for minio health (max ${max}s, polled from within ${NETWORK_NAME})..."
    while [ "${n}" -lt "${max}" ]; do
        if docker run --rm \
                --network "${NETWORK_NAME}" \
                "${BINDINGS_IMAGE}" \
                sh -lc "curl -fsS http://${MINIO_CONTAINER}:${MINIO_PORT}/minio/health/live" \
                >/dev/null 2>&1; then
            log "minio /minio/health/live OK (attempt $((n+1)))"
            return 0
        fi
        n=$((n+1))
        sleep 1
    done
    die "minio not healthy after ${max}s"
}

# ── Python inline: AWS SigV4 S3 helper ───────────────────────────────────────
# Mirrors the bucket-creation fallback from tests/golden/run_parity.sh.
# Runs inside a throwaway container on the relay network so the minio hostname
# resolves via docker DNS.
#
# Usage: _run_s3_python <python_script_text> [extra_args...]
# The script receives: sys.argv[1]=endpoint sys.argv[2]=bucket sys.argv[3]=access sys.argv[4]=secret
# -i: attach stdin so python3 receives the heredoc script body.
_run_s3_python() {
    local script_body="$1"; shift
    docker run --rm -i \
        --network "${NETWORK_NAME}" \
        "${BINDINGS_IMAGE}" \
        python3 - \
            "http://${MINIO_CONTAINER}:${MINIO_PORT}" \
            "${INTEG_BUCKET}" \
            "${MINIO_ROOT_USER}" \
            "${MINIO_ROOT_PASSWORD}" \
            "$@" \
        <<EOF
${script_body}
EOF
}

# ── 4. create the relay bucket ────────────────────────────────────────────────
# Uses Python stdlib AWS SigV4 PUT — no mc/aws CLI required.
create_bucket() {
    log "creating bucket ${INTEG_BUCKET} (Python SigV4 PUT from ${NETWORK_NAME})"
    _run_s3_python '
import hashlib, hmac, datetime, urllib.request, sys
endpoint, bucket, access, secret = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
region = "us-east-1"; service = "s3"
now = datetime.datetime.utcnow()
date = now.strftime("%Y%m%d"); amz_dt = now.strftime("%Y%m%dT%H%M%SZ")
host = endpoint.split("//")[1]
payload_hash = hashlib.sha256(b"").hexdigest()
can_hdrs = f"host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_dt}\n"
signed_hdrs = "host;x-amz-content-sha256;x-amz-date"
can_req = f"PUT\n/{bucket}\n\n{can_hdrs}\n{signed_hdrs}\n{payload_hash}"
cred_scope = f"{date}/{region}/{service}/aws4_request"
sts = f"AWS4-HMAC-SHA256\n{amz_dt}\n{cred_scope}\n" + hashlib.sha256(can_req.encode()).hexdigest()
def sign(key, msg): return hmac.new(key, msg.encode(), hashlib.sha256).digest()
k = sign(sign(sign(sign(f"AWS4{secret}".encode(), date), region), service), "aws4_request")
sig = hmac.new(k, sts.encode(), hashlib.sha256).hexdigest()
auth = f"AWS4-HMAC-SHA256 Credential={access}/{cred_scope}, SignedHeaders={signed_hdrs}, Signature={sig}"
req = urllib.request.Request(f"{endpoint}/{bucket}", data=b"", method="PUT")
req.add_header("Authorization", auth); req.add_header("X-Amz-Date", amz_dt)
req.add_header("X-Amz-Content-Sha256", payload_hash)
try:
    urllib.request.urlopen(req); print(f"bucket {bucket}: created"); sys.exit(0)
except urllib.error.HTTPError as e:
    if e.code == 409: print(f"bucket {bucket}: already exists"); sys.exit(0)
    raise
'
}

# ── 5. smoke: prove S3 reach end-to-end ──────────────────────────────────────
# PUT a small file, GET it back to confirm the round-trip, print SMOKE_OK.
smoke_s3() {
    log "smoke: PUT + GET file via Python SigV4 on ${NETWORK_NAME}"
    _run_s3_python '
import hashlib, hmac, datetime, urllib.request, sys
endpoint, bucket, access, secret = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
region = "us-east-1"; service = "s3"

def sigv4_request(method, key, body=b""):
    now = datetime.datetime.utcnow()
    date = now.strftime("%Y%m%d"); amz_dt = now.strftime("%Y%m%dT%H%M%SZ")
    host = endpoint.split("//")[1]
    payload_hash = hashlib.sha256(body).hexdigest()
    can_hdrs = f"host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_dt}\n"
    signed_hdrs = "host;x-amz-content-sha256;x-amz-date"
    can_req = f"{method}\n/{bucket}/{key}\n\n{can_hdrs}\n{signed_hdrs}\n{payload_hash}"
    cred_scope = f"{date}/{region}/{service}/aws4_request"
    sts = f"AWS4-HMAC-SHA256\n{amz_dt}\n{cred_scope}\n" + hashlib.sha256(can_req.encode()).hexdigest()
    def sign(k, msg): return hmac.new(k, msg.encode(), hashlib.sha256).digest()
    k = sign(sign(sign(sign(f"AWS4{secret}".encode(), date), region), service), "aws4_request")
    sig = hmac.new(k, sts.encode(), hashlib.sha256).hexdigest()
    auth = f"AWS4-HMAC-SHA256 Credential={access}/{cred_scope}, SignedHeaders={signed_hdrs}, Signature={sig}"
    req = urllib.request.Request(f"{endpoint}/{bucket}/{key}", data=body if method=="PUT" else None, method=method)
    req.add_header("Authorization", auth); req.add_header("X-Amz-Date", amz_dt)
    req.add_header("X-Amz-Content-Sha256", payload_hash)
    return req

smoke_body = b"snapdir-relay-smoke"
urllib.request.urlopen(sigv4_request("PUT", "smoke", smoke_body))
print("  PUT smoke: OK")

got = urllib.request.urlopen(sigv4_request("GET", "smoke")).read()
assert got == smoke_body, f"smoke body mismatch: {got!r} != {smoke_body!r}"
print("  GET smoke: OK")
print("SMOKE_OK")
'
}

# ── 6. stage packaged artifacts ───────────────────────────────────────────────
# Builds and copies the 6 packaged language artifacts from the workspace into
# per-language staging directories.  Each later docker build uses its staging
# dir as the build context (via -f examples/<lang>/Dockerfile).
#
# Every artifact is rebuilt from the current HOST workspace source so that
# any snapdir-api changes (e.g. the api-multistore fix that routes s3:// to
# the real S3Store instead of FileStore) are compiled in.  The cargo
# incremental cache under target/ makes repeated runs fast.
stage_artifacts() {
    log "stage_artifacts: staging packaged artifacts from workspace"
    mkdir -p "${ARTIFACTS_DIR}"

    # ── 6a. rebuild snapdir-ffi (C ABI staticlib + cdylib) ───────────────────
    # Produces target/release/libsnapdir_ffi.{a,so} with all current source
    # changes compiled in.  Go/C++/Zig use the .a; Java uses the .so.
    log "  ffi: rebuilding snapdir-ffi (cargo --locked incremental; picks up api changes)"
    docker run --rm \
        -v "${REPO_ROOT}":/w -w /w \
        "${BINDINGS_IMAGE}" sh -lc '
            set -e
            cargo build --release -p snapdir-ffi --locked 2>&1 | tail -3
            echo "  ffi: libsnapdir_ffi.{a,so} rebuilt"
        '

    # ── node: npm build → npm pack → snapdir.tgz ─────────────────────────────
    local A_NODE="${ARTIFACTS_DIR}/node"
    mkdir -p "${A_NODE}"
    log "  node: rebuilding native addon + npm pack inside ${BINDINGS_IMAGE}"
    docker run --rm \
        -v "${REPO_ROOT}":/w -w /w \
        -v "${A_NODE}":/artifacts \
        "${BINDINGS_IMAGE}" sh -lc '
            set -e
            cd bindings/node
            # Always rebuild so the .node addon picks up current snapdir-api source.
            npm run build >/dev/null 2>&1
            npm pack --quiet 2>/dev/null
            mv snapdir-*.tgz /artifacts/snapdir.tgz
            echo "  node tgz staged"
        '
    cp "${REPO_ROOT}/examples/node/app.mjs" "${A_NODE}/"

    # ── python: rebuild manylinux wheel ──────────────────────────────────────
    local A_PYTHON="${ARTIFACTS_DIR}/python"
    mkdir -p "${A_PYTHON}"
    log "  python: rebuilding wheel inside ${BINDINGS_IMAGE}"
    docker run --rm \
        -v "${REPO_ROOT}":/w -w /w \
        "${BINDINGS_IMAGE}" sh -lc '
            set -e
            rm -f /w/target/wheels/snapdir-*.whl 2>/dev/null || true
            cd bindings/python && uv run maturin build --release 2>&1 | tail -2
        '
    local WHEEL
    WHEEL="$(ls "${REPO_ROOT}/target/wheels/snapdir-"*.whl 2>/dev/null | head -1)"
    [[ -n "${WHEEL}" ]] || die "python wheel not found after build attempt"
    cp "${WHEEL}" "${A_PYTHON}/"  # keep the original filename (pip requires it)
    cp "${REPO_ROOT}/examples/python/app.py" "${A_PYTHON}/"
    log "  python: staged $(basename "${WHEEL}")"

    # ── go: vendored module source + freshly built lib ────────────────────────
    local A_GO="${ARTIFACTS_DIR}/go"
    mkdir -p "${A_GO}/snapdir-go"
    [[ -f "${REPO_ROOT}/bindings/go/lib/libsnapdir_ffi.a" ]] \
        || die "go: bindings/go/lib/libsnapdir_ffi.a not found (run go-pack gate first)"
    # Copy the full go module directory (source + header); lib is overwritten below.
    rsync -a --delete \
        --exclude='*.test' \
        "${REPO_ROOT}/bindings/go/" "${A_GO}/snapdir-go/"
    # Replace the vendored lib with the freshly built one (picks up api-multistore fix).
    cp "${REPO_ROOT}/target/release/libsnapdir_ffi.a" "${A_GO}/snapdir-go/lib/libsnapdir_ffi.a"
    cp "${REPO_ROOT}/examples/go/app.go" "${A_GO}/"
    log "  go: staged bindings/go/ → snapdir-go/ (lib from target/release)"

    # ── cpp: headers + freshly built static lib ───────────────────────────────
    local A_CPP="${ARTIFACTS_DIR}/cpp"
    mkdir -p "${A_CPP}"
    [[ -f "${REPO_ROOT}/bindings/cpp/lib/libsnapdir_ffi.a" ]] \
        || die "cpp: bindings/cpp/lib/libsnapdir_ffi.a not found (run cpp-pack gate first)"
    cp "${REPO_ROOT}/bindings/cpp/include/snapdir.hpp"  "${A_CPP}/"
    cp "${REPO_ROOT}/bindings/cpp/include/snapdir.h"    "${A_CPP}/"
    # Replace with the freshly built lib (picks up api-multistore fix).
    cp "${REPO_ROOT}/target/release/libsnapdir_ffi.a"   "${A_CPP}/libsnapdir_ffi.a"
    cp "${REPO_ROOT}/examples/cpp/app.cpp" "${A_CPP}/"
    log "  cpp: staged headers + libsnapdir_ffi.a (from target/release)"

    # ── zig: binding source + freshly built lib ───────────────────────────────
    local A_ZIG="${ARTIFACTS_DIR}/zig"
    mkdir -p "${A_ZIG}"
    [[ -f "${REPO_ROOT}/bindings/zig/lib/libsnapdir_ffi.a" ]] \
        || die "zig: bindings/zig/lib/libsnapdir_ffi.a not found (run zig-pack gate first)"
    rsync -a --delete \
        "${REPO_ROOT}/bindings/zig/src/"      "${A_ZIG}/src/"
    rsync -a --delete \
        "${REPO_ROOT}/bindings/zig/include/"  "${A_ZIG}/include/"
    rsync -a --delete \
        "${REPO_ROOT}/bindings/zig/lib/"      "${A_ZIG}/lib/"
    # Replace the vendored lib with the freshly built one (picks up api-multistore fix).
    cp "${REPO_ROOT}/target/release/libsnapdir_ffi.a" "${A_ZIG}/lib/libsnapdir_ffi.a"
    cp "${REPO_ROOT}/examples/zig/app.zig"   "${A_ZIG}/"
    cp "${REPO_ROOT}/examples/zig/build.zig" "${A_ZIG}/"
    log "  zig: staged src/ include/ lib/ + app.zig + build.zig (lib from target/release)"

    # ── java: patch snapdir.jar with freshly built native .so ────────────────
    local A_JAVA="${ARTIFACTS_DIR}/java"
    mkdir -p "${A_JAVA}"
    [[ -f "${REPO_ROOT}/bindings/java/build/libs/snapdir.jar" ]] \
        || die "java: bindings/java/build/libs/snapdir.jar not found (run java-pack gate first)"
    # Extract the existing jar, replace the bundled .so with the freshly built
    # cdylib (picks up api-multistore fix), and repack into the staging area.
    log "  java: patching snapdir.jar with rebuilt libsnapdir_ffi.so"
    docker run --rm \
        -v "${REPO_ROOT}":/w \
        -v "${A_JAVA}":/artifacts \
        "${BINDINGS_IMAGE}" sh -lc '
            set -e
            mkdir -p /tmp/jar_work && cd /tmp/jar_work
            jar xf /w/bindings/java/build/libs/snapdir.jar
            cp /w/target/release/libsnapdir_ffi.so \
               native/linux-aarch64/libsnapdir_ffi.so
            jar cfm /artifacts/snapdir.jar META-INF/MANIFEST.MF -C /tmp/jar_work .
            echo "  java: jar patched (native/linux-aarch64/libsnapdir_ffi.so updated)"
        '
    cp "${REPO_ROOT}/examples/java/App.java" "${A_JAVA}/"
    log "  java: staged patched snapdir.jar + App.java"

    log "stage_artifacts: done"
}

# ── 7. build per-language example app images from barebones bases ─────────────
# Each Dockerfile starts from the language's official slim base (NOT
# snapdir-bindings:dev) and installs ONLY the staged artifact + its runtime.
# Build logs are saved to artifacts/<lang>/build.log for the credential scan.
build_app_images() {
    log "build_app_images: building 6 barebones example app images"

    _build_image() {
        local lang="$1" tag="snapdir-example-${1}"
        local artifacts_dir="${ARTIFACTS_DIR}/${lang}"
        local dockerfile="${REPO_ROOT}/examples/${lang}/Dockerfile"
        local log_file="${artifacts_dir}/build.log"

        log "  ${lang}: docker build -t ${tag} ..."
        if docker build \
                -t "${tag}" \
                -f "${dockerfile}" \
                "${artifacts_dir}" \
                >"${log_file}" 2>&1; then
            APP_BUILT["${lang}"]=1
            log "  ${lang}: image ${tag} built OK"
        else
            log "  WARN: ${lang} image build FAILED — leg will be DEFERRED (see ${log_file})"
        fi
    }

    # Build images sequentially (shared cargo target lock; one at a time).
    _build_image node
    _build_image python
    _build_image go
    _build_image cpp
    _build_image zig
    _build_image java

    local built_count=0
    for lang in node python go java cpp zig; do
        [[ -n "${APP_BUILT[${lang}]:-}" ]] && built_count=$((built_count + 1))
    done
    log "build_app_images: ${built_count}/6 images built"
}

# ── relay hook implementations ────────────────────────────────────────────────

# relay_app <lang> <cmd> [args...]
# Run lang's app image once; print only the app's stdout; return exit code.
relay_app() {
    local lang="$1"; shift
    docker run --rm \
        --network "${NETWORK_NAME}" \
        -v "${RELAY_WORK}:/relay" \
        "${RELAY_S3_ENV[@]}" \
        "snapdir-example-${lang}" \
        "$@"
}

# relay_image <lang> → prints the built image tag for lang.
relay_image() {
    echo "snapdir-example-${1}"
}

# relay_leg_available <lang> → exit 0 if the image is built and usable.
relay_leg_available() {
    [[ -n "${APP_BUILT[${1}]:-}" ]]
}

# relay_build_log <lang> → prints the path to lang's docker build log (host file).
relay_build_log() {
    echo "${ARTIFACTS_DIR}/${1}/build.log"
}

# ── smoke mode ────────────────────────────────────────────────────────────────
run_smoke() {
    log "=== SMOKE MODE ==="
    pre_clean
    create_network
    start_minio
    wait_minio_healthy
    create_bucket
    smoke_s3
    log "=== SMOKE PASS ==="
}

# ── full relay mode ───────────────────────────────────────────────────────────
run_full_relay() {
    log "=== FULL RELAY MODE ==="
    pre_clean
    create_network
    start_minio
    wait_minio_healthy
    create_bucket
    smoke_s3
    stage_artifacts
    build_app_images

    # Prepare the relay work dir (fixtures are created inside relay_oracle_compute).
    # The store is S3 (minio); /relay is only a scratch vol for fixtures + pull dests.
    mkdir -p "${RELAY_WORK}"

    # Export parameters the assertions file reads.
    export RELAY_REPO_ROOT="${REPO_ROOT}"
    export REPO_ROOT
    export RELAY_WORK
    export BINDINGS_IMAGE
    export RELAY_DEV_IMAGE="${BINDINGS_IMAGE}"
    export NETWORK_NAME
    export RELAY_STORE

    # Source the adversary's assertion suite (defines oracle, choreography,
    # packaging assertions, and a default relay_leg_available that returns 1).
    # Our relay_leg_available override (defined above) wins because bash resolves
    # function names at call time — the later definition replaces the sourced one.
    # shellcheck disable=SC1091
    source "${SCRIPT_DIR}/integration_assertions.sh"

    # Re-define relay_leg_available after source to override the always-DEFER default.
    relay_leg_available() {
        [[ -n "${APP_BUILT[${1}]:-}" ]]
    }

    # Sanity check: confirm the relay network is still alive before invoking the
    # relay.  The packaging assertions run many docker containers on the host's
    # default bridge; if Docker Desktop reclaims the custom bridge in between,
    # this gives a clear diagnostic instead of an opaque rc=125 from the
    # first relay_s3_list_keys call.
    if ! docker network inspect "${NETWORK_NAME}" >/dev/null 2>&1; then
        die "network ${NETWORK_NAME} lost before integration_relay_main (Docker Desktop instability?)"
    fi
    log "relay network ${NETWORK_NAME} healthy before relay"

    # Run the relay: oracle + packaging proof + choreography + byte-exact assertions.
    if ! integration_relay_main; then
        die "relay FAILED — see output above"
    fi

    log "=== FULL RELAY PASS ==="
}

# ── entrypoint ────────────────────────────────────────────────────────────────
MODE="${1:-}"

case "${MODE}" in
    --smoke)
        run_smoke
        ;;
    "")
        run_full_relay
        ;;
    *)
        die "unknown mode: ${MODE}  (use --smoke or omit for full relay)"
        ;;
esac
