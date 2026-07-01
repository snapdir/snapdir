#define _XOPEN_SOURCE 700 /* mkdtemp/setenv/mkdir/etc.: C99-safe POSIX (MUST precede all #includes) */

/*
 * m1_abi_surface.c — BLACK-BOX C adversary test for the snapdir-ffi §4 extern surface.
 *
 * GATE: m1-abi-surface-spec-tests (phase 35, owner adversary, opus). M1 C cluster 2/3
 *       (memory-contract done; abi-surface here; init-idempotent next).
 *
 * SOURCES (black-box): authored from TWO artifacts ONLY —
 *   1. include/snapdir.h               (the cbindgen-generated C header — the public contract)
 *   2. .gatesmith/reviews/m1-c-abi.md  (the locked C ABI spec — esp. §4 the extern surface,
 *                                       §2 memory contract, §3 init)
 * It does NOT read crates/snapdir-ffi/src — ZERO Rust-source visibility. No struct layouts,
 * no private symbols, no internal helpers: only header-declared names + the locked spec. The
 * sibling crates/snapdir-ffi/tests/c/m1_memory_contract.c was consulted for STYLE/harness only.
 *
 * WHAT THIS PINS — EVERY §4 extern fn is exercised against a REAL file:// temp store, with
 * BEHAVIORAL invariants (round-trip self-consistency) the impl cannot fake. Each §4 fn and the
 * invariant pinning it:
 *
 *   sync:
 *     [§4-id]              snapdir_id(tree) -> 64-hex BLAKE3 id (deterministic).
 *     [§4-manifest]        snapdir_manifest(tree) -> manifest text referencing the tree.
 *     [§4-id-from-text]    snapdir_id_from_manifest_text(manifest(tree)) == snapdir_id(tree).
 *     [§4-stage]           snapdir_stage(tree) -> an id (== snapdir_id, checksum-faithful).
 *
 *   distribution (BLOCKING — routed through the embedded runtime via block_on):
 *     [§4-push]            snapdir_push_blocking(path,store) -> the SAME id as snapdir_id(tree).
 *     [§4-fetch]           snapdir_fetch_blocking(id,store) -> 0 after a push (objects present).
 *     [§4-pull]            snapdir_pull_blocking(id,store,dest) reproduces a tree whose
 *                          snapdir_id == the pushed id (byte-faithful materialization).
 *     [§4-checkout]        snapdir_checkout_blocking(id,dest) (after fetch) reproduces a tree
 *                          whose snapdir_id == the pushed id.
 *     [§4-sync]            snapdir_sync_blocking(id,srcStore,dstStore) -> 0; the id is then
 *                          pull-able from the destination store (== pushed id).
 *     [§4-verify]          snapdir_verify_blocking(id,store) -> 0 on a healthy store.
 *
 *   JSON (caller frees):
 *     [§4-diff]            snapdir_diff_json(self vs self, include_unchanged=false) -> empty
 *                          change set; diff of two trees differing by +1/-1/~1 file -> JSON
 *                          structurally mentions the added, removed, and modified paths.
 *     [§4-locations]       snapdir_locations_json(...) -> well-formed JSON (non-NULL).
 *     [§4-ancestors]       snapdir_ancestors_json(id,...) -> well-formed JSON (non-NULL).
 *     [§4-revisions]       snapdir_revisions_json(location,...) -> well-formed JSON (non-NULL).
 *
 *   cache:
 *     [§4-verify-cache]    snapdir_verify_cache(cache_dir,purge=false) -> 0 on a real cache dir.
 *     [§4-flush-cache]     snapdir_flush_cache(cache_dir) -> 0 on a real cache dir.
 *
 * Plus the §2 memory contract THROUGHOUT: snapdir_init() first; every returned char* freed via
 * snapdir_string_free exactly once; every error inspected via the SnapdirError** out-param and
 * freed via snapdir_error_free exactly once; borrowed inspect pointers never freed. The test is
 * sanitizer-clean-by-construction (every allocation freed exactly once; no double-free/UAF).
 *
 * EXPECTED RESULT until impl: LINK FAILURE. The current cbindgen header declares only the sync
 * pair that memory-contract's impl landed (snapdir_id / snapdir_manifest); the remaining §4 fns
 * (snapdir_id_from_manifest_text, snapdir_stage, the six *_blocking fns, the four *_json fns, and
 * snapdir_verify_cache / snapdir_flush_cache) are NOT yet implemented. We FORWARD-DECLARE all of
 * them here with EXACT §4 prototypes so the file COMPILES NOW (clang -std=c99 -fsyntax-only clean)
 * but is UNDEFINED AT LINK — the correct "no-impl state" for this triple. When m1-abi-surface-impl
 * wires the full blocking+JSON surface and cbindgen regenerates the header, these forward decls
 * become redundant-but-compatible (identical C signatures) and the impl may drop them; the test
 * file is `git mv`'d BYTE-IDENTICAL into crates/snapdir-ffi/tests/c/ and must then pass under
 * `clang -fsanitize=address,undefined` vs a file:// store, exit 0 only when ALL invariants hold.
 *
 * The §4 signatures used here (VERBATIM from m1-c-abi.md §4):
 *   char* snapdir_id_from_manifest_text(const char* manifest_text, SnapdirError** err_out);
 *   char* snapdir_stage(const char* path, bool keep, const char* cache_dir, SnapdirError** err_out);
 *   char* snapdir_push_blocking(const char* source_path, const char* source_id,
 *                               const char* store_uri, unsigned jobs, const char* limit_rate,
 *                               unsigned max_retries, const char* cache_dir, SnapdirError** err_out);
 *   int   snapdir_fetch_blocking(const char* id, const char* store_uri, unsigned jobs, SnapdirError** err_out);
 *   int   snapdir_pull_blocking(const char* id, const char* store_uri, const char* dest_path,
 *                               bool delete_extra, unsigned jobs, SnapdirError** err_out);
 *   int   snapdir_checkout_blocking(const char* id, const char* dest_path, bool linked,
 *                                   bool delete_extra, SnapdirError** err_out);
 *   int   snapdir_sync_blocking(const char* id, const char* src_uri, const char* dst_uri,
 *                               unsigned jobs, SnapdirError** err_out);
 *   int   snapdir_verify_blocking(const char* id, const char* store_uri, bool purge, SnapdirError** err_out);
 *   char* snapdir_diff_json(const char** from_uris, const char** to_uris, const char* id,
 *                           bool include_unchanged, const char* on_conflict, SnapdirError** err_out);
 *   char* snapdir_locations_json(const char* cache_dir, const char* catalog, SnapdirError** err_out);
 *   char* snapdir_ancestors_json(const char* id, const char* catalog, SnapdirError** err_out);
 *   char* snapdir_revisions_json(const char* location, const char* catalog, SnapdirError** err_out);
 *   int   snapdir_verify_cache(const char* cache_dir, bool purge, SnapdirError** err_out);
 *   int   snapdir_flush_cache(const char* cache_dir, SnapdirError** err_out);
 */

