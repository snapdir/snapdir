#!/usr/bin/env bash
# tests/golden/gen_fixtures.sh
#
# Deterministically materializes all 8 golden fixtures and captures
# expected/<fixture>.manifest + expected/<fixture>.id from the pinned
# oracle (the 1.10.0 `snapdir` binary built from the frozen workspace source).
#
# DESIGN: Phase 36, gate fixtures-generate.
# SPEC:   .gatesmith/reviews/fixtures-corpus.md (LOCKED D1-D4).
# ORACLE: the workspace `snapdir` CLI binary (target/debug/snapdir or
#         target/release/snapdir) — identical to the 1.10.0 oracle because
#         `snapdir-api` and `snapdir-ffi` are purely additive (the manifest
#         format is FROZEN by manifest-format.sha.lock).
#
# DETERMINISM: fixed names, fixed contents (seed-derived via sha256sum),
#   LC_ALL=C sort, no $RANDOM, no dates, no mtimes in the manifest.
#   A deterministic tree → a byte-stable manifest (the format records
#   path/type/perms/checksum/size — NO mtimes).
#
# SETUID FALLBACK (D4 LOCKED): after chmod u+s / g+s on the permissions
#   fixture, stat to check whether the bit actually stuck (some container /
#   CI mounts strip setuid). If stripped, fall back to recording WITHOUT the
#   high bit for ONLY those two files; sticky + dangling/escaping remain
#   unconditional. A note is printed to stderr.
#
# RUN-TO-RUN IDEMPOTENCY: the work/ dir is cleaned and recreated on each run.
#   Re-running yields byte-identical expected/*.
#
# VERIFICATION (PM runs exactly):
#   tests/golden/gen_fixtures.sh && \
#     ls tests/golden/expected/*.manifest tests/golden/expected/*.id
#
# What is NOT committed:
#   tests/golden/work/     — the scratch tree (gitignored)
#   The 10k large-tree files inside work/large-tree/

set -euo pipefail
LC_ALL=C
export LC_ALL

# ---------------------------------------------------------------------------
# Paths (all relative to the workspace root, which is CWD when run in-image).
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
WORK_DIR="${SCRIPT_DIR}/work"
EXPECTED_DIR="${SCRIPT_DIR}/expected"

# Seed for deterministic large-tree generation (LOCKED D2).
SNAPDIR_GOLDEN_SEED="20260624"

# ---------------------------------------------------------------------------
# Oracle binary — prefer release over debug if both exist; build if missing.
# ---------------------------------------------------------------------------
locate_or_build_oracle() {
    local release_bin="${WORKSPACE_ROOT}/target/release/snapdir"
    local debug_bin="${WORKSPACE_ROOT}/target/debug/snapdir"

    if [[ -x "${release_bin}" ]]; then
        echo "${release_bin}"
        return
    fi
    if [[ -x "${debug_bin}" ]]; then
        echo "${debug_bin}"
        return
    fi

    echo "[gen_fixtures] Oracle not found — building (cargo build -p snapdir --locked)..." >&2
    (cd "${WORKSPACE_ROOT}" && cargo build -p snapdir --locked) >&2
    if [[ ! -x "${debug_bin}" ]]; then
        echo "[gen_fixtures] ERROR: build succeeded but ${debug_bin} not found" >&2
        exit 1
    fi
    echo "${debug_bin}"
}

ORACLE="$(locate_or_build_oracle)"
echo "[gen_fixtures] Oracle: ${ORACLE}" >&2
echo "[gen_fixtures] Oracle version: $("${ORACLE}" --version 2>&1 || true)" >&2

# ---------------------------------------------------------------------------
# Oracle invocation helpers (env-clean: remove store-routing variables per
# the m0 suite precedent in crates/snapdir-api/tests/m0_golden_parity.rs).
# ---------------------------------------------------------------------------
oracle_manifest() {
    # Usage: oracle_manifest [extra args] <path>
    env -u SNAPDIR_STORE -u SNAPDIR_OBJECTS_STORE -u SNAPDIR_MANIFEST_CONTEXT \
        "${ORACLE}" manifest "$@"
}

oracle_id_from_stdin() {
    # Pipe a manifest text into `snapdir id` (reads manifest from stdin when
    # not a TTY). Returns 64 lowercase hex. Strips trailing newline.
    env -u SNAPDIR_STORE -u SNAPDIR_OBJECTS_STORE -u SNAPDIR_MANIFEST_CONTEXT \
        "${ORACLE}" id
}

