#define _XOPEN_SOURCE 700 /* pthread/mkdtemp/setenv: C99-safe POSIX (MUST precede all #includes) */

/*
 * m1_init.c — BLACK-BOX C adversary test for snapdir-ffi §3 runtime/init + §2 panic boundary.
 *
 * GATE: m1-init-idempotent-spec-tests (phase 35, owner adversary, opus). M1 C cluster 3/3
 *       (memory-contract done; abi-surface done; init-idempotent here — the FINAL M1 C cluster).
 *
 * SOURCES (black-box): authored from TWO artifacts ONLY —
 *   1. include/snapdir.h               (the cbindgen-generated C header — the public contract)
 *   2. .gatesmith/reviews/m1-c-abi.md  (the locked C ABI spec — esp. §3 runtime/init, §2 memory
 *                                       contract incl. the catch_unwind/panic boundary)
 * It does NOT read crates/snapdir-ffi/src — ZERO Rust-source visibility. No struct layouts,
 * no private symbols, no internal helpers: only header-declared names + the locked spec. The
 * sibling crates/snapdir-ffi/tests/c/{m1_memory_contract.c,m1_abi_surface.c} were consulted for
 * STYLE/harness conventions ONLY (the CHECK macro, mkdtemp/file:// helpers, report_err shape).
 *
 * BUILD/LINK NOTE FOR THE IMPL GATE: this test uses POSIX threads — the impl gate must compile +
 * link it with `-lpthread` (in addition to `-fsanitize=address,undefined` + `-lsnapdir_ffi`).
 * Every thread frees its own allocations; the whole test is ASan/UBSan + leak-clean by construction.
 *
 * WHAT §3 (+§2) CLAUSES THIS PINS (each test below is tagged with the clause):
 *
 *   [§3-idempotent]  snapdir_init() called MANY times in a row (100x) is safe — no crash, no
 *                    double-init; snapdir_version() still returns the SAME valid string afterward;
 *                    SNAPDIR_ABI_VERSION == 1. (The runtime is guarded by OnceLock/Once — a 2nd+
 *                    init is a no-op, never a re-build or a panic.)
 *
 *   [§3-lazy-init]   Calling a runtime-backed BLOCKING op WITHOUT any prior snapdir_init() is safe
 *                    (the fns lazily init the shared runtime on first use, per §3). This test runs
 *                    its lazy-init case FIRST, before ANY explicit snapdir_init(), so the very first
 *                    runtime use is the lazy path — a real file:// push/fetch/verify round-trip
 *                    succeeds, proving the lazily-built runtime drives the async facade.
 *
 *   [§3-concurrent]  Several pthreads (16) ALL call snapdir_init() concurrently (an init STORM);
 *                    some immediately run a real runtime-backed op right after. All succeed, no
 *                    data race / no crash (the impl gate runs this under ASan; TSan optional). The
 *                    embedded tokio runtime is created EXACTLY ONCE and SHARED — observed
 *                    behaviorally: after the storm, a blocking round-trip works from the MAIN thread
 *                    AND from a worker thread (a re-built-per-call or per-thread runtime would
 *                    either deadlock, leak, or mis-handle the shared OnceLock).
 *
 *   [§3-runtime-reuse] After the init storm + lazy path, a SECOND independent blocking round-trip
 *                    (push a different tree → fetch → verify → pull) still works — the ONE shared
 *                    runtime is reused across many ops on many threads, not torn down after the
 *                    first op.
 *
 *   [§2-panic-boundary] "A caught panic becomes a SnapdirError (code INTERNAL), never an unwind
 *                    across extern "C"." There is NO public way to deliberately trigger a Rust
 *                    panic from well-formed C — every documented input either succeeds or returns a
 *                    freeable SnapdirError (NOT a panic). So we do NOT fabricate a panic. Instead we
 *                    pin what IS observable black-box: pathological-but-VALID inputs — a very long
 *                    (~4 KB) NUL-terminated path, and MANY concurrent error-path calls hammering the
 *                    error machinery under the init storm — NEVER abort the process. EVERY call
 *                    either succeeds OR returns a freeable SnapdirError (with a stable code, freed
 *                    exactly once); the process reaches a clean exit 0. (The actual
 *                    catch_unwind-at-EVERY-boundary mechanic is audited by the -tests-review leg
 *                    against the landed src, which can see the impl; this spec-test pins the
 *                    OUTSIDE-observable contract: no abort, always a SnapdirError on failure.)
 *
 * EXPECTED RESULT until impl: snapdir_init / snapdir_version / snapdir_string_free /
 * snapdir_error_free / snapdir_error_message / snapdir_error_code ARE already in the header (from
 * the scaffold + memory-contract impl), and the §4 blocking/sync fns this test drives
 * (snapdir_id / snapdir_manifest / snapdir_push_blocking / snapdir_fetch_blocking /
 * snapdir_verify_blocking / snapdir_pull_blocking) ARE in the header (landed by the abi-surface
 * cluster). So this file should LINK once the ffi cdylib is on the link line — the "fail with no
 * impl" state for THIS cluster is therefore weaker (it mainly fails if a runtime/init regression
 * exists). That is expected: the impl gate adds the build wiring (-lpthread) + RUNS it under the
 * concurrency storm with ASan/UBSan, where a non-idempotent / non-shared / aborting runtime trips.
 * After impl: compiles + runs clean under `clang -fsanitize=address,undefined -lpthread`, exits 0
 * only when ALL §3/§2 invariants above hold.
 */

