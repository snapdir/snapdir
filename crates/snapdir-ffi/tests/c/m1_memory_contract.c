#define _XOPEN_SOURCE 700 /* mkdtemp/rmdir/unlink: C99-safe POSIX (must precede all #includes) */

/*
 * m1_memory_contract.c — BLACK-BOX C adversary test for the snapdir-ffi §2 memory contract.
 *
 * GATE: m1-memory-contract-spec-tests (phase 35, owner adversary, opus).
 *
 * SOURCES (black-box): authored from TWO artifacts ONLY —
 *   1. include/snapdir.h          (the cbindgen-generated C header — the public contract)
 *   2. .gatesmith/reviews/m1-c-abi.md  (the locked C ABI spec, esp. §2 memory contract, §3 init, §4 surface)
 * It does NOT read crates/snapdir-ffi/src — ZERO Rust-source visibility. No struct layouts,
 * no private symbols, no internal helpers: only header-declared names + the locked spec.
 *
 * WHAT §2 CLAUSES THIS PINS (each assertion below is tagged with the clause):
 *   [§2-strings]     Returned char* are CString::into_raw; caller frees via snapdir_string_free(ptr).
 *   [§2-strfree-null] snapdir_string_free(NULL) is a no-op (must not crash under ASan).
 *   [§2-version]     snapdir_version() returns a STATIC string — caller must NOT free it.
 *   [§2-abi]         SNAPDIR_ABI_VERSION == 1.
 *   [§2-errfree]     Opaque SnapdirError is heap; freed exactly once via snapdir_error_free(err).
 *   [§2-errfree-null] snapdir_error_free(NULL) is a no-op (must not crash under ASan).
 *   [§2-outparam-ok] *err_out == NULL ⇒ success; string fn returns non-NULL on the happy path.
 *   [§2-outparam-err] non-NULL *err_out ⇒ failure; string fn returns NULL; caller frees *err_out.
 *   [§2-errmsg/code] snapdir_error_message/snapdir_error_code return const char* valid within the
 *                    err lifetime — non-NULL, non-empty; caller does NOT free them.
 *   [§2-codes]       error code is one of the 8 stable codes (IO_ERROR/HASH_MISMATCH/STORE_ERROR/
 *                    IN_FLUX/CATALOG_ERROR/INVALID_ID/INVALID_STORE/CONFLICT) or INTERNAL.
 *   [§3-init]        snapdir_init() is idempotent — called up top (and again) is safe.
 *
 * HOW IT EXERCISES THE CONTRACT (uses the §4 surface — `snapdir_id` / `snapdir_manifest`,
 * specified in §4 but NOT YET in the current 6-fn header). Referencing those undefined symbols
 * is INTENTIONAL: it makes this file fail to LINK now — the correct "no-impl state" for the
 * triple. The impl gate (m1-memory-contract-impl) wires snapdir_id/snapdir_manifest + their
 * error paths and `git mv`s this file BYTE-IDENTICAL into crates/snapdir-ffi/tests/c/.
 *
 * EXPECTED RESULT until impl: link failure (undefined symbols snapdir_id, snapdir_manifest).
 * After impl: compiles + runs clean under `clang -fsanitize=address,undefined`, exits 0 only
 * when the FULL §2 contract holds. Every allocation is freed exactly once (ASan/leak clean).
 *
 * The §4 signatures used here (verbatim from m1-c-abi.md §4):
 *   char* snapdir_id(const char* path, const char* exclude, unsigned walk_jobs,
 *                    const char* cache_dir, SnapdirError** err_out);
 *   char* snapdir_manifest(const char* path, const char* exclude, unsigned walk_jobs,
 *                          bool absolute, bool no_follow, const char* checksum_bin,
 *                          const char* cache_dir, const char* catalog, SnapdirError** err_out);
 */

#include "snapdir.h"

#include <assert.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/*
 * Forward declarations of the §4 surface this test drives. The CURRENT cbindgen header
 * (include/snapdir.h) only declares the 6 scaffolded fns (init/version/string_free/
 * error_free/error_message/error_code). snapdir_id / snapdir_manifest are specified in
 * §4 of the locked spec but not yet emitted into the header — so we declare them here to
 * compile; they remain UNDEFINED at link time until m1-memory-contract-impl lands them.
 * When the impl regenerates the header to include these, these forward decls become
 * redundant-but-compatible (identical C signatures) — the impl may drop them.
 */
extern char *snapdir_id(const char *path, const char *exclude, unsigned walk_jobs,
                        const char *cache_dir, struct SnapdirError **err_out);