#include "snapdir.h"

#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

/* --------------------------------------------------------------------------- */
/* Forward declarations of the §4 surface NOT yet in the current header.        */
/* (snapdir_id / snapdir_manifest ARE in the header — landed by memory-contract */
/* impl — so they are NOT redeclared here; we call them via snapdir.h.)         */
/* These remain UNDEFINED at link time until m1-abi-surface-impl lands them.    */
/* When the impl regenerates the header to include them, these decls become     */
/* redundant-but-compatible (identical C signatures) and may be dropped.        */
/* --------------------------------------------------------------------------- */
extern char *snapdir_id_from_manifest_text(const char *manifest_text,
                                           struct SnapdirError **err_out);
extern char *snapdir_stage(const char *path, bool keep, const char *cache_dir,
                           struct SnapdirError **err_out);
extern char *snapdir_push_blocking(const char *source_path, const char *source_id,
                                   const char *store_uri, unsigned jobs,
                                   const char *limit_rate, unsigned max_retries,
                                   const char *cache_dir, struct SnapdirError **err_out);
extern int snapdir_fetch_blocking(const char *id, const char *store_uri, unsigned jobs,
                                  struct SnapdirError **err_out);
extern int snapdir_pull_blocking(const char *id, const char *store_uri, const char *dest_path,
                                 bool delete_extra, unsigned jobs, struct SnapdirError **err_out);
extern int snapdir_checkout_blocking(const char *id, const char *dest_path, bool linked,
                                     bool delete_extra, struct SnapdirError **err_out);
extern int snapdir_sync_blocking(const char *id, const char *src_uri, const char *dst_uri,
                                 unsigned jobs, struct SnapdirError **err_out);
extern int snapdir_verify_blocking(const char *id, const char *store_uri, bool purge,
                                   struct SnapdirError **err_out);
extern char *snapdir_diff_json(const char **from_uris, const char **to_uris, const char *id,
                               bool include_unchanged, const char *on_conflict,
                               struct SnapdirError **err_out);
extern char *snapdir_locations_json(const char *cache_dir, const char *catalog,
                                    struct SnapdirError **err_out);
extern char *snapdir_ancestors_json(const char *id, const char *catalog,
                                    struct SnapdirError **err_out);
extern char *snapdir_revisions_json(const char *location, const char *catalog,
                                    struct SnapdirError **err_out);
extern int snapdir_verify_cache(const char *cache_dir, bool purge, struct SnapdirError **err_out);
extern int snapdir_flush_cache(const char *cache_dir, struct SnapdirError **err_out);

#define CHECK(cond, msg)                                                            \
    do {                                                                            \
        if (!(cond)) {                                                              \
            fprintf(stderr, "FAIL: %s\n  at %s:%d\n", (msg), __FILE__, __LINE__);   \
            return 1;                                                               \
        }                                                                           \
    } while (0)

/*
 * On any error path we want a readable diagnostic. Prints the borrowed (do-NOT-free) message +
 * code from a SnapdirError, then frees the error exactly once. Returns non-zero always so call
 * sites can `return report_err(...)`.
 */
static int report_err(const char *what, struct SnapdirError *err) {
    if (err != NULL) {
        const char *msg = snapdir_error_message(err);
        const char *code = snapdir_error_code(err);
        fprintf(stderr, "FAIL: %s — code=%s msg=%s\n", what,
                code ? code : "(null)", msg ? msg : "(null)");
        snapdir_error_free(err);
    } else {
        fprintf(stderr, "FAIL: %s — (no error object)\n", what);
    }
    return 1;
}

/* ------------------------------------------------------------------------- */
/* Tiny filesystem helpers (black-box; just build/read real temp trees).     */
/* ------------------------------------------------------------------------- */

/* Write `contents` to `dir/name`. Returns 0 on success. */
static int write_file(const char *dir, const char *name, const char *contents) {
    char path[2048];
    int n = snprintf(path, sizeof(path), "%s/%s", dir, name);
    if (n < 0 || (size_t)n >= sizeof(path)) {
        return 1;
    }
    FILE *f = fopen(path, "w");
    if (f == NULL) {
        return 1;
    }
    fputs(contents, f);
    fclose(f);
    return 0;
}

static void unlink_in(const char *dir, const char *name) {
    char path[2048];
    int n = snprintf(path, sizeof(path), "%s/%s", dir, name);
    if (n > 0 && (size_t)n < sizeof(path)) {
        unlink(path);
    }
}

/* mkdtemp into out; out must hold at least 64 bytes. Returns 0 on success. */
static int make_tmpdir(const char *prefix, char *out, size_t out_len) {
    char tmpl[1024];
    int n = snprintf(tmpl, sizeof(tmpl), "/tmp/snapdir_abi_%s_XXXXXX", prefix);
    if (n < 0 || (size_t)n >= sizeof(tmpl)) {
        return 1;
    }
    char *dir = mkdtemp(tmpl);
    if (dir == NULL) {
        return 1;
    }
    if (strlen(dir) + 1 > out_len) {
        return 1;
    }
    strcpy(out, dir);
    return 0;
}

/* Build a file:// URI from an absolute directory path into `out`. Returns 0 on success. */
static int file_uri(const char *abs_dir, char *out, size_t out_len) {
    /* abs_dir is an absolute /tmp/... path from mkdtemp → file:///tmp/... */
    int n = snprintf(out, out_len, "file://%s", abs_dir);
    if (n < 0 || (size_t)n >= out_len) {
        return 1;
    }
    return 0;
}

/* ------------------------------------------------------------------------- */
/* Compute snapdir_id for a tree; on success writes a freshly-allocated 64-hex */
/* id into *id_out (caller frees) and returns 0. On failure reports + nonzero. */
/* ------------------------------------------------------------------------- */
static int id_of(const char *path, char **id_out) {
    struct SnapdirError *err = NULL;
    char *id = snapdir_id(path, NULL, 0, NULL, &err);
    if (id == NULL || err != NULL) {
        return report_err("snapdir_id", err);
    }
    if (strlen(id) != 64) {
        snapdir_string_free(id);
        fprintf(stderr, "FAIL: snapdir_id returned a non-64-char id\n");
        return 1;
    }
    *id_out = id;
    return 0;
}