#include "snapdir.h"

#include <pthread.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

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

#define CHECK(cond, msg)                                                          \
    do {                                                                          \
        if (!(cond)) {                                                            \
            fprintf(stderr, "FAIL: %s\n  at %s:%d\n", (msg), __FILE__, __LINE__); \
            return 1;                                                             \
        }                                                                         \
    } while (0)

/* Print a borrowed (do-NOT-free) error message+code, free the error once, return non-zero. */
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
/* Tiny filesystem helpers (black-box; build/read real temp trees + stores).  */
/* ------------------------------------------------------------------------- */

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
    int n = snprintf(tmpl, sizeof(tmpl), "/tmp/snapdir_init_%s_XXXXXX", prefix);
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
    int n = snprintf(out, out_len, "file://%s", abs_dir);
    if (n < 0 || (size_t)n >= out_len) {
        return 1;
    }
    return 0;
}

/*
 * One full blocking round-trip against a fresh file:// store: build a 1-file tree, snapdir_id it,
 * push it (must return the SAME id), fetch it (0), verify it (0), pull it into a dest (0) and
 * re-id the dest (must equal the pushed id). This drives the SHARED embedded runtime end-to-end.
 * `label` distinguishes the source content/dirs so concurrent calls don't collide. Returns 0 on
 * success; on any failure prints a diagnostic and returns non-zero. Sanitizer-clean: every char*
 * freed exactly once, every SnapdirError freed exactly once.
 */