extern char *snapdir_manifest(const char *path, const char *exclude, unsigned walk_jobs,
                              bool absolute, bool no_follow, const char *checksum_bin,
                              const char *cache_dir, const char *catalog,
                              struct SnapdirError **err_out);

/* The 8 stable error codes (+ INTERNAL for caught panics) per §2 / the header doc-comment. */
static const char *const STABLE_CODES[] = {
    "IO_ERROR", "HASH_MISMATCH", "STORE_ERROR", "IN_FLUX",
    "CATALOG_ERROR", "INVALID_ID", "INVALID_STORE", "CONFLICT",
    "INTERNAL",
};

static int code_is_stable(const char *code) {
    if (code == NULL) {
        return 0;
    }
    for (size_t i = 0; i < sizeof(STABLE_CODES) / sizeof(STABLE_CODES[0]); i++) {
        if (strcmp(code, STABLE_CODES[i]) == 0) {
            return 1;
        }
    }
    return 0;
}

#define CHECK(cond, msg)                                                        \
    do {                                                                        \
        if (!(cond)) {                                                          \
            fprintf(stderr, "FAIL: %s\n  at %s:%d\n", (msg), __FILE__, __LINE__); \
            return 1;                                                           \
        }                                                                       \
    } while (0)

/*
 * Build a small temp directory with one regular file, so the happy path has a real tree to
 * snapshot. Returns the mkdtemp'd path in `out` (caller must rmdir/unlink contents). Returns
 * 0 on success, non-zero on setup failure.
 */
static int make_temp_tree(char *out, size_t out_len) {
    char tmpl[] = "/tmp/snapdir_mc_XXXXXX";
    char *dir = mkdtemp(tmpl);
    if (dir == NULL) {
        return 1;
    }
    if (strlen(dir) + 1 > out_len) {
        return 1;
    }
    strcpy(out, dir);

    char filepath[1100];
    int n = snprintf(filepath, sizeof(filepath), "%s/hello.txt", dir);
    if (n < 0 || (size_t)n >= sizeof(filepath)) {
        return 1;
    }
    FILE *f = fopen(filepath, "w");
    if (f == NULL) {
        return 1;
    }
    fputs("snapdir memory-contract fixture\n", f);
    fclose(f);
    return 0;
}

static void cleanup_temp_tree(const char *dir) {
    if (dir == NULL || dir[0] == '\0') {
        return;
    }
    char filepath[1100];
    int n = snprintf(filepath, sizeof(filepath), "%s/hello.txt", dir);
    if (n > 0 && (size_t)n < sizeof(filepath)) {
        unlink(filepath);
    }
    rmdir(dir);
}

/* ------------------------------------------------------------------------- */
/* [§2-strfree-null] / [§2-errfree-null]: NULL frees are safe no-ops.        */
/* STRENGTHENED (tests-review): exhaustive repeated + INTERLEAVED NULL ops —  */
/* the §2 clause "Passing NULL is a no-op" must hold no matter how many times */
/* or in what order the four NULL-tolerant entry points are hit. ASan/UBSan   */
/* must stay clean across all of them.                                        */
/* ------------------------------------------------------------------------- */
static int test_null_safe_frees(void) {
    /* Repeated no-op frees (already staged) — keep them. */
    snapdir_string_free(NULL);
    snapdir_string_free(NULL);
    snapdir_error_free(NULL);
    snapdir_error_free(NULL);

    /* [§2-strfree-null]/[§2-errfree-null] INTERLEAVED + under a loop: pin that the
     * NULL no-op is order-independent and idempotent across many iterations
     * (a regression that, say, dereferenced NULL on the 2nd call would trip ASan). */
    for (int i = 0; i < 64; i++) {
        snapdir_string_free(NULL);
        snapdir_error_free(NULL);
        /* [§2-errmsg/code] NULL-err inspects interleaved with the NULL frees:
         * each must independently return NULL and must not touch freed/NULL memory. */
        CHECK(snapdir_error_message(NULL) == NULL,
              "snapdir_error_message(NULL) must return NULL every call");
        CHECK(snapdir_error_code(NULL) == NULL,
              "snapdir_error_code(NULL) must return NULL every call");
        snapdir_error_free(NULL);
        snapdir_string_free(NULL);
    }

    /* [§2-errmsg/code] on NULL err: header documents NULL ⇒ returns NULL. */
    CHECK(snapdir_error_message(NULL) == NULL,
          "snapdir_error_message(NULL) must return NULL");
    CHECK(snapdir_error_code(NULL) == NULL,
          "snapdir_error_code(NULL) must return NULL");
    return 0;
}