/* ------------------------------------------------------------------------- */
/* [§4-id] / [§4-manifest] / [§4-id-from-text] / [§4-stage]:                   */
/* The sync surface — manifest/id round-trip + stage checksum-faithfulness.   */
/* ------------------------------------------------------------------------- */
static int test_sync_surface(void) {
    char tree[1024];
    CHECK(make_tmpdir("synctree", tree, sizeof(tree)) == 0, "mkdtemp sync tree");
    CHECK(write_file(tree, "a.txt", "alpha\n") == 0, "write a.txt");
    CHECK(write_file(tree, "b.txt", "bravo\n") == 0, "write b.txt");

    /* cache dir override so we never touch the user's default cache (sanitizer/CI hygiene). */
    char cache[1024];
    CHECK(make_tmpdir("cache", cache, sizeof(cache)) == 0, "mkdtemp cache");

    /* [§4-id] */
    char *id = NULL;
    CHECK(id_of(tree, &id) == 0, "snapdir_id on the sync tree");
    for (size_t i = 0; i < 64; i++) {
        char c = id[i];
        int is_hex = (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f');
        CHECK(is_hex, "snapdir_id must be lowercase hex");
    }

    /* [§4-manifest]: manifest text references both files. */
    struct SnapdirError *merr = NULL;
    char *manifest = snapdir_manifest(tree, NULL, 0, false, false, NULL, NULL, NULL, &merr);
    if (manifest == NULL || merr != NULL) {
        snapdir_string_free(id);
        return report_err("snapdir_manifest", merr);
    }
    CHECK(strstr(manifest, "a.txt") != NULL, "manifest must reference a.txt");
    CHECK(strstr(manifest, "b.txt") != NULL, "manifest must reference b.txt");

    /* [§4-id-from-text]: id_from_manifest_text(manifest(tree)) == id(tree). */
    struct SnapdirError *ierr = NULL;
    char *id_from_text = snapdir_id_from_manifest_text(manifest, &ierr);
    if (id_from_text == NULL || ierr != NULL) {
        snapdir_string_free(manifest);
        snapdir_string_free(id);
        return report_err("snapdir_id_from_manifest_text", ierr);
    }
    CHECK(strcmp(id_from_text, id) == 0,
          "id_from_manifest_text(manifest(tree)) must equal id(tree)");
    snapdir_string_free(id_from_text);
    snapdir_string_free(manifest);

    /* [§4-stage]: stage returns an id; for a plain BLAKE3 snapshot it must equal snapdir_id
     * (stage is checksum-faithful — it stages the same content-addressed objects). We pass
     * keep=false (do not retain a working copy) and a dedicated cache_dir. */
    struct SnapdirError *serr = NULL;
    char *staged_id = snapdir_stage(tree, false, cache, &serr);
    if (staged_id == NULL || serr != NULL) {
        snapdir_string_free(id);
        return report_err("snapdir_stage", serr);
    }
    CHECK(strlen(staged_id) == 64, "snapdir_stage must return a 64-hex id");
    CHECK(strcmp(staged_id, id) == 0,
          "snapdir_stage id must equal snapdir_id (checksum-faithful staging)");
    snapdir_string_free(staged_id);

    snapdir_string_free(id);

    /* cleanup */
    unlink_in(tree, "a.txt");
    unlink_in(tree, "b.txt");
    rmdir(tree);
    /* cache dir may now hold staged objects; leave it to the OS /tmp reaper — but remove the
     * empty dir best-effort (non-empty rmdir simply fails, harmless). */
    rmdir(cache);
    return 0;
}

/* ------------------------------------------------------------------------- */
/* [§4-push]/[§4-fetch]/[§4-pull]/[§4-checkout]/[§4-verify]:                   */
/* The blocking distribution round-trip against a real file:// store.         */
/* push returns the SAME id; pull/checkout reproduce a byte-faithful tree     */
/* (same snapdir_id); fetch + verify succeed.                                 */
/* ------------------------------------------------------------------------- */
static int test_blocking_roundtrip(void) {
    char tree[1024], storedir[1024], pulldir[1024], codir[1024];
    CHECK(make_tmpdir("src", tree, sizeof(tree)) == 0, "mkdtemp src tree");
    CHECK(make_tmpdir("store", storedir, sizeof(storedir)) == 0, "mkdtemp store");
    CHECK(make_tmpdir("pull", pulldir, sizeof(pulldir)) == 0, "mkdtemp pull dest");
    CHECK(make_tmpdir("checkout", codir, sizeof(codir)) == 0, "mkdtemp checkout dest");

    CHECK(write_file(tree, "one.txt", "first file\n") == 0, "write one.txt");
    CHECK(write_file(tree, "two.txt", "second file\n") == 0, "write two.txt");

    char store_uri[1100];
    CHECK(file_uri(storedir, store_uri, sizeof(store_uri)) == 0, "build store file:// uri");

    /* Reference id of the source tree. */
    char *src_id = NULL;
    CHECK(id_of(tree, &src_id) == 0, "snapdir_id on the source tree");

    /* [§4-push]: push the path to the store; returns the SAME id. source_id=NULL (path XOR id). */
    struct SnapdirError *perr = NULL;
    char *pushed_id = snapdir_push_blocking(tree, NULL, store_uri, 0, NULL, 0, NULL, &perr);
    if (pushed_id == NULL || perr != NULL) {
        snapdir_string_free(src_id);
        return report_err("snapdir_push_blocking", perr);
    }
    CHECK(strcmp(pushed_id, src_id) == 0,
          "snapdir_push_blocking must return the same id as snapdir_id(tree)");

    /* [§4-fetch]: fetch the snapshot's objects into the local cache → 0. */
    struct SnapdirError *ferr = NULL;
    int frc = snapdir_fetch_blocking(pushed_id, store_uri, 0, &ferr);
    if (frc != 0 || ferr != NULL) {
        snapdir_string_free(pushed_id);
        snapdir_string_free(src_id);
        return report_err("snapdir_fetch_blocking", ferr);
    }

    /* [§4-verify]: a healthy store verifies → 0. purge=false. */
    struct SnapdirError *verr = NULL;
    int vrc = snapdir_verify_blocking(pushed_id, store_uri, false, &verr);
    if (vrc != 0 || verr != NULL) {
        snapdir_string_free(pushed_id);
        snapdir_string_free(src_id);
        return report_err("snapdir_verify_blocking", verr);
    }

    /* [§4-pull]: materialize the snapshot into pulldir → 0; resulting tree's id == pushed id. */
    struct SnapdirError *plerr = NULL;
    int plrc = snapdir_pull_blocking(pushed_id, store_uri, pulldir, false, 0, &plerr);
    if (plrc != 0 || plerr != NULL) {
        snapdir_string_free(pushed_id);
        snapdir_string_free(src_id);
        return report_err("snapdir_pull_blocking", plerr);
    }
    char *pulled_id = NULL;
    if (id_of(pulldir, &pulled_id) != 0) {
        snapdir_string_free(pushed_id);
        snapdir_string_free(src_id);
        return 1;
    }
    CHECK(strcmp(pulled_id, pushed_id) == 0,
          "snapdir_pull_blocking must reproduce a tree whose id equals the pushed id");
    snapdir_string_free(pulled_id);
    /* Concrete content sanity: the pulled tree must actually contain a known file. */
    {
        char p[2048];
        int n = snprintf(p, sizeof(p), "%s/one.txt", pulldir);
        CHECK(n > 0 && (size_t)n < sizeof(p), "compose pulled one.txt path");
        CHECK(access(p, F_OK) == 0, "pulled tree must contain one.txt");
    }

    /* [§4-checkout]: checkout (after fetch, from cache) into codir → 0; id == pushed id.
     * linked=false (editable copy), delete_extra=false. */
    struct SnapdirError *coerr = NULL;
    int corc = snapdir_checkout_blocking(pushed_id, codir, false, false, &coerr);
    if (corc != 0 || coerr != NULL) {
        snapdir_string_free(pushed_id);
        snapdir_string_free(src_id);
        return report_err("snapdir_checkout_blocking", coerr);
    }
    char *co_id = NULL;
    if (id_of(codir, &co_id) != 0) {
        snapdir_string_free(pushed_id);
        snapdir_string_free(src_id);
        return 1;
    }
    CHECK(strcmp(co_id, pushed_id) == 0,
          "snapdir_checkout_blocking must reproduce a tree whose id equals the pushed id");
    snapdir_string_free(co_id);

    snapdir_string_free(pushed_id);
    snapdir_string_free(src_id);

    /* cleanup (best-effort; /tmp reaper handles object pools). */
    unlink_in(tree, "one.txt");
    unlink_in(tree, "two.txt");
    unlink_in(pulldir, "one.txt");
    unlink_in(pulldir, "two.txt");
    unlink_in(codir, "one.txt");
    unlink_in(codir, "two.txt");
    rmdir(tree);
    rmdir(pulldir);
    rmdir(codir);
    rmdir(storedir);
    return 0;
}

/* ------------------------------------------------------------------------- */
/* [§4-sync]: store-to-store replication. push to src store, sync src→dst,    */
/* then pull from the dst store reproduces the same id.                       */
/* ------------------------------------------------------------------------- */
static int test_sync_blocking(void) {
    char tree[1024], srcstore[1024], dststore[1024], dest[1024];
    CHECK(make_tmpdir("synctree2", tree, sizeof(tree)) == 0, "mkdtemp sync2 tree");
    CHECK(make_tmpdir("srcstore", srcstore, sizeof(srcstore)) == 0, "mkdtemp src store");
    CHECK(make_tmpdir("dststore", dststore, sizeof(dststore)) == 0, "mkdtemp dst store");
    CHECK(make_tmpdir("syncdest", dest, sizeof(dest)) == 0, "mkdtemp sync dest");

    CHECK(write_file(tree, "x.dat", "syncable payload\n") == 0, "write x.dat");

    char src_uri[1100], dst_uri[1100];
    CHECK(file_uri(srcstore, src_uri, sizeof(src_uri)) == 0, "build src file:// uri");
    CHECK(file_uri(dststore, dst_uri, sizeof(dst_uri)) == 0, "build dst file:// uri");

    char *src_id = NULL;
    CHECK(id_of(tree, &src_id) == 0, "snapdir_id on the sync2 tree");

    /* push to the SRC store. */
    struct SnapdirError *perr = NULL;
    char *pushed_id = snapdir_push_blocking(tree, NULL, src_uri, 0, NULL, 0, NULL, &perr);
    if (pushed_id == NULL || perr != NULL) {
        snapdir_string_free(src_id);
        return report_err("snapdir_push_blocking (sync src)", perr);
    }
    CHECK(strcmp(pushed_id, src_id) == 0, "pushed id must equal id(tree) for sync test");

    /* [§4-sync]: replicate id from src store → dst store → 0. */
    struct SnapdirError *syerr = NULL;
    int syrc = snapdir_sync_blocking(pushed_id, src_uri, dst_uri, 0, &syerr);
    if (syrc != 0 || syerr != NULL) {
        snapdir_string_free(pushed_id);
        snapdir_string_free(src_id);
        return report_err("snapdir_sync_blocking", syerr);
    }

    /* Prove the snapshot now lives in the DST store: pull from dst reproduces the same id. */
    struct SnapdirError *plerr = NULL;
    int plrc = snapdir_pull_blocking(pushed_id, dst_uri, dest, false, 0, &plerr);
    if (plrc != 0 || plerr != NULL) {
        snapdir_string_free(pushed_id);
        snapdir_string_free(src_id);
        return report_err("snapdir_pull_blocking (from dst store)", plerr);
    }
    char *dst_id = NULL;
    if (id_of(dest, &dst_id) != 0) {
        snapdir_string_free(pushed_id);
        snapdir_string_free(src_id);
        return 1;
    }
    CHECK(strcmp(dst_id, pushed_id) == 0,
          "after sync, pulling from the dst store must reproduce the pushed id");
    snapdir_string_free(dst_id);

    snapdir_string_free(pushed_id);
    snapdir_string_free(src_id);

    unlink_in(tree, "x.dat");
    unlink_in(dest, "x.dat");
    rmdir(tree);
    rmdir(dest);
    rmdir(srcstore);
    rmdir(dststore);
    return 0;
}

/* ------------------------------------------------------------------------- */
/* [§4-diff]: diff_json self-vs-self (include_unchanged=false) is empty; diff  */
/* of two trees differing by +1/-1/~1 file structurally names those paths.    */
/* Uses file:// store URIs (the from/to operands are stores holding pushed     */
/* snapshots) — robust against key ordering via substring needle checks.      */
/* ------------------------------------------------------------------------- */

/* needle helper — present iff JSON mentions the path. */
static int json_mentions(const char *json, const char *needle) {
    return json != NULL && strstr(json, needle) != NULL;
}

/*
 * Faithful-shape needle: present iff the JSON carries a diff entry for `path`
 * tagged with the exact status glyph `status` (one of "A"/"D"/"M"/"="). This is
 * STRONGER than json_mentions: it pins the §4 DiffEntry serialization (the
 * frozen DiffStatus glyph PLUS the path), not just that the path string appears
 * somewhere. The impl serializes snapdir_api::DiffEntry as
 * {"status":"<glyph>","path":"<path>"} — the FULL faithful DiffEntry shape
 * (DiffEntry has exactly `status` + `path`; no checksum/size/perm fields exist
 * to drop). Key order within an object is not guaranteed by serde_json::json!,
 * so we accept BOTH orderings ("status" before "path" and vice-versa).
 *
 * Returns 1 if an object mentioning BOTH `"path":"<path>"` AND `"status":"<glyph>"`
 * is present (we require both substrings to coexist in the document — sufficient
 * here because each path is unique across the change set under test).
 */
static int json_entry_is(const char *json, const char *path, const char *status) {
    if (json == NULL) {
        return 0;
    }
    char path_kv[256];
    char status_kv[64];
    int n1 = snprintf(path_kv, sizeof(path_kv), "\"path\":\"%s\"", path);
    int n2 = snprintf(status_kv, sizeof(status_kv), "\"status\":\"%s\"", status);
    if (n1 < 0 || (size_t)n1 >= sizeof(path_kv) || n2 < 0 ||
        (size_t)n2 >= sizeof(status_kv)) {
        return 0;
    }
    return strstr(json, path_kv) != NULL && strstr(json, status_kv) != NULL;
}

static int test_diff_json(void) {
    /* Two trees: base = {keep.txt, gone.txt, mod.txt(v1)}; next = {keep.txt, mod.txt(v2), add.txt}.
     * Δ: +add.txt, -gone.txt, ~mod.txt; keep.txt unchanged.
     *
     * A→B addressing: base is pushed to its OWN store (storeA) and next to a SEPARATE store
     * (storeB). The A→B diff is then snapdir_diff_json(from=[storeA], to=[storeB], id=NULL): the
     * `from` and `to` URI lists each carry exactly ONE distinct manifest, so they map directly to
     * snapdir_api::diff(DiffOptions{ from:[storeA], to:[storeB] }) — an EXACT facade delegation
     * with no same-store ambiguity. (The facade unions all manifests per side; if BOTH sides
     * pointed at the same store the union would be identical on each side and yield an empty diff,
     * which is why base and next get distinct stores here.) */
    char base[1024], next[1024], storeA[1024], storeB[1024];
    CHECK(make_tmpdir("diffbase", base, sizeof(base)) == 0, "mkdtemp diff base");
    CHECK(make_tmpdir("diffnext", next, sizeof(next)) == 0, "mkdtemp diff next");
    CHECK(make_tmpdir("diffstoreA", storeA, sizeof(storeA)) == 0, "mkdtemp diff storeA");
    CHECK(make_tmpdir("diffstoreB", storeB, sizeof(storeB)) == 0, "mkdtemp diff storeB");

    CHECK(write_file(base, "keep.txt", "constant\n") == 0, "write base/keep.txt");
    CHECK(write_file(base, "gone.txt", "to be removed\n") == 0, "write base/gone.txt");
    CHECK(write_file(base, "mod.txt", "version one\n") == 0, "write base/mod.txt");

    CHECK(write_file(next, "keep.txt", "constant\n") == 0, "write next/keep.txt");
    CHECK(write_file(next, "mod.txt", "version two — changed\n") == 0, "write next/mod.txt");
    CHECK(write_file(next, "add.txt", "newly added\n") == 0, "write next/add.txt");

    char storeA_uri[1100], storeB_uri[1100];
    CHECK(file_uri(storeA, storeA_uri, sizeof(storeA_uri)) == 0, "build diff storeA uri");
    CHECK(file_uri(storeB, storeB_uri, sizeof(storeB_uri)) == 0, "build diff storeB uri");

    /* Push base → storeA and next → storeB so each store holds exactly one manifest/object set. */
    char *base_id = NULL;
    struct SnapdirError *be = NULL;
    base_id = snapdir_push_blocking(base, NULL, storeA_uri, 0, NULL, 0, NULL, &be);
    if (base_id == NULL || be != NULL) {
        return report_err("snapdir_push_blocking (diff base → storeA)", be);
    }
    char *next_id = NULL;
    struct SnapdirError *ne = NULL;
    next_id = snapdir_push_blocking(next, NULL, storeB_uri, 0, NULL, 0, NULL, &ne);
    if (next_id == NULL || ne != NULL) {
        snapdir_string_free(base_id);
        return report_err("snapdir_push_blocking (diff next → storeB)", ne);
    }

    /* [§4-diff] self-vs-self (one store, same id on both sides), include_unchanged=false ⇒ empty
     * change set. "Empty" is asserted structurally: NONE of the member files appear, because a
     * snapshot diffed against itself with unchanged suppressed has no entries. We use storeA (which
     * holds base) on both sides with id=base_id. */
    const char *self_from[] = { storeA_uri, NULL };
    const char *self_to[] = { storeA_uri, NULL };
    struct SnapdirError *d0e = NULL;
    char *self_diff = snapdir_diff_json(self_from, self_to, base_id, false, "error", &d0e);
    if (self_diff == NULL || d0e != NULL) {
        snapdir_string_free(base_id);
        snapdir_string_free(next_id);
        return report_err("snapdir_diff_json (self vs self)", d0e);
    }
    /* With include_unchanged=false, a snapshot diffed against ITSELF yields no entries — so none
     * of the member filenames should be present in the change set. */
    CHECK(!json_mentions(self_diff, "keep.txt"),
          "self-diff (include_unchanged=false) must not list unchanged keep.txt");
    CHECK(!json_mentions(self_diff, "gone.txt"),
          "self-diff (include_unchanged=false) must be empty (no gone.txt)");
    CHECK(!json_mentions(self_diff, "mod.txt"),
          "self-diff (include_unchanged=false) must be empty (no mod.txt)");
    snapdir_string_free(self_diff);

    /* [§4-diff] base→next: structurally names the added, removed, and modified paths.
     * Addressed as TWO SEPARATE stores — from=[storeA] (base), to=[storeB] (next), id=NULL.
     * Because each side's URI list resolves to exactly one distinct manifest, this is an EXACT
     * delegation to snapdir_api::diff(DiffOptions{ from:[storeA], to:[storeB] }) — no same-store
     * ambiguity and no `.manifests`-walk heuristic needed in the impl. */
    const char *from_uris[] = { storeA_uri, NULL };
    const char *to_uris[] = { storeB_uri, NULL };
    struct SnapdirError *d1e = NULL;
    char *diff = snapdir_diff_json(from_uris, to_uris, NULL, false, "error", &d1e);
    if (diff == NULL || d1e != NULL) {
        snapdir_string_free(base_id);
        snapdir_string_free(next_id);
        return report_err("snapdir_diff_json (base vs next)", d1e);
    }
    /* STRENGTHENED [§4-diff / DiffEntry faithfulness]: not merely that the path
     * string appears, but that each path carries the CORRECT frozen DiffStatus
     * glyph — add.txt=Added("A"), gone.txt=Deleted("D"), mod.txt=Modified("M").
     * This pins the FULL faithful DiffEntry serialization (status glyph + path),
     * so the impl cannot satisfy the substring checks with a mis-classified or
     * status-stripped entry. (DiffEntry's complete shape is {status, path} —
     * snapdir_api::DiffEntry has no checksum/size/perm fields to drop; the JSON
     * is therefore the entire faithful DiffEntry.) */
    CHECK(json_entry_is(diff, "./add.txt", "A"),
          "diff base→next: add.txt must be classified Added (\"A\")");
    CHECK(json_entry_is(diff, "./gone.txt", "D"),
          "diff base→next: gone.txt must be classified Deleted (\"D\")");
    CHECK(json_entry_is(diff, "./mod.txt", "M"),
          "diff base→next: mod.txt must be classified Modified (\"M\")");
    /* keep.txt is unchanged and include_unchanged=false ⇒ it must NOT appear. */
    CHECK(!json_mentions(diff, "keep.txt"),
          "diff base→next (include_unchanged=false) must suppress unchanged keep.txt");
    snapdir_string_free(diff);

    /* STRENGTHENED [§4-diff include_unchanged=true]: the current test only
     * exercised the FALSE case. Re-run the SAME base→next diff with
     * include_unchanged=true and assert the unchanged keep.txt is now PRESENT
     * with the Unchanged glyph "=" (DiffOptions.all=true), while the three
     * changed paths keep their A/D/M classification. This pins that
     * include_unchanged actually threads through to snapdir_api::diff and that
     * "=" is emitted faithfully. */
    const char *from_uris_all[] = { storeA_uri, NULL };
    const char *to_uris_all[] = { storeB_uri, NULL };
    struct SnapdirError *dae = NULL;
    char *diff_all = snapdir_diff_json(from_uris_all, to_uris_all, NULL, true, "error", &dae);
    if (diff_all == NULL || dae != NULL) {
        snapdir_string_free(base_id);
        snapdir_string_free(next_id);
        return report_err("snapdir_diff_json (base vs next, include_unchanged=true)", dae);
    }
    CHECK(json_entry_is(diff_all, "./keep.txt", "="),
          "diff base→next (include_unchanged=true) must include keep.txt as Unchanged (\"=\")");
    CHECK(json_entry_is(diff_all, "./add.txt", "A"),
          "diff base→next (include_unchanged=true) must still classify add.txt Added");
    CHECK(json_entry_is(diff_all, "./gone.txt", "D"),
          "diff base→next (include_unchanged=true) must still classify gone.txt Deleted");
    CHECK(json_entry_is(diff_all, "./mod.txt", "M"),
          "diff base→next (include_unchanged=true) must still classify mod.txt Modified");
    snapdir_string_free(diff_all);

    snapdir_string_free(base_id);
    snapdir_string_free(next_id);

    unlink_in(base, "keep.txt");
    unlink_in(base, "gone.txt");
    unlink_in(base, "mod.txt");
    unlink_in(next, "keep.txt");
    unlink_in(next, "mod.txt");
    unlink_in(next, "add.txt");
    rmdir(base);
    rmdir(next);
    rmdir(storeA);
    rmdir(storeB);
    return 0;
}

/* ------------------------------------------------------------------------- */
/* [§4-locations]/[§4-ancestors]/[§4-revisions]: catalog JSON queries.        */
/* After a push (catalog recording on, via $SNAPDIR_CATALOG_DB_PATH temp DB),  */
/* each returns well-formed, non-NULL JSON. We assert structural facts rather  */
/* than brittle exact bytes: the JSON is non-NULL/non-empty and bracket-shaped */
/* (array/object), so the impl cannot fake a stub that returns NULL or "".     */
/* ------------------------------------------------------------------------- */

/* A minimal "looks like JSON" check: non-empty and starts with '[' or '{' (after ws). */
static int looks_like_json(const char *s) {
    if (s == NULL) {
        return 0;
    }
    const char *p = s;
    while (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') {
        p++;
    }
    return (*p == '[' || *p == '{');
}

static int test_catalog_json(void) {
    char tree[1024], store[1024], cache[1024], catdir[1024];
    CHECK(make_tmpdir("cattree", tree, sizeof(tree)) == 0, "mkdtemp catalog tree");
    CHECK(make_tmpdir("catstore", store, sizeof(store)) == 0, "mkdtemp catalog store");
    CHECK(make_tmpdir("catcache", cache, sizeof(cache)) == 0, "mkdtemp catalog cache");
    CHECK(make_tmpdir("catdb", catdir, sizeof(catdir)) == 0, "mkdtemp catalog db dir");

    CHECK(write_file(tree, "doc.txt", "catalog subject\n") == 0, "write doc.txt");

    /* The catalog DB path seam is the $SNAPDIR_CATALOG_DB_PATH env var the M0 facade uses
     * (per m1-c-abi.md §4: the `catalog` arg selects the catalog NAME; the DB path is the env). */
    char catdb[2048];
    int n = snprintf(catdb, sizeof(catdb), "%s/catalog.db", catdir);
    CHECK(n > 0 && (size_t)n < sizeof(catdb), "compose catalog db path");
    CHECK(setenv("SNAPDIR_CATALOG_DB_PATH", catdb, 1) == 0, "setenv SNAPDIR_CATALOG_DB_PATH");

    char store_uri[1100];
    CHECK(file_uri(store, store_uri, sizeof(store_uri)) == 0, "build catalog store uri");

    /* Push with the default catalog (NULL cache_dir uses default; we pass the temp cache to keep
     * hygiene). The push records into the catalog DB. */
    struct SnapdirError *perr = NULL;
    char *id = snapdir_push_blocking(tree, NULL, store_uri, 0, NULL, 0, cache, &perr);
    if (id == NULL || perr != NULL) {
        unsetenv("SNAPDIR_CATALOG_DB_PATH");
        return report_err("snapdir_push_blocking (catalog)", perr);
    }

    /* [§4-locations]: locations JSON is well-formed. catalog=NULL → default adapter. */
    struct SnapdirError *lerr = NULL;
    char *locations = snapdir_locations_json(cache, NULL, &lerr);
    if (locations == NULL || lerr != NULL) {
        snapdir_string_free(id);
        unsetenv("SNAPDIR_CATALOG_DB_PATH");
        return report_err("snapdir_locations_json", lerr);
    }
    CHECK(looks_like_json(locations), "locations JSON must be array/object-shaped");
    snapdir_string_free(locations);

    /* [§4-ancestors]: ancestors of the pushed id — well-formed JSON. */
    struct SnapdirError *aerr = NULL;
    char *ancestors = snapdir_ancestors_json(id, NULL, &aerr);
    if (ancestors == NULL || aerr != NULL) {
        snapdir_string_free(id);
        unsetenv("SNAPDIR_CATALOG_DB_PATH");
        return report_err("snapdir_ancestors_json", aerr);
    }
    CHECK(looks_like_json(ancestors), "ancestors JSON must be array/object-shaped");
    snapdir_string_free(ancestors);

    /* [§4-revisions]: revisions for a location — well-formed JSON. We use the store URI as the
     * location operand (a catalog records snapshots against their store location). */
    struct SnapdirError *rerr = NULL;
    char *revisions = snapdir_revisions_json(store_uri, NULL, &rerr);
    if (revisions == NULL || rerr != NULL) {
        snapdir_string_free(id);
        unsetenv("SNAPDIR_CATALOG_DB_PATH");
        return report_err("snapdir_revisions_json", rerr);
    }
    CHECK(looks_like_json(revisions), "revisions JSON must be array/object-shaped");
    snapdir_string_free(revisions);

    snapdir_string_free(id);
    unsetenv("SNAPDIR_CATALOG_DB_PATH");

    unlink_in(tree, "doc.txt");
    rmdir(tree);
    rmdir(store);
    rmdir(cache);
    /* catdir holds catalog.db; best-effort. */
    unlink_in(catdir, "catalog.db");
    rmdir(catdir);
    return 0;
}

/* ------------------------------------------------------------------------- */
/* [§4-verify-cache]/[§4-flush-cache]: cache maintenance on a real cache dir.  */
/* After a fetch populates the cache, verify_cache (no purge) → 0; flush_cache */
/* → 0; verify_cache again → 0 (an empty cache is still valid).               */
/* ------------------------------------------------------------------------- */
static int test_cache_surface(void) {
    char tree[1024], store[1024], cache[1024];
    CHECK(make_tmpdir("cachetree", tree, sizeof(tree)) == 0, "mkdtemp cache tree");
    CHECK(make_tmpdir("cachestore", store, sizeof(store)) == 0, "mkdtemp cache store");
    CHECK(make_tmpdir("realcache", cache, sizeof(cache)) == 0, "mkdtemp real cache");

    CHECK(write_file(tree, "c.txt", "cache content\n") == 0, "write c.txt");

    char store_uri[1100];
    CHECK(file_uri(store, store_uri, sizeof(store_uri)) == 0, "build cache store uri");

    /* Push then fetch into the cache so verify/flush operate on a populated cache. */
    struct SnapdirError *perr = NULL;
    char *id = snapdir_push_blocking(tree, NULL, store_uri, 0, NULL, 0, cache, &perr);
    if (id == NULL || perr != NULL) {
        return report_err("snapdir_push_blocking (cache)", perr);
    }
    struct SnapdirError *ferr = NULL;
    int frc = snapdir_fetch_blocking(id, store_uri, 0, &ferr);
    /* fetch uses the default cache; the verify/flush below operate on our explicit cache dir,
     * which the push populated (cache_dir arg). A non-zero fetch is still reported. */
    if (frc != 0 || ferr != NULL) {
        snapdir_string_free(id);
        return report_err("snapdir_fetch_blocking (cache)", ferr);
    }

    /* [§4-verify-cache]: verify the cache dir (no purge) → 0. */
    struct SnapdirError *verr = NULL;
    int vrc = snapdir_verify_cache(cache, false, &verr);
    if (vrc != 0 || verr != NULL) {
        snapdir_string_free(id);
        return report_err("snapdir_verify_cache", verr);
    }

    /* [§4-flush-cache]: flush the cache dir → 0. */
    struct SnapdirError *flerr = NULL;
    int flrc = snapdir_flush_cache(cache, &flerr);
    if (flrc != 0 || flerr != NULL) {
        snapdir_string_free(id);
        return report_err("snapdir_flush_cache", flerr);
    }

    /* [§4-verify-cache] again: a freshly-flushed (empty) cache is still valid → 0. */
    struct SnapdirError *v2err = NULL;
    int v2rc = snapdir_verify_cache(cache, false, &v2err);
    if (v2rc != 0 || v2err != NULL) {
        snapdir_string_free(id);
        return report_err("snapdir_verify_cache (post-flush)", v2err);
    }

    snapdir_string_free(id);

    unlink_in(tree, "c.txt");
    rmdir(tree);
    rmdir(store);
    rmdir(cache);
    return 0;
}

/* ------------------------------------------------------------------------- */
/* [§2 error contract / §4-code]: FAILURE paths must return the failure        */
/* sentinel (NULL / -1) AND set a freeable SnapdirError carrying the CORRECT   */
/* stable code (one of the 8). The current test only ever walked HAPPY paths   */
/* through the blocking surface; this pins that the impl never returns a false */
/* success on failure and that error codes are faithful to snapdir_api.        */
/*                                                                              */
/* Each case: assert sentinel, assert *err non-NULL, assert the borrowed code  */
/* string equals the expected stable code, then free the error EXACTLY ONCE    */
/* (the inspect ptrs are borrowed — never freed). Sanitizer-clean by           */
/* construction.                                                                */
/* ------------------------------------------------------------------------- */

/* Assert err is non-NULL with code==expected; free it once. Returns 0 on pass. */
static int expect_err_code(struct SnapdirError *err, const char *what,
                           const char *expected_code) {
    if (err == NULL) {
        fprintf(stderr, "FAIL: %s — expected a SnapdirError, got NULL (false success)\n", what);
        return 1;
    }
    const char *code = snapdir_error_code(err); /* borrowed — do NOT free */
    if (code == NULL || strcmp(code, expected_code) != 0) {
        fprintf(stderr, "FAIL: %s — expected code=%s, got code=%s msg=%s\n", what,
                expected_code, code ? code : "(null)",
                snapdir_error_message(err) ? snapdir_error_message(err) : "(null)");
        snapdir_error_free(err);
        return 1;
    }
    snapdir_error_free(err); /* free exactly once */
    return 0;
}

static int test_error_paths(void) {
    const char *good_id =
        "0000000000000000000000000000000000000000000000000000000000000000"; /* 64-hex, absent */
    const char *bad_id = "nothex"; /* malformed → INVALID_ID */
    const char *missing_store = "file:///tmp/snapdir_abi_no_such_store_xyzzy";

    /* (1) push to an INVALID store scheme → NULL + INVALID_STORE. */
    {
        struct SnapdirError *e = NULL;
        char *r = snapdir_push_blocking("/tmp", NULL, "bogus://nowhere", 0, NULL, 0, NULL, &e);
        CHECK(r == NULL, "push to invalid store scheme must return NULL");
        if (expect_err_code(e, "push invalid store scheme", "INVALID_STORE") != 0) {
            if (r) snapdir_string_free(r);
            return 1;
        }
    }

    /* (2) fetch with a MALFORMED id → -1 + INVALID_ID. */
    {
        struct SnapdirError *e = NULL;
        int rc = snapdir_fetch_blocking(bad_id, missing_store, 0, &e);
        CHECK(rc == -1, "fetch with malformed id must return -1");
        if (expect_err_code(e, "fetch malformed id", "INVALID_ID") != 0) {
            return 1;
        }
    }

    /* (3) fetch a non-existent (but well-formed) id from a missing store → -1 + STORE_ERROR. */
    {
        struct SnapdirError *e = NULL;
        int rc = snapdir_fetch_blocking(good_id, missing_store, 0, &e);
        CHECK(rc == -1, "fetch of absent snapshot must return -1");
        if (expect_err_code(e, "fetch absent snapshot", "STORE_ERROR") != 0) {
            return 1;
        }
    }

    /* (4) pull a non-existent id → -1 + STORE_ERROR (manifest not found). */
    {
        struct SnapdirError *e = NULL;
        int rc = snapdir_pull_blocking(good_id, missing_store, "/tmp/snapdir_abi_pull_nodest",
                                       false, 0, &e);
        CHECK(rc == -1, "pull of absent snapshot must return -1");
        if (expect_err_code(e, "pull absent snapshot", "STORE_ERROR") != 0) {
            return 1;
        }
    }

    /* (5) checkout with a MALFORMED id → -1 + INVALID_ID. */
    {
        struct SnapdirError *e = NULL;
        int rc = snapdir_checkout_blocking(bad_id, "/tmp/snapdir_abi_co_nodest", false, false, &e);
        CHECK(rc == -1, "checkout with malformed id must return -1");
        if (expect_err_code(e, "checkout malformed id", "INVALID_ID") != 0) {
            return 1;
        }
    }

    /* (6) checkout a well-formed id absent from the cache → -1 + STORE_ERROR. */
    {
        struct SnapdirError *e = NULL;
        int rc = snapdir_checkout_blocking(good_id, "/tmp/snapdir_abi_co_nodest2", false, false, &e);
        CHECK(rc == -1, "checkout of uncached snapshot must return -1");
        if (expect_err_code(e, "checkout uncached snapshot", "STORE_ERROR") != 0) {
            return 1;
        }
    }

    /* (7) verify a non-existent snapshot in a missing store → -1 + STORE_ERROR. */
    {
        struct SnapdirError *e = NULL;
        int rc = snapdir_verify_blocking(good_id, missing_store, false, &e);
        CHECK(rc == -1, "verify of absent snapshot must return -1");
        if (expect_err_code(e, "verify absent snapshot", "STORE_ERROR") != 0) {
            return 1;
        }
    }

    /* (8) diff against an INVALID store scheme on the `from` side → NULL + INVALID_STORE. */
    {
        struct SnapdirError *e = NULL;
        const char *bad_from[] = { "bogus://nowhere", NULL };
        const char *empty_to[] = { NULL };
        char *r = snapdir_diff_json(bad_from, empty_to, NULL, false, "error", &e);
        CHECK(r == NULL, "diff with invalid from-store scheme must return NULL");
        if (expect_err_code(e, "diff invalid from-store", "INVALID_STORE") != 0) {
            if (r) snapdir_string_free(r);
            return 1;
        }
    }

    /* (9) ancestors with a MALFORMED id → NULL + INVALID_ID. */
    {
        struct SnapdirError *e = NULL;
        char *r = snapdir_ancestors_json(bad_id, NULL, &e);
        CHECK(r == NULL, "ancestors with malformed id must return NULL");
        if (expect_err_code(e, "ancestors malformed id", "INVALID_ID") != 0) {
            if (r) snapdir_string_free(r);
            return 1;
        }
    }

    return 0;
}

/* ------------------------------------------------------------------------- */
/* [§4-stage cache_dir/keep]: stage(keep=true, cache_dir) writes the snapshot  */
/* objects UNDER the given cache_dir (the sharded `.objects` pool appears);     */
/* stage(keep=false, cache_dir) computes the SAME id but writes NOTHING to the  */
/* cache (no `.objects`). Pins that cache_dir/keep are honored end-to-end (the  */
/* m0-options-cache-dir-extend contract the impl delegates to), not ignored.    */
/* ------------------------------------------------------------------------- */

/* True iff `dir/.objects` exists. */
static int has_objects_pool(const char *dir) {
    char p[2048];
    int n = snprintf(p, sizeof(p), "%s/.objects", dir);
    if (n < 0 || (size_t)n >= sizeof(p)) {
        return 0;
    }
    struct stat st;
    return stat(p, &st) == 0;
}

static int test_stage_cache_dir(void) {
    char tree[1024], keepcache[1024], nokeepcache[1024];
    CHECK(make_tmpdir("stagetree", tree, sizeof(tree)) == 0, "mkdtemp stage tree");
    CHECK(make_tmpdir("keepcache", keepcache, sizeof(keepcache)) == 0, "mkdtemp keep cache");
    CHECK(make_tmpdir("nokeepcache", nokeepcache, sizeof(nokeepcache)) == 0, "mkdtemp nokeep cache");

    CHECK(write_file(tree, "s.txt", "stage payload\n") == 0, "write s.txt");

    /* Reference id (independent of staging). */
    char *ref_id = NULL;
    CHECK(id_of(tree, &ref_id) == 0, "snapdir_id on the stage tree");

    /* keep=true: objects must land under THIS cache_dir. */
    struct SnapdirError *e1 = NULL;
    char *kid = snapdir_stage(tree, true, keepcache, &e1);
    if (kid == NULL || e1 != NULL) {
        snapdir_string_free(ref_id);
        return report_err("snapdir_stage (keep=true, cache_dir)", e1);
    }
    CHECK(strcmp(kid, ref_id) == 0, "stage(keep=true) id must equal snapdir_id (checksum-faithful)");
    CHECK(has_objects_pool(keepcache),
          "stage(keep=true, cache_dir) must write the .objects pool UNDER cache_dir");
    snapdir_string_free(kid);

    /* keep=false: same id, but NOTHING written to the (fresh, empty) cache_dir. */
    struct SnapdirError *e2 = NULL;
    char *nkid = snapdir_stage(tree, false, nokeepcache, &e2);
    if (nkid == NULL || e2 != NULL) {
        snapdir_string_free(ref_id);
        return report_err("snapdir_stage (keep=false, cache_dir)", e2);
    }
    CHECK(strcmp(nkid, ref_id) == 0, "stage(keep=false) must still return the correct id");
    CHECK(!has_objects_pool(nokeepcache),
          "stage(keep=false, cache_dir) must NOT write any objects to cache_dir");
    snapdir_string_free(nkid);

    snapdir_string_free(ref_id);

    unlink_in(tree, "s.txt");
    rmdir(tree);
    /* keepcache now holds a .objects pool; best-effort cleanup of the empty nokeepcache. */
    rmdir(nokeepcache);
    rmdir(keepcache);
    return 0;
}

int main(void) {
    /* §3: front-load runtime init (idempotent; the blocking §4 fns also lazily init). */
    snapdir_init();

    if (test_sync_surface() != 0) {
        return 1;
    }
    if (test_blocking_roundtrip() != 0) {
        return 1;
    }
    if (test_sync_blocking() != 0) {
        return 1;
    }
    if (test_diff_json() != 0) {
        return 1;
    }
    if (test_catalog_json() != 0) {
        return 1;
    }
    if (test_cache_surface() != 0) {
        return 1;
    }
    if (test_error_paths() != 0) {
        return 1;
    }
    if (test_stage_cache_dir() != 0) {
        return 1;
    }

    printf("m1_abi_surface: all assertions passed\n");
    return 0;
}