capture_fixture() {
    # Usage: capture_fixture <fixture_name> <fixture_dir> [extra_manifest_args...]
    # Writes expected/<name>.manifest and expected/<name>.id.
    local name="$1"
    local fixture_dir="$2"
    shift 2
    local extra_args=("$@")

    echo "[gen_fixtures] Capturing ${name}..." >&2

    local manifest_text
    manifest_text="$(oracle_manifest "${extra_args[@]}" "${fixture_dir}")"

    local snapshot_id
    snapshot_id="$(printf '%s' "${manifest_text}" | oracle_id_from_stdin)"
    snapshot_id="${snapshot_id%$'\n'}"  # strip trailing newline

    # Verbatim manifest — trailing newline is part of the byte contract.
    printf '%s\n' "${manifest_text}" > "${EXPECTED_DIR}/${name}.manifest"
    printf '%s\n' "${snapshot_id}"   > "${EXPECTED_DIR}/${name}.id"

    echo "[gen_fixtures]   ${name}.id = ${snapshot_id}" >&2
}

# ---------------------------------------------------------------------------
# Setup: clean work dir, ensure expected dir.
# ---------------------------------------------------------------------------
echo "[gen_fixtures] Cleaning work dir ${WORK_DIR}..." >&2
rm -rf "${WORK_DIR}"
mkdir -p "${WORK_DIR}"
mkdir -p "${EXPECTED_DIR}"

# ===========================================================================
# FIXTURE 1: empty
# ===========================================================================
# One empty directory, dir mode 0755.
# Pins: the degenerate manifest = the lone D ./ root line; id = BLAKE3 of
#       that one line.
# ===========================================================================
FIXTURE_EMPTY="${WORK_DIR}/empty"
mkdir -p "${FIXTURE_EMPTY}"
chmod 0755 "${FIXTURE_EMPTY}"

capture_fixture "empty" "${FIXTURE_EMPTY}"

# ===========================================================================
# FIXTURE 2: single-file
# ===========================================================================
# a.txt = "hello", file 0644, dir 0755.
# Pins: single F-line (TYPE PERM CHECKSUM SIZE PATH) + root D-line merkle.
# ===========================================================================
FIXTURE_SINGLE="${WORK_DIR}/single-file"
mkdir -p "${FIXTURE_SINGLE}"
printf 'hello' > "${FIXTURE_SINGLE}/a.txt"
chmod 0644 "${FIXTURE_SINGLE}/a.txt"
chmod 0755 "${FIXTURE_SINGLE}"

capture_fixture "single-file" "${FIXTURE_SINGLE}"

# ===========================================================================
# FIXTURE 3: nested
# ===========================================================================
# 8 levels deep (l1/.../l8) + sibling files at several depths + duplicate
# content interleaved (mirrors parity_deep_nesting_eight_levels_many_siblings).
# Pins: recursive merkle, sort -k5 path ordering, dedup at multiple depths.
# ===========================================================================
FIXTURE_NESTED="${WORK_DIR}/nested"
DEEP="${FIXTURE_NESTED}/l1/l2/l3/l4/l5/l6/l7/l8"
mkdir -p "${DEEP}"

# Files at various depths
printf 'bottom' > "${DEEP}/bottom.txt"
printf 'shared-bytes\n' > "${FIXTURE_NESTED}/l1/l2/l3/dup.txt"
printf 'shared-bytes\n' > "${FIXTURE_NESTED}/l1/l2/dup.txt"
printf 'a' > "${FIXTURE_NESTED}/l1/a.txt"
printf 'z' > "${FIXTURE_NESTED}/l1/z.txt"
printf 'top' > "${FIXTURE_NESTED}/top.txt"

# Set deterministic perms on files
for f in \
    "l1/l2/l3/l4/l5/l6/l7/l8/bottom.txt" \
    "l1/l2/l3/dup.txt" \
    "l1/l2/dup.txt" \
    "l1/a.txt" \
    "l1/z.txt" \
    "top.txt"; do
    chmod 0644 "${FIXTURE_NESTED}/${f}"
done