static int roundtrip(const char *label) {
    char tree[1024], storedir[1024], pulldir[1024];
    if (make_tmpdir(label, tree, sizeof(tree)) != 0) {
        fprintf(stderr, "FAIL: %s — mkdtemp tree\n", label);
        return 1;
    }
    if (make_tmpdir(label, storedir, sizeof(storedir)) != 0) {
        fprintf(stderr, "FAIL: %s — mkdtemp store\n", label);
        return 1;
    }
    if (make_tmpdir(label, pulldir, sizeof(pulldir)) != 0) {
        fprintf(stderr, "FAIL: %s — mkdtemp pull\n", label);
        return 1;
    }

    /* Content is keyed on the label so different round-trips snapshot different bytes. */
    char content[256];
    snprintf(content, sizeof(content), "runtime round-trip payload for %s\n", label);
    if (write_file(tree, "payload.txt", content) != 0) {
        fprintf(stderr, "FAIL: %s — write payload.txt\n", label);
        return 1;
    }

    char store_uri[1100];
    if (file_uri(storedir, store_uri, sizeof(store_uri)) != 0) {
        fprintf(stderr, "FAIL: %s — build store uri\n", label);
        return 1;
    }

    /* Reference id of the source tree (drives the shared runtime indirectly is NOT needed; id is
     * sync — but push/fetch/verify/pull below ARE runtime-backed). */
    struct SnapdirError *ierr = NULL;
    char *src_id = snapdir_id(tree, NULL, 0, NULL, &ierr);
    if (src_id == NULL || ierr != NULL) {
        return report_err("roundtrip snapdir_id", ierr);
    }
    if (strlen(src_id) != 64) {
        snapdir_string_free(src_id);
        fprintf(stderr, "FAIL: %s — snapdir_id not 64-hex\n", label);
        return 1;
    }

    /* push (runtime-backed) → must return the same id. source_id=NULL (path XOR id). */
    struct SnapdirError *perr = NULL;
    char *pushed_id = snapdir_push_blocking(tree, NULL, store_uri, 0, NULL, 0, NULL, &perr);
    if (pushed_id == NULL || perr != NULL) {
        snapdir_string_free(src_id);
        return report_err("roundtrip snapdir_push_blocking", perr);
    }
    if (strcmp(pushed_id, src_id) != 0) {
        snapdir_string_free(src_id);
        snapdir_string_free(pushed_id);
        fprintf(stderr, "FAIL: %s — pushed id != id(tree)\n", label);
        return 1;
    }

    /* fetch (runtime-backed) → 0. */
    struct SnapdirError *ferr = NULL;
    int frc = snapdir_fetch_blocking(pushed_id, store_uri, 0, &ferr);
    if (frc != 0 || ferr != NULL) {
        snapdir_string_free(src_id);
        snapdir_string_free(pushed_id);
        return report_err("roundtrip snapdir_fetch_blocking", ferr);
    }

    /* verify (runtime-backed) → 0 on a healthy store. */
    struct SnapdirError *verr = NULL;
    int vrc = snapdir_verify_blocking(pushed_id, store_uri, false, &verr);
    if (vrc != 0 || verr != NULL) {
        snapdir_string_free(src_id);
        snapdir_string_free(pushed_id);
        return report_err("roundtrip snapdir_verify_blocking", verr);
    }

    /* pull (runtime-backed) → 0; re-id the materialized tree → must equal the pushed id. */
    struct SnapdirError *plerr = NULL;
    int plrc = snapdir_pull_blocking(pushed_id, store_uri, pulldir, false, 0, &plerr);
    if (plrc != 0 || plerr != NULL) {
        snapdir_string_free(src_id);
        snapdir_string_free(pushed_id);
        return report_err("roundtrip snapdir_pull_blocking", plerr);
    }
    struct SnapdirError *p2err = NULL;
    char *pulled_id = snapdir_id(pulldir, NULL, 0, NULL, &p2err);
    if (pulled_id == NULL || p2err != NULL) {
        snapdir_string_free(src_id);
        snapdir_string_free(pushed_id);
        return report_err("roundtrip snapdir_id(pulled)", p2err);
    }
    if (strcmp(pulled_id, pushed_id) != 0) {
        snapdir_string_free(src_id);
        snapdir_string_free(pushed_id);
        snapdir_string_free(pulled_id);
        fprintf(stderr, "FAIL: %s — pulled id != pushed id (runtime did not faithfully drive the op)\n",
                label);
        return 1;
    }

    snapdir_string_free(pulled_id);
    snapdir_string_free(pushed_id);
    snapdir_string_free(src_id);

    /* best-effort cleanup; the /tmp reaper handles object pools. */
    unlink_in(tree, "payload.txt");
    unlink_in(pulldir, "payload.txt");
    rmdir(tree);
    rmdir(pulldir);
    rmdir(storedir);
    return 0;
}