/* ------------------------------------------------------------------------- */
/* [§2-version] / [§2-abi]: static version string, never freed; ABI == 1.    */
/* ------------------------------------------------------------------------- */
static int test_static_version_not_freed(void) {
    const char *v1 = snapdir_version();
    CHECK(v1 != NULL, "snapdir_version() must return non-NULL");
    CHECK(v1[0] != '\0', "snapdir_version() must be non-empty");

    /* Static lifetime: two calls return a usable string each time; we read it and do NOT
     * free it. (We deliberately do NOT call snapdir_string_free on it — that would be a
     * free of static memory = UB; ASan/the contract require we leave it alone.) */
    const char *v2 = snapdir_version();
    CHECK(v2 != NULL, "snapdir_version() must return non-NULL on re-call");
    CHECK(strcmp(v1, v2) == 0, "snapdir_version() must be stable across calls");

    CHECK(SNAPDIR_ABI_VERSION == 1, "SNAPDIR_ABI_VERSION must be 1");
    return 0;
}

/* ------------------------------------------------------------------------- */
/* [§2-strings] / [§2-outparam-ok]: into_raw → string_free happy round-trip. */
/* Drives a sync string-returning §4 fn on a REAL temp tree.                  */
/* ------------------------------------------------------------------------- */
static int test_happy_into_raw_string_free(void) {
    char dir[1024];
    CHECK(make_temp_tree(dir, sizeof(dir)) == 0, "failed to build temp fixture tree");

    /* snapdir_id(path, exclude=NULL, walk_jobs=0/default, cache_dir=NULL, &err) */
    struct SnapdirError *err = NULL;
    char *id = snapdir_id(dir, NULL, 0, NULL, &err);

    /* On success: *err_out stays NULL AND the string fn returns non-NULL. */
    CHECK(err == NULL, "snapdir_id on a valid path must leave err_out == NULL");
    CHECK(id != NULL, "snapdir_id on a valid path must return a non-NULL id string");
    CHECK(id[0] != '\0', "returned id must be a non-empty string");

    /* The id is a BLAKE3 snapshot id: 64 lowercase hex chars per the oracle contract. */
    size_t len = strlen(id);
    CHECK(len == 64, "snapdir_id must return a 64-char hex BLAKE3 id");
    for (size_t i = 0; i < len; i++) {
        char c = id[i];
        int is_hex = (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f');
        CHECK(is_hex, "snapdir_id must be lowercase hex");
    }

    /* STRENGTHENED (tests-review) [§2-strings round-trip integrity]: snapdir_id is a
     * pure function of the tree — a second call on the SAME tree must produce a SEPARATE
     * allocation (distinct pointer) with the IDENTICAL 64-hex value, and BOTH must be
     * freed exactly once. This pins (a) into_raw hands back a fresh owned buffer each call
     * (not a cached/static pointer the caller would wrongly double-free), and (b) the
     * round-trip value is stable/deterministic, not garbage. */
    struct SnapdirError *err2 = NULL;
    char *id2 = snapdir_id(dir, NULL, 0, NULL, &err2);
    CHECK(err2 == NULL, "second snapdir_id on the same tree must succeed");
    CHECK(id2 != NULL, "second snapdir_id must return non-NULL");
    CHECK(id2 != id, "each snapdir_id must return a FRESH allocation (own buffer)");
    CHECK(strcmp(id, id2) == 0, "snapdir_id must be deterministic for the same tree");
    snapdir_string_free(id2);

    /* Caller owns it: free exactly once via snapdir_string_free (no leak under ASan). */
    snapdir_string_free(id);

    /* Also exercise snapdir_manifest happy path: returns owned manifest text. */
    struct SnapdirError *merr = NULL;
    char *manifest = snapdir_manifest(dir, NULL, 0, false, false, NULL, NULL, NULL, &merr);
    CHECK(merr == NULL, "snapdir_manifest on a valid path must leave err_out == NULL");
    CHECK(manifest != NULL, "snapdir_manifest on a valid path must return non-NULL text");
    CHECK(manifest[0] != '\0', "manifest text must be non-empty");
    /* The manifest must mention the file we created (sanity that it snapshotted the tree). */
    CHECK(strstr(manifest, "hello.txt") != NULL,
          "manifest must reference the fixture file");

    /* STRENGTHENED (tests-review) [§2-strings + option marshalling oracle].
     * The M0 program caught manifest()/id() IGNORING their ManifestOptions. Pin black-box
     * behavioral oracles so a future regression in the C-side option marshalling is caught:
     * each variant below must produce manifest text OBSERVABLY DIFFERENT from the default. */

    /* (a) absolute=true must change the path rendering vs the default ./-relative form.
     *     The default manifest must NOT contain the absolute temp-dir path; the absolute
     *     one MUST contain it. (If snapdir_manifest dropped `absolute`, these would be equal.) */
    struct SnapdirError *aerr = NULL;
    char *abs_manifest =
        snapdir_manifest(dir, NULL, 0, true /*absolute*/, false, NULL, NULL, NULL, &aerr);
    CHECK(aerr == NULL, "snapdir_manifest(absolute=true) must succeed");
    CHECK(abs_manifest != NULL, "snapdir_manifest(absolute=true) must return non-NULL");
    CHECK(strcmp(abs_manifest, manifest) != 0,
          "absolute=true must change manifest output vs default (option must marshal)");
    CHECK(strstr(abs_manifest, dir) != NULL,
          "absolute=true manifest must contain the absolute temp-dir path");
    CHECK(strstr(manifest, dir) == NULL,
          "default (relative) manifest must NOT contain the absolute temp-dir path");
    snapdir_string_free(abs_manifest);

    /* (b) exclude="hello" must drop the hello.txt entry — observably removing it from the
     *     output. (If snapdir_manifest dropped `exclude`, hello.txt would still be present.) */
    struct SnapdirError *xerr = NULL;
    char *excl_manifest =
        snapdir_manifest(dir, "hello", 0, false, false, NULL, NULL, NULL, &xerr);
    CHECK(xerr == NULL, "snapdir_manifest(exclude=\"hello\") must succeed");
    CHECK(excl_manifest != NULL, "snapdir_manifest(exclude) must return non-NULL");
    CHECK(strstr(excl_manifest, "hello.txt") == NULL,
          "exclude=\"hello\" must drop hello.txt from the manifest (option must marshal)");
    CHECK(strcmp(excl_manifest, manifest) != 0,
          "exclude must change manifest output vs default");
    snapdir_string_free(excl_manifest);

    /* (c) catalog="none" must NOT error and must still return a well-formed manifest
     *     (mentions the fixture). This pins the catalog-arg marshalling path ("none"=off)
     *     produces a valid result rather than a spurious failure. */
    struct SnapdirError *cerr = NULL;
    char *cat_manifest =
        snapdir_manifest(dir, NULL, 0, false, false, NULL, NULL, "none" /*catalog*/, &cerr);
    CHECK(cerr == NULL, "snapdir_manifest(catalog=\"none\") must not error");
    CHECK(cat_manifest != NULL, "snapdir_manifest(catalog=\"none\") must return non-NULL");
    CHECK(strstr(cat_manifest, "hello.txt") != NULL,
          "catalog=\"none\" manifest must still reference the fixture file");
    snapdir_string_free(cat_manifest);

    snapdir_string_free(manifest);

    cleanup_temp_tree(dir);
    return 0;
}

/* ------------------------------------------------------------------------- */
/* [§2-outparam-err] / [§2-errmsg/code] / [§2-codes] / [§2-errfree]:          */
/* error path on a NON-EXISTENT path. String fn returns NULL, err non-NULL.  */
/* ------------------------------------------------------------------------- */
static int test_error_out_param_round_trip(void) {
    const char *missing = "/nonexistent/snapdir/path/does/not/exist/xyzzy-42";

    struct SnapdirError *err = NULL;
    char *id = snapdir_id(missing, NULL, 0, NULL, &err);

    /* String-returning fn returns NULL on failure AND sets *err_out non-NULL. */
    CHECK(id == NULL, "snapdir_id on a missing path must return NULL");
    CHECK(err != NULL, "snapdir_id on a missing path must set err_out != NULL");

    /* message + code are const char* valid within the err lifetime; non-NULL, non-empty.
     * We must NOT free these — they borrow from err. */
    const char *msg = snapdir_error_message(err);
    CHECK(msg != NULL, "error message must be non-NULL on the error path");
    CHECK(msg[0] != '\0', "error message must be non-empty");

    const char *code = snapdir_error_code(err);
    CHECK(code != NULL, "error code must be non-NULL on the error path");
    CHECK(code[0] != '\0', "error code must be non-empty");
    CHECK(code_is_stable(code), "error code must be one of the 8 stable codes (or INTERNAL)");
    /* A missing-path failure surfaces as an IO error per §2's mapping. */
    CHECK(strcmp(code, "IO_ERROR") == 0,
          "a missing-path failure must map to IO_ERROR");

    /* The borrowed message pointer must still be valid right up until we free err. */
    CHECK(strlen(msg) > 0, "borrowed error message must remain valid before free");

    /* STRENGTHENED (tests-review) [§2-errmsg/code borrowed-pointer lifetime]: the inspect
     * pointers borrow from `err` and must be STABLE while `err` is alive — the header says
     * they "remain valid for the lifetime of `err`". Re-invoking the inspect fns must return
     * the SAME pointer (the cached CString) AND the SAME content. (A regression that
     * re-allocated per call would (a) leak and (b) break the "do not free" contract since the
     * caller has no handle to the transient allocation.) We use these strictly BEFORE free. */
    const char *msg_again = snapdir_error_message(err);
    const char *code_again = snapdir_error_code(err);
    CHECK(msg_again == msg, "error message pointer must be stable across calls (borrowed)");
    CHECK(code_again == code, "error code pointer must be stable across calls (borrowed)");
    CHECK(strcmp(code_again, "IO_ERROR") == 0,
          "error code must remain IO_ERROR on re-read");

    /* Free the error EXACTLY ONCE. (We do NOT free msg/code — they were borrowed.) */
    snapdir_error_free(err);

    return 0;
}

/* ------------------------------------------------------------------------- */
/* STRENGTHENED (tests-review) [§2-strings + §2-errfree leak-tightness].      */
/* Allocate→free many iterations of BOTH the happy (snapdir_id → string_free) */
/* and error (missing path → snapdir_error_free) paths. Under ASan's leak     */
/* detector, any per-iteration leak (a forgotten into_raw owner, a            */
/* SnapdirError not reclaimed by _free, or the cached msg/code CStrings not   */
/* dropped with the box) compounds 200× and is reported at exit. exit 0 here  */
/* ⇒ the contract's "free exactly once, no leak" holds under load.            */
/* ------------------------------------------------------------------------- */
static int test_leak_tightness_under_load(void) {
    char dir[1024];
    CHECK(make_temp_tree(dir, sizeof(dir)) == 0, "failed to build temp fixture tree");

    const char *missing = "/nonexistent/snapdir/leak/loop/path/zzz";

    for (int i = 0; i < 100; i++) {
        /* Happy: id allocated via into_raw, freed exactly once. */
        struct SnapdirError *e = NULL;
        char *id = snapdir_id(dir, NULL, 0, NULL, &e);
        CHECK(e == NULL, "loop: happy snapdir_id must leave err NULL");
        CHECK(id != NULL && strlen(id) == 64, "loop: happy snapdir_id must return a 64-hex id");
        snapdir_string_free(id);

        /* Error: SnapdirError (with its cached msg+code CStrings) allocated, freed once. */
        struct SnapdirError *ee = NULL;
        char *bad = snapdir_id(missing, NULL, 0, NULL, &ee);
        CHECK(bad == NULL, "loop: error snapdir_id must return NULL");
        CHECK(ee != NULL, "loop: error snapdir_id must set err");
        /* Touch the borrowed pointers (exercises the cached CStrings) before free. */
        CHECK(snapdir_error_message(ee) != NULL, "loop: error message must be non-NULL");
        CHECK(code_is_stable(snapdir_error_code(ee)), "loop: error code must be stable");
        snapdir_error_free(ee);
    }

    cleanup_temp_tree(dir);
    return 0;
}

/* ------------------------------------------------------------------------- */
/* [§3-init]: snapdir_init() is idempotent — calling it many times is safe.   */
/* ------------------------------------------------------------------------- */
static int test_init_idempotent(void) {
    snapdir_init();
    snapdir_init();
    snapdir_init();
    /* No crash, no leak; the runtime-backed fns above already ran post-init. */
    return 0;
}

int main(void) {
    /* §3: front-load runtime init (idempotent; the §4 fns also lazily init). */
    snapdir_init();

    if (test_null_safe_frees() != 0) {
        return 1;
    }
    if (test_static_version_not_freed() != 0) {
        return 1;
    }
    if (test_init_idempotent() != 0) {
        return 1;
    }
    if (test_happy_into_raw_string_free() != 0) {
        return 1;
    }
    if (test_error_out_param_round_trip() != 0) {
        return 1;
    }
    if (test_leak_tightness_under_load() != 0) {
        return 1;
    }

    /* Final NULL-safe frees after real allocations to pin the no-op once more. */
    snapdir_string_free(NULL);
    snapdir_error_free(NULL);

    printf("m1_memory_contract: all assertions passed\n");
    return 0;
}