# Set deterministic perms on dirs
for d in \
    "l1" \
    "l1/l2" \
    "l1/l2/l3" \
    "l1/l2/l3/l4" \
    "l1/l2/l3/l4/l5" \
    "l1/l2/l3/l4/l5/l6" \
    "l1/l2/l3/l4/l5/l6/l7" \
    "l1/l2/l3/l4/l5/l6/l7/l8"; do
    chmod 0755 "${FIXTURE_NESTED}/${d}"
done
chmod 0755 "${FIXTURE_NESTED}"

capture_fixture "nested" "${FIXTURE_NESTED}"

# ===========================================================================
# FIXTURE 4: unicode-paths
# ===========================================================================
# Filenames covering NFC/NFD/RTL/emoji/space/collation names.
# CRITICAL: content = the filename's own bytes (reproducible, distinct per file).
# All files 0644, dir 0755.
# Pins: PATH column survives byte-for-byte, sort -k5 ordering over multibyte bytes.
#
# Use Python 3 to create the files so that non-ASCII bytes are correctly placed
# into the filesystem (bash does not expand \xNN in paths, only in printf content).
# ===========================================================================
FIXTURE_UNICODE="${WORK_DIR}/unicode-paths"
mkdir -p "${FIXTURE_UNICODE}"

export FIXTURE_UNICODE
python3 - <<'PYTHON_EOF'
import os
import sys

ROOT = os.environ.get("FIXTURE_UNICODE", "")
if not ROOT:
    print("ERROR: FIXTURE_UNICODE env not set", file=sys.stderr)
    sys.exit(1)

# Each tuple: (filename_bytes, content_bytes)
# Content = the filename bytes themselves (deterministic, distinct per file).
# All filenames are literal UTF-8 bytes (or raw bytes for NFD, RTL, emoji, etc.).
files = [
    # NFC café (U+00E9 = precomposed e-acute: \xc3\xa9)
    (b"caf\xc3\xa9.txt",        b"caf\xc3\xa9.txt"),
    # NFD café (e + combining acute U+0301: e\xcc\x81 — distinct from NFC)
    (b"cafe\xcc\x81.txt",       b"cafe\xcc\x81.txt"),
    # Space in name
    (b"with space.txt",          b"with space.txt"),
    # Trailing space in name
    (b"trailing.space .txt",     b"trailing.space .txt"),
    # Symbols
    (b"sym+bol&(name).bin",      b"sym+bol&(name).bin"),
    # Emoji (crab U+1F980: \xf0\x9f\xa6\x80)
    (b"emoji-\xf0\x9f\xa6\x80.rs", b"emoji-\xf0\x9f\xa6\x80.rs"),
    # Cyrillic: naïve (n a ï=\xc3\xaf v e) + space + файл (Cyrillic f a j l)
    (b"na\xc3\xaf\xd0\xb2\xd0\xb5 \xd1\x84\xd0\xb0\xd0\xb9\xd0\xbb.dat",
     b"na\xc3\xaf\xd0\xb2\xd0\xb5 \xd1\x84\xd0\xb0\xd0\xb9\xd0\xbb.dat"),
    # RTL Hebrew: עברית = \xd7\xa2\xd7\x91\xd7\xa8\xd7\x99\xd7\xaa
    (b"\xd7\xa2\xd7\x91\xd7\xa8\xd7\x99\xd7\xaa.txt",
     b"\xd7\xa2\xd7\x91\xd7\xa8\xd7\x99\xd7\xaa.txt"),
    # Line separator U+2028 in name (\xe2\x80\xa8)
    (b"tabless\xe2\x80\xa8line.txt", b"tabless\xe2\x80\xa8line.txt"),
    # Leading dot (dotfile)
    (b".hidden",                 b".hidden"),
    # Leading dash
    (b"-leading-dash.txt",       b"-leading-dash.txt"),
    # Double dash
    (b"--double-dash",           b"--double-dash"),
    # Mixed-case collation
    (b"Zebra.TXT",               b"Zebra.TXT"),
    (b"apple.txt",               b"apple.txt"),
    # Digit-vs-letter ordering
    (b"2-two.txt",               b"2-two.txt"),
    (b"10-ten.txt",              b"10-ten.txt"),
]

for filename_bytes, content_bytes in files:
    fpath = os.path.join(ROOT.encode(), filename_bytes)
    with open(fpath, "wb") as fh:
        fh.write(content_bytes)
    os.chmod(fpath, 0o644)

os.chmod(ROOT, 0o755)
print(f"[gen_fixtures] unicode-paths: {len(files)} files created", file=sys.stderr)
PYTHON_EOF