/* ------------------------------------------------------------------------- */
/* [§3-lazy-init]: a runtime-backed op WITHOUT any prior snapdir_init().       */
/* This test is invoked FIRST from main(), before ANY explicit snapdir_init(), */
/* so the very first runtime use is the lazy-init path — it must succeed.      */
/* ------------------------------------------------------------------------- */
static int test_lazy_init_before_explicit_init(void) {
    /* NO snapdir_init() has run yet at this point (main() calls this first). A full blocking
     * round-trip therefore exercises the LAZY runtime construction on first use. */
    if (roundtrip("lazy") != 0) {
        fprintf(stderr, "FAIL: lazy-init round-trip (op before snapdir_init) must succeed\n");
        return 1;
    }
    return 0;
}

/* ------------------------------------------------------------------------- */
/* [§3-idempotent] / [§2-version] / [§2-abi]: many serial snapdir_init() calls */
/* are a safe no-op; version stays valid+stable; ABI == 1.                     */
/* ------------------------------------------------------------------------- */
static int test_init_idempotent_serial(void) {
    /* Capture version BEFORE the init storm. */
    const char *v_before = snapdir_version();
    CHECK(v_before != NULL && v_before[0] != '\0',
          "snapdir_version() must be non-NULL/non-empty before the init storm");

    /* 100x serial init — must be a guarded no-op every time (OnceLock/Once), never a re-build,
     * never a crash, never a leak (a re-init that leaked a runtime would compound 100x → ASan). */
    for (int i = 0; i < 100; i++) {
        snapdir_init();
    }

    /* version is STATIC (do NOT free) and must remain valid + STABLE across the storm. */
    const char *v_after = snapdir_version();
    CHECK(v_after != NULL && v_after[0] != '\0',
          "snapdir_version() must be non-NULL/non-empty after 100x init");
    CHECK(strcmp(v_before, v_after) == 0,
          "snapdir_version() must be stable across repeated snapdir_init()");
    /* Static lifetime: the same backing pointer is expected (OnceLock<CString>); pin it. */
    CHECK(v_before == v_after,
          "snapdir_version() must return the SAME static pointer (never re-allocated by init)");

    CHECK(SNAPDIR_ABI_VERSION == 1, "SNAPDIR_ABI_VERSION must be 1");

    /* A real runtime-backed op still works after the serial init storm. */
    CHECK(roundtrip("postserial") == 0,
          "a blocking round-trip must still work after 100x snapdir_init()");
    return 0;
}

/* ------------------------------------------------------------------------- */
/* [§3-concurrent] / [§3-runtime-reuse]: an init STORM across many pthreads.   */
/* Each worker calls snapdir_init() repeatedly; some then run a real blocking  */
/* round-trip from their own thread. The shared runtime must be created EXACTLY*/
/* ONCE and reused — a per-call/per-thread runtime would deadlock, leak, or    */
/* mishandle the OnceLock. ASan/(optional TSan) must stay clean.               */
/* ------------------------------------------------------------------------- */

#define N_THREADS 16

typedef struct {
    int idx;
    int rc; /* 0 = ok, non-zero = this worker observed a failure */
} init_worker_arg;

static void *init_worker(void *raw) {
    init_worker_arg *a = (init_worker_arg *)raw;
    a->rc = 0;

    /* Hammer snapdir_init() concurrently — the OnceLock must serialize a SINGLE construction. */
    for (int i = 0; i < 32; i++) {
        snapdir_init();
    }

    /* version must be reachable from a worker thread too (thread-safe, post/peri-init). */
    const char *v = snapdir_version();
    if (v == NULL || v[0] == '\0') {
        fprintf(stderr, "FAIL: worker %d — snapdir_version() unusable on a worker thread\n", a->idx);
        a->rc = 1;
        return NULL;
    }

    /* A subset of workers also drive a REAL runtime-backed round-trip from their own thread,
     * proving the shared runtime services blocking ops from arbitrary threads concurrently. */
    if (a->idx % 4 == 0) {
        char label[64];
        snprintf(label, sizeof(label), "worker%d", a->idx);
        if (roundtrip(label) != 0) {
            fprintf(stderr, "FAIL: worker %d — concurrent blocking round-trip failed\n", a->idx);
            a->rc = 1;
            return NULL;
        }
    }
    return NULL;
}