capture_fixture "unicode-paths" "${FIXTURE_UNICODE}"

# ===========================================================================
# FIXTURE 5: symlinks
# ===========================================================================
# realdir/inner.txt; target.txt; link-to-file → target.txt;
# link-to-dir → realdir; hop-a → hop-b → realdir/inner.txt (chain);
# broken → ./nonexistent (dangling); escape → ../../etc/hostname (escaping).
#
# Captured twice:
#   symlinks-follow.manifest / symlinks-follow.id   (default, follow)
#   symlinks-nofollow.manifest / symlinks-nofollow.id  (--no-follow)
# ===========================================================================
FIXTURE_SYMLINKS="${WORK_DIR}/symlinks"
mkdir -p "${FIXTURE_SYMLINKS}/realdir"

printf 'inner' > "${FIXTURE_SYMLINKS}/realdir/inner.txt"
printf 'target-bytes' > "${FIXTURE_SYMLINKS}/target.txt"

# Symlink to file
ln -sf "target.txt" "${FIXTURE_SYMLINKS}/link-to-file"
# Symlink to dir
ln -sf "realdir" "${FIXTURE_SYMLINKS}/link-to-dir"
# Chain: hop-a → hop-b → realdir/inner.txt
ln -sf "hop-b" "${FIXTURE_SYMLINKS}/hop-a"
ln -sf "realdir/inner.txt" "${FIXTURE_SYMLINKS}/hop-b"
# Dangling link
ln -sf "./nonexistent" "${FIXTURE_SYMLINKS}/broken"
# Escaping symlink (relative, points outside fixture root)
ln -sf "../../etc/hostname" "${FIXTURE_SYMLINKS}/escape"

chmod 0644 "${FIXTURE_SYMLINKS}/realdir/inner.txt"
chmod 0644 "${FIXTURE_SYMLINKS}/target.txt"
chmod 0755 "${FIXTURE_SYMLINKS}/realdir"
chmod 0755 "${FIXTURE_SYMLINKS}"

# Capture follow (default) variant
capture_fixture "symlinks-follow" "${FIXTURE_SYMLINKS}"

# Capture no-follow variant
capture_fixture "symlinks-nofollow" "${FIXTURE_SYMLINKS}" "--no-follow"

# ===========================================================================
# FIXTURE 6: identical-content
# ===========================================================================
# a.txt=b.txt=c.txt=d.txt="same-content\n" (0644) + z-different.txt="unique\n"
# Pins: dedup — duplicate F lines still appear but dir merkle sort -u collapses
#       duplicate child checksums in the root D-line.
# ===========================================================================
FIXTURE_IDENTICAL="${WORK_DIR}/identical-content"
mkdir -p "${FIXTURE_IDENTICAL}"

for name in a.txt b.txt c.txt d.txt; do
    printf 'same-content\n' > "${FIXTURE_IDENTICAL}/${name}"
    chmod 0644 "${FIXTURE_IDENTICAL}/${name}"
done
printf 'unique\n' > "${FIXTURE_IDENTICAL}/z-different.txt"
chmod 0644 "${FIXTURE_IDENTICAL}/z-different.txt"
chmod 0755 "${FIXTURE_IDENTICAL}"

capture_fixture "identical-content" "${FIXTURE_IDENTICAL}"

# ===========================================================================
# FIXTURE 7: large-tree (10,000 entries, seed 20260624)
# ===========================================================================
# Deterministic nested structure; content = sha256(seed || relpath) truncated
# to 32 bytes (gives each file a distinct deterministic checksum). NO randomness,
# NO timestamps. Only gen_fixtures.sh + expected/large-tree.{manifest,id} committed.
# ===========================================================================
echo "[gen_fixtures] Generating large-tree (10,000 entries, seed=${SNAPDIR_GOLDEN_SEED})..." >&2
FIXTURE_LARGE="${WORK_DIR}/large-tree"
mkdir -p "${FIXTURE_LARGE}"

# Generate the tree using Python 3 (available in-image) for speed + portability.
# Uses hashlib.sha256 (stdlib; blake3 Python module not in the image) as the
# deterministic PRF. The ORACLE (snapdir) independently computes BLAKE3 of each
# file's content — what matters is that the content is deterministic per path.
# Export FIXTURE_LARGE so the Python script can read it from the environment.
export FIXTURE_LARGE
python3 - <<'PYTHON_EOF'
import hashlib
import os
import sys