static int test_init_concurrent_storm(void) {
    pthread_t threads[N_THREADS];
    init_worker_arg args[N_THREADS];

    for (int i = 0; i < N_THREADS; i++) {
        args[i].idx = i;
        args[i].rc = 0;
        int prc = pthread_create(&threads[i], NULL, init_worker, &args[i]);
        CHECK(prc == 0, "pthread_create must succeed for the init storm");
    }
    for (int i = 0; i < N_THREADS; i++) {
        int jrc = pthread_join(threads[i], NULL);
        CHECK(jrc == 0, "pthread_join must succeed");
    }
    /* Every worker must have succeeded — no race, no crash, no failed op. */
    for (int i = 0; i < N_THREADS; i++) {
        CHECK(args[i].rc == 0, "every init-storm worker must succeed (shared runtime, no race)");
    }

    /* [§3-runtime-reuse]: after the concurrent storm, the ONE shared runtime is reused — a fresh
     * blocking round-trip from the MAIN thread still works (not torn down per-op/per-thread). */
    CHECK(roundtrip("postconcurrent-main") == 0,
          "main-thread blocking round-trip must work after the concurrent init storm (runtime reused)");
    return 0;
}

/* ------------------------------------------------------------------------- */
/* [§2-panic-boundary]: pathological-but-VALID inputs NEVER abort the process; */
/* every call either succeeds or returns a freeable SnapdirError (stable code).*/
/* (a) a very long (~4 KB) NUL-terminated path. (b) MANY concurrent error-path */
/* calls hammering the error machinery. No fabricated panic — well-formed C    */
/* yields errors, not unwinds; the catch_unwind-everywhere mechanic is audited */
/* by the -tests-review leg against the landed src.                            */
/* ------------------------------------------------------------------------- */

/* (a) A very long but well-formed, NUL-terminated, non-existent path. snapdir_id must NOT abort —
 * it must return NULL + a freeable SnapdirError with a stable code (an IO/INVALID class failure). */
static int test_panic_boundary_long_path(void) {
    /* Build a ~4 KB path: many "/seg" components, NUL-terminated, definitely non-existent. */
    char longpath[4096];
    size_t pos = 0;
    longpath[pos++] = '/';
    while (pos < sizeof(longpath) - 8) {
        int n = snprintf(longpath + pos, sizeof(longpath) - pos, "seg/");
        if (n <= 0 || (size_t)n >= sizeof(longpath) - pos) {
            break;
        }
        pos += (size_t)n;
    }
    longpath[pos] = '\0';

    struct SnapdirError *err = NULL;
    char *id = snapdir_id(longpath, NULL, 0, NULL, &err);
    /* The contract: NEVER an abort. Either it (implausibly) succeeds with an id, or it returns
     * NULL + a freeable SnapdirError carrying a stable code. Both are acceptable; an abort is NOT. */
    if (id != NULL) {
        /* Implausible for a non-existent path, but if so it must be a clean owned string. */
        CHECK(err == NULL, "if snapdir_id(longpath) returns non-NULL, err must be NULL");
        snapdir_string_free(id);
    } else {
        CHECK(err != NULL, "snapdir_id(longpath) failure must set a SnapdirError (never abort)");
        const char *code = snapdir_error_code(err);
        const char *msg = snapdir_error_message(err);
        CHECK(code != NULL && code[0] != '\0', "long-path error code must be non-empty");
        CHECK(code_is_stable(code), "long-path error code must be one of the stable codes / INTERNAL");
        CHECK(msg != NULL && msg[0] != '\0', "long-path error message must be non-empty");
        snapdir_error_free(err); /* free exactly once; msg/code were borrowed — not freed */
    }
    return 0;
}

/* (b) MANY concurrent error-path calls: each worker hammers a failing op in a loop. Under the
 * init storm + concurrency, the error machinery (catch_unwind boundary → SnapdirError) must hold:
 * every failing call returns the sentinel + a freeable SnapdirError with a stable code, the
 * process never aborts, and there is no leak (each error freed exactly once). */
typedef struct {
    int idx;
    int rc;
} err_worker_arg;

static void *err_worker(void *raw) {
    err_worker_arg *a = (err_worker_arg *)raw;
    a->rc = 0;

    const char *missing_store = "file:///tmp/snapdir_init_no_such_store_xyzzy";
    const char *bad_id = "nothex"; /* malformed → INVALID_ID class */
    const char *good_id =
        "0000000000000000000000000000000000000000000000000000000000000000"; /* absent */

    for (int i = 0; i < 50; i++) {
        /* (1) fetch a malformed id → -1 + freeable SnapdirError, stable code. */
        struct SnapdirError *e1 = NULL;
        int rc1 = snapdir_fetch_blocking(bad_id, missing_store, 0, &e1);
        if (rc1 != -1 || e1 == NULL || !code_is_stable(snapdir_error_code(e1))) {
            fprintf(stderr, "FAIL: err_worker %d — malformed-id fetch contract broken\n", a->idx);
            if (e1) snapdir_error_free(e1);
            a->rc = 1;
            return NULL;
        }
        snapdir_error_free(e1);

        /* (2) verify an absent (well-formed) snapshot in a missing store → -1 + freeable err. */
        struct SnapdirError *e2 = NULL;
        int rc2 = snapdir_verify_blocking(good_id, missing_store, false, &e2);
        if (rc2 != -1 || e2 == NULL || !code_is_stable(snapdir_error_code(e2))) {
            fprintf(stderr, "FAIL: err_worker %d — absent-snapshot verify contract broken\n", a->idx);
            if (e2) snapdir_error_free(e2);
            a->rc = 1;
            return NULL;
        }
        snapdir_error_free(e2);

        /* (3) snapdir_id of a non-existent path → NULL + freeable err, stable code. */
        struct SnapdirError *e3 = NULL;
        char *r3 = snapdir_id("/nonexistent/snapdir/init/error/path/zzz", NULL, 0, NULL, &e3);
        if (r3 != NULL || e3 == NULL || !code_is_stable(snapdir_error_code(e3))) {
            fprintf(stderr, "FAIL: err_worker %d — missing-path id contract broken\n", a->idx);
            if (r3) snapdir_string_free(r3);
            if (e3) snapdir_error_free(e3);
            a->rc = 1;
            return NULL;
        }
        snapdir_error_free(e3);
    }
    return NULL;
}

static int test_panic_boundary_concurrent_errors(void) {
    pthread_t threads[N_THREADS];
    err_worker_arg args[N_THREADS];

    for (int i = 0; i < N_THREADS; i++) {
        args[i].idx = i;
        args[i].rc = 0;
        int prc = pthread_create(&threads[i], NULL, err_worker, &args[i]);
        CHECK(prc == 0, "pthread_create must succeed for the error storm");
    }
    for (int i = 0; i < N_THREADS; i++) {
        int jrc = pthread_join(threads[i], NULL);
        CHECK(jrc == 0, "pthread_join must succeed (error storm)");
    }
    for (int i = 0; i < N_THREADS; i++) {
        CHECK(args[i].rc == 0,
              "every error-storm worker must observe a freeable SnapdirError, never an abort");
    }
    return 0;
}