SEED = b"20260624"
ROOT = os.environ.get("FIXTURE_LARGE", "")
if not ROOT:
    print("ERROR: FIXTURE_LARGE env not set", file=sys.stderr)
    sys.exit(1)

TARGET_ENTRIES = 10000

# Deterministic tree layout:
#   256 top-level dirs (two hex digits: 00..ff)
#   Each top-level dir contains ~39 subdirs (aa..zz, selected by hash) +
#   files to hit the total.
# Entry numbering is index-based so the layout is fully deterministic.

def content_for_relpath(relpath: str) -> bytes:
    """32 bytes of sha256(SEED || relpath.encode())."""
    h = hashlib.sha256(SEED + relpath.encode("utf-8")).digest()
    return h  # 32 bytes, distinct per relpath

# Build exactly 10000 entries (files only; dirs are counted separately below).
# Layout: 200 dirs of 50 files each = 10000 files + 200 dirs = 10200 entries.
# We want ≥10000 total entries (files+dirs). Use 40 top-level dirs × 10 subdirs
# × 25 files/subdir = 10000 files + 40+400=440 dirs = 10440 total — but we want
# exactly 10000 entries (as in the spec "10,000 entries").
#
# Simplified: 100 dirs × 100 files = 10000 files + 100 dirs = 10100 total.
# The spec says "≥10,000 entries" (large-tree comment §2). We target 10000 files
# exactly (dirs are extra) so total entries ≥ 10000.
#
# To be exact about entry count semantics (spec says 10k, D2 says "10,000 entries"):
# We produce 10000 file entries + the dirs they live in. This matches the spec intent.

DIRS_COUNT = 100
FILES_PER_DIR = 100  # 100 × 100 = 10,000 files

for dir_idx in range(DIRS_COUNT):
    # Deterministic dir name: two hex digits
    dir_name = f"{dir_idx:02x}"
    dir_path = os.path.join(ROOT, dir_name)
    os.makedirs(dir_path, exist_ok=True)
    os.chmod(dir_path, 0o755)

    for file_idx in range(FILES_PER_DIR):
        # Deterministic file name
        file_name = f"f{file_idx:03d}.dat"
        relpath = f"{dir_name}/{file_name}"
        file_path = os.path.join(ROOT, relpath)
        content = content_for_relpath(relpath)
        with open(file_path, "wb") as fh:
            fh.write(content)
        os.chmod(file_path, 0o644)

os.chmod(ROOT, 0o755)
print(f"[gen_fixtures] large-tree: {DIRS_COUNT * FILES_PER_DIR} files in {DIRS_COUNT} dirs", file=sys.stderr)
PYTHON_EOF

capture_fixture "large-tree" "${FIXTURE_LARGE}"

# ===========================================================================
# FIXTURE 8: permissions
# ===========================================================================
# readable.txt 0644, private.txt 0600, world-read.txt 0444, executable.sh 0755,
# lockeddir/ 0700 with inner 0600,
# setuid.bin 04755 + setgid.bin 02755 + sticky/ 01777.
#
# D4 LOCKED — SETUID FALLBACK:
# After chmod u+s / g+s, stat to check if the bit stuck (some container mounts
# strip setuid). If stripped, fall back WITHOUT the high bit for those files only
# (print a clear note to stderr), keeping the fixture deterministic. Sticky and
# dangling/escaping remain unconditional.
# ===========================================================================
FIXTURE_PERMS="${WORK_DIR}/permissions"
mkdir -p "${FIXTURE_PERMS}/lockeddir"
mkdir -p "${FIXTURE_PERMS}/sticky"

printf 'r' > "${FIXTURE_PERMS}/readable.txt"
printf 'p' > "${FIXTURE_PERMS}/private.txt"
printf 'w' > "${FIXTURE_PERMS}/world-read.txt"
printf '#!/bin/sh\n' > "${FIXTURE_PERMS}/executable.sh"
printf 'i' > "${FIXTURE_PERMS}/lockeddir/inner"
# setuid / setgid candidates
printf 'setuid-content' > "${FIXTURE_PERMS}/setuid.bin"
printf 'setgid-content' > "${FIXTURE_PERMS}/setgid.bin"