/* ------------------------------------------------------------------------- */
/* [§3-runtime-reuse] / [§2-version] (-tests-review STRENGTHENING)             */
/*                                                                            */
/* version-pointer STABILITY under a concurrent init+version storm. The src   */
/* audit confirmed snapdir_version() caches its CString in a static           */
/* OnceLock<CString> and returns `.as_ptr()` — so EVERY call from ANY thread, */
/* before/during/after any number of snapdir_init() calls, must return the    */
/* IDENTICAL backing pointer. A per-call re-allocation, a non-shared (e.g.    */
/* thread-local) cache, or a runtime re-build that re-seeded version would    */
/* hand back a DIFFERENT pointer from some thread → caught here. Each worker   */
/* also pounds snapdir_init() so version-read races init-write on the runtime */
/* OnceLock; the two OnceLocks are independent but a broken "re-init rebuilds */
/* everything" impl would perturb the version pointer too.                    */
/* ------------------------------------------------------------------------- */

#define N_VER_THREADS 24

typedef struct {
    int idx;
    const char *expected_ptr; /* the reference static version pointer */
    int rc;
} ver_worker_arg;

static void *ver_worker(void *raw) {
    ver_worker_arg *a = (ver_worker_arg *)raw;
    a->rc = 0;
    for (int i = 0; i < 64; i++) {
        /* interleave init and version reads so a version-read races an init-write. */
        snapdir_init();
        const char *v = snapdir_version();
        if (v == NULL || v[0] == '\0') {
            fprintf(stderr, "FAIL: ver_worker %d — snapdir_version() unusable under storm\n", a->idx);
            a->rc = 1;
            return NULL;
        }
        /* The cached static pointer must be byte-identical across every thread + call. */
        if (v != a->expected_ptr) {
            fprintf(stderr,
                    "FAIL: ver_worker %d — snapdir_version() returned a DIFFERENT static pointer "
                    "under the init storm (expected stable OnceLock<CString> cache)\n",
                    a->idx);
            a->rc = 1;
            return NULL;
        }
        snapdir_init();
    }
    return NULL;
}

static int test_version_pointer_stable_under_storm(void) {
    /* Reference pointer captured on the main thread. */
    const char *ref = snapdir_version();
    CHECK(ref != NULL && ref[0] != '\0',
          "reference snapdir_version() must be non-NULL/non-empty");

    pthread_t threads[N_VER_THREADS];
    ver_worker_arg args[N_VER_THREADS];
    for (int i = 0; i < N_VER_THREADS; i++) {
        args[i].idx = i;
        args[i].expected_ptr = ref;
        args[i].rc = 0;
        int prc = pthread_create(&threads[i], NULL, ver_worker, &args[i]);
        CHECK(prc == 0, "pthread_create must succeed for the version-stability storm");
    }
    for (int i = 0; i < N_VER_THREADS; i++) {
        int jrc = pthread_join(threads[i], NULL);
        CHECK(jrc == 0, "pthread_join must succeed (version-stability storm)");
    }
    for (int i = 0; i < N_VER_THREADS; i++) {
        CHECK(args[i].rc == 0,
              "every thread must observe the SAME stable static version pointer under the storm");
    }
    /* And the main-thread pointer is still the same one after the storm. */
    CHECK(snapdir_version() == ref,
          "snapdir_version() must still return the same static pointer after the version storm");
    return 0;
}

/* ------------------------------------------------------------------------- */
/* [§3-concurrent] / [§3-runtime-reuse] (-tests-review STRENGTHENING)          */
/*                                                                            */
/* init DENSELY INTERLEAVED with real runtime-backed ops, many threads, each  */
/* thread alternating init → op → init → op. The src audit confirmed ONE      */
/* shared OnceLock<Runtime> drives every block_on; this pins that arbitrary   */
/* init/op interleavings across 20 threads all share + reuse that single      */
/* runtime (a per-call/per-thread Runtime::new would deadlock under block_on  */
/* nesting, leak runtimes → ASan, or mishandle the OnceLock). Every thread    */
/* runs MULTIPLE ops to pin reuse (the runtime is not torn down after op #1). */
/* ------------------------------------------------------------------------- */

#define N_MIX_THREADS 20

typedef struct {
    int idx;
    int rc;
} mix_worker_arg;

static void *mix_worker(void *raw) {
    mix_worker_arg *a = (mix_worker_arg *)raw;
    a->rc = 0;
    for (int round = 0; round < 2; round++) {
        snapdir_init(); /* init interleaved BEFORE an op */
        char label[64];
        snprintf(label, sizeof(label), "mix%d_%d", a->idx, round);
        if (roundtrip(label) != 0) {
            fprintf(stderr, "FAIL: mix_worker %d round %d — interleaved init+op round-trip failed\n",
                    a->idx, round);
            a->rc = 1;
            return NULL;
        }
        snapdir_init(); /* init interleaved AFTER an op (same shared runtime) */
    }
    return NULL;
}

static int test_init_op_interleaved_storm(void) {
    pthread_t threads[N_MIX_THREADS];
    mix_worker_arg args[N_MIX_THREADS];
    for (int i = 0; i < N_MIX_THREADS; i++) {
        args[i].idx = i;
        args[i].rc = 0;
        int prc = pthread_create(&threads[i], NULL, mix_worker, &args[i]);
        CHECK(prc == 0, "pthread_create must succeed for the init/op interleave storm");
    }
    for (int i = 0; i < N_MIX_THREADS; i++) {
        int jrc = pthread_join(threads[i], NULL);
        CHECK(jrc == 0, "pthread_join must succeed (init/op interleave storm)");
    }
    for (int i = 0; i < N_MIX_THREADS; i++) {
        CHECK(args[i].rc == 0,
              "every interleave worker must succeed (one shared runtime reused across init+ops)");
    }
    /* Reuse once more from main after the interleave storm — runtime still alive. */
    CHECK(roundtrip("post-interleave-main") == 0,
          "main-thread round-trip must still work after the init/op interleave storm (reuse)");
    return 0;
}

int main(void) {
    /* [§3-lazy-init] MUST run FIRST — before any explicit snapdir_init() — so the first runtime
     * use is the lazy path. (Once a sibling test calls snapdir_init() the runtime is initialized.) */
    if (test_lazy_init_before_explicit_init() != 0) {
        return 1;
    }

    /* [§3-idempotent] + [§2-version]/[§2-abi]: 100x serial init no-op, stable version, ABI==1. */
    if (test_init_idempotent_serial() != 0) {
        return 1;
    }

    /* [§3-concurrent] + [§3-runtime-reuse]: 16-thread init storm + concurrent + main-thread reuse. */
    if (test_init_concurrent_storm() != 0) {
        return 1;
    }

    /* [§2-panic-boundary] (a): a ~4 KB valid path never aborts — succeeds or freeable error. */
    if (test_panic_boundary_long_path() != 0) {
        return 1;
    }

    /* [§2-panic-boundary] (b): many concurrent error-path calls never abort; always freeable err. */
    if (test_panic_boundary_concurrent_errors() != 0) {
        return 1;
    }

    /* [§2-version]/[§3] STRENGTHENING: version static pointer stable across a 24-thread init+read storm. */
    if (test_version_pointer_stable_under_storm() != 0) {
        return 1;
    }

    /* [§3-concurrent]/[§3-runtime-reuse] STRENGTHENING: 20 threads alternate init→op→init (reuse). */
    if (test_init_op_interleaved_storm() != 0) {
        return 1;
    }

    /* Final NULL-safe frees to pin the no-op once more after real allocations (ASan-clean). */
    snapdir_string_free(NULL);
    snapdir_error_free(NULL);

    printf("m1_init: all assertions passed\n");
    return 0;
}