chmod 0644 "${FIXTURE_PERMS}/readable.txt"
chmod 0600 "${FIXTURE_PERMS}/private.txt"
chmod 0444 "${FIXTURE_PERMS}/world-read.txt"
chmod 0755 "${FIXTURE_PERMS}/executable.sh"
chmod 0600 "${FIXTURE_PERMS}/lockeddir/inner"
chmod 0700 "${FIXTURE_PERMS}/lockeddir"
chmod 01777 "${FIXTURE_PERMS}/sticky"

# Attempt setuid on setuid.bin (04755)
chmod 04755 "${FIXTURE_PERMS}/setuid.bin"
SETUID_ACTUAL=$(stat -c '%a' "${FIXTURE_PERMS}/setuid.bin" 2>/dev/null || \
                stat -f '%Mp%Lp' "${FIXTURE_PERMS}/setuid.bin" 2>/dev/null || echo "unknown")
# Normalize: strip leading zeros for comparison but check high bit presence.
# On GNU stat: 4755 (setuid present); on BSD stat: 4755 or 755.
if printf '%s' "${SETUID_ACTUAL}" | grep -qE '^[0-9]*4[0-9]{3}$|^4[0-9]{3}$'; then
    echo "[gen_fixtures] setuid.bin: setuid bit SET (mode=${SETUID_ACTUAL}) — capturing with 04755" >&2
    SETUID_STUCK=true
else
    echo "[gen_fixtures] NOTE (D4 fallback): setuid bit STRIPPED on this mount (stat=${SETUID_ACTUAL})." >&2
    echo "[gen_fixtures]   Recording setuid.bin without the 04 high bit — using 0755 instead." >&2
    chmod 0755 "${FIXTURE_PERMS}/setuid.bin"
    SETUID_STUCK=false
fi

# Attempt setgid on setgid.bin (02755)
chmod 02755 "${FIXTURE_PERMS}/setgid.bin"
SETGID_ACTUAL=$(stat -c '%a' "${FIXTURE_PERMS}/setgid.bin" 2>/dev/null || \
                stat -f '%Mp%Lp' "${FIXTURE_PERMS}/setgid.bin" 2>/dev/null || echo "unknown")
if printf '%s' "${SETGID_ACTUAL}" | grep -qE '^[0-9]*2[0-9]{3}$|^2[0-9]{3}$'; then
    echo "[gen_fixtures] setgid.bin: setgid bit SET (mode=${SETGID_ACTUAL}) — capturing with 02755" >&2
    SETGID_STUCK=true
else
    echo "[gen_fixtures] NOTE (D4 fallback): setgid bit STRIPPED on this mount (stat=${SETGID_ACTUAL})." >&2
    echo "[gen_fixtures]   Recording setgid.bin without the 02 high bit — using 0755 instead." >&2
    chmod 0755 "${FIXTURE_PERMS}/setgid.bin"
    SETGID_STUCK=false
fi

# sticky is unconditional — sticky directories are always retained.
chmod 0755 "${FIXTURE_PERMS}"

capture_fixture "permissions" "${FIXTURE_PERMS}"

echo "[gen_fixtures] setuid_stuck=${SETUID_STUCK} setgid_stuck=${SETGID_STUCK}" >&2

# ===========================================================================
# Summary
# ===========================================================================
echo "" >&2
echo "[gen_fixtures] Done. Expected files:" >&2
ls -1 "${EXPECTED_DIR}/" >&2
echo "" >&2
echo "[gen_fixtures] Verifying all expected files exist..." >&2

EXPECTED_FILES=(
    "empty.manifest"          "empty.id"
    "single-file.manifest"    "single-file.id"
    "nested.manifest"         "nested.id"
    "unicode-paths.manifest"  "unicode-paths.id"
    "symlinks-follow.manifest" "symlinks-follow.id"
    "symlinks-nofollow.manifest" "symlinks-nofollow.id"
    "identical-content.manifest" "identical-content.id"
    "large-tree.manifest"     "large-tree.id"
    "permissions.manifest"    "permissions.id"
)

ALL_OK=true
for f in "${EXPECTED_FILES[@]}"; do
    if [[ ! -f "${EXPECTED_DIR}/${f}" ]]; then
        echo "[gen_fixtures] ERROR: missing ${f}" >&2
        ALL_OK=false
    fi
done

if [[ "${ALL_OK}" == "true" ]]; then
    echo "[gen_fixtures] All 20 expected files present. SUCCESS." >&2
else
    echo "[gen_fixtures] Some expected files are missing. FAIL." >&2
    exit 1
fi
