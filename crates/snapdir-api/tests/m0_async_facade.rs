// M0 async-facade black-box spec (gate: m0-async-facade-spec-tests, phase 34).
//
// Pins the DEEPER §7 async-facade CONTRACT of `crates/snapdir-api` against a
// `file://` temp store. Authored from `.gatesmith/reviews/m0-public-api.md`
// §7 (+ §1/§6) ALONE — black-box re: the async wiring. The async fns already
// round-trip (proven by the api-surface cluster); THIS suite pins runtime
// reuse, dropped-future safety, cancellation, error mapping, concurrency, and
// the sync-stays-sync boundary.
//
// SPEC §7:
//   - `snapdir-api` owns ONE shared multi-thread tokio runtime (lazily, via
//     `OnceLock`). Each async fn `spawn_blocking`s the corresponding SYNC
//     `snapdir-stores` call (which internally `block_on`s the per-store
//     runtime) so the reactor never blocks. (§7 bullet 1)
//   - Dropping a returned future is SAFE: the spawned blocking task completes
//     harmlessly. (§7 bullet 2)
//   - This isolates tokio in `snapdir-api`; `snapdir-core` gains nothing. (§7 b3)
// SPEC §6: manifest/id/stage are SYNC; push/fetch/pull/checkout/sync/diff/verify
//   are ASYNC. §1: errors surface as typed `SnapdirError`.
//
// EXPECTED to FAIL / be fragile until the async-facade impl hardens behavior —
// these are not weakened to pass. A per-call `Runtime::new().block_on()` impl
// would PANIC ("Cannot start a runtime from within a runtime" / "Cannot drop a
// runtime in a context where blocking is not allowed") for every test below
// that calls the facade from inside a `#[tokio::test]` (multi_thread) context;
// only a `spawn_blocking`-over-a-shared-OnceLock-runtime impl survives them.
//
// NOTE for the impl gate: this file uses `tokio::time::timeout`, so the
// `time` feature must be enabled on the `tokio` DEV-dependency of
// `crates/snapdir-api/Cargo.toml` (it currently has `macros, rt-multi-thread`;
// ADD `time`). No `futures` crate is required — concurrency uses `tokio::spawn`
// / `tokio::join!` only.

#![allow(
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::redundant_locals,
    clippy::cast_sign_loss,
    clippy::items_after_statements
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use snapdir_api::{
    CheckoutOptions, DiffEntry, DiffOptions, ManifestOptions, PushSource, SnapdirError, SnapshotId,
    StageOptions, StoreUri, TransferOptions, VerifyOptions, VerifyResult,
};

// ---------------------------------------------------------------------------
// Helpers (test-owned; no visibility into src/).
// ---------------------------------------------------------------------------

/// A small, deterministic directory tree in a fresh temp dir.
/// Keep the returned guard alive for the duration of the test.
fn fixture_tree() -> (tempfile::TempDir, PathBuf) {
    let td = tempfile::tempdir().expect("tempdir");
    let root = td.path().to_path_buf();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("a.txt"), b"hello async snapdir\n").unwrap();
    std::fs::write(root.join("sub/b.bin"), vec![7u8; 8192]).unwrap();
    std::fs::write(root.join("empty.txt"), b"").unwrap();
    (td, root)
}

/// A distinct tree whose contents differ from `fixture_tree`, for diff cases.
fn fixture_tree_variant() -> (tempfile::TempDir, PathBuf) {
    let td = tempfile::tempdir().expect("tempdir");
    let root = td.path().to_path_buf();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("a.txt"), b"hello async snapdir\n").unwrap(); // Unchanged
    std::fs::write(root.join("sub/b.bin"), vec![9u8; 8192]).unwrap(); // Modified
    std::fs::write(root.join("added.txt"), b"new file\n").unwrap(); // Added
    (td, root)
}

/// A `file://` StoreUri pointing at a fresh temp dir (object store / catalog root).
fn file_store() -> (tempfile::TempDir, StoreUri) {
    let td = tempfile::tempdir().expect("tempdir");
    let uri = format!("file://{}", td.path().display());
    let store = StoreUri::parse(&uri).expect("parse file:// store uri");
    (td, store)
}

fn is_64_lower_hex(s: &str) -> bool {
    s.len() == 64
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

// ===========================================================================
// ROUND-TRIP CORRECTNESS against file:// (driven from an ASYNC context, which
// already exercises the "no per-call runtime" contract — see header).
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_fetch_pull_roundtrip_reids_to_source() {
    // §6/§7: push(Path)->fetch->pull to a fresh dir, re-id == source id. The
    // whole chain runs from inside a multi_thread runtime: a per-call
    // Runtime::new().block_on() would panic here — the facade must spawn_blocking.
    let (_g, root) = fixture_tree();
    let (_sg, store) = file_store();
    let to = TransferOptions::default();

    let src_id = snapdir_api::id(root.as_path(), &ManifestOptions::default()).expect("source id");
    let pushed = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
        .await
        .expect("push from inside async ctx");
    assert_eq!(pushed, src_id, "push returns the source snapshot id");
    assert!(is_64_lower_hex(&pushed.to_hex()));

    snapdir_api::fetch(&pushed, &store, &to)
        .await
        .expect("fetch objects from file:// store");

    let dest = tempfile::tempdir().expect("dest tempdir");
    let co = CheckoutOptions::default();
    snapdir_api::pull(&pushed, &store, dest.path(), &co)
        .await
        .expect("pull to a fresh dir");

    let reid = snapdir_api::id(dest.path(), &ManifestOptions::default()).expect("re-id pulled");
    assert_eq!(reid, src_id, "pulled tree re-ids to the source id");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_staged_id_then_checkout_reids_to_source() {
    // §6/§7: push(StagedId) round-trips; checkout (after fetch) re-ids to source.
    let (_g, root) = fixture_tree();
    let (_sg, store) = file_store();
    let to = TransferOptions::default();

    let src_id = snapdir_api::id(root.as_path(), &ManifestOptions::default()).expect("source id");
    let staged = snapdir_api::stage(root.as_path(), &StageOptions::default()).expect("stage");
    assert_eq!(staged, src_id, "stage() == id()");

    let pushed = snapdir_api::push(PushSource::StagedId(&staged), &store, &to)
        .await
        .expect("push staged id");
    assert_eq!(pushed, src_id);

    snapdir_api::fetch(&pushed, &store, &to)
        .await
        .expect("fetch");
    let dest = tempfile::tempdir().expect("dest");
    snapdir_api::checkout(&pushed, dest.path(), &CheckoutOptions::default())
        .await
        .expect("checkout from cache");
    let reid = snapdir_api::id(dest.path(), &ManifestOptions::default()).expect("re-id");
    assert_eq!(reid, src_id, "checked-out tree re-ids to source");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sync_makes_id_fetchable_from_dst() {
    // §6/§7: sync(id, src, dst) makes the id fetchable/pullable from dst.
    let (_g, root) = fixture_tree();
    let (_sg1, src) = file_store();
    let (_sg2, dst) = file_store();
    let to = TransferOptions::default();

    let id = snapdir_api::push(PushSource::Path(root.as_path()), &src, &to)
        .await
        .expect("push to src");
    snapdir_api::sync(&id, &src, &dst, &to)
        .await
        .expect("sync src -> dst");

    // The id must now be pullable straight from dst.
    let dest = tempfile::tempdir().expect("dest");
    snapdir_api::pull(&id, &dst, dest.path(), &CheckoutOptions::default())
        .await
        .expect("pull from dst after sync");
    let reid = snapdir_api::id(dest.path(), &ManifestOptions::default()).expect("re-id");
    assert_eq!(reid, id, "id fetchable from dst after sync");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn diff_returns_vec_diffentry() {
    // §6: diff(o) -> Result<Vec<DiffEntry>>. Push two differing trees to two
    // stores, diff them; the call must succeed from async ctx and return a Vec.
    let (_ga, root_a) = fixture_tree();
    let (_gb, root_b) = fixture_tree_variant();
    let (_sa, src) = file_store();
    let (_sb, dst) = file_store();
    let to = TransferOptions::default();

    let _ = snapdir_api::push(PushSource::Path(root_a.as_path()), &src, &to)
        .await
        .expect("push a");
    let _ = snapdir_api::push(PushSource::Path(root_b.as_path()), &dst, &to)
        .await
        .expect("push b");

    let opts = DiffOptions {
        from: vec![src.clone()],
        to: vec![dst.clone()],
        all: true,
        ..Default::default()
    };
    let entries: Vec<DiffEntry> = snapdir_api::diff(&opts).await.expect("diff two stores");
    // Differing trees => at least one non-Unchanged entry exists.
    assert!(
        !entries.is_empty(),
        "diff of differing trees yields entries; got {entries:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verify_ok_on_healthy_store() {
    // §6: verify(id, store, o) -> Result<VerifyResult> with .ok == true on a
    // healthy store. Exercises the async path returning a typed struct.
    let (_g, root) = fixture_tree();
    let (_sg, store) = file_store();
    let to = TransferOptions::default();
    let id = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
        .await
        .expect("push");
    let vr: VerifyResult = snapdir_api::verify(&id, &store, &VerifyOptions::default())
        .await
        .expect("verify healthy store");
    assert!(vr.ok, "verify().ok is true on a healthy store");
}

// ===========================================================================
// SHARED RUNTIME REUSE — §7 bullet 1 ("ONE shared runtime, never per-call").
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_sequential_async_calls_reuse_one_runtime() {
    // §7.1: a long sequence of async calls from inside a runtime must all
    // succeed — a per-call Runtime::new() would exhaust threads / panic on the
    // 2nd nested runtime. Reuse of the shared OnceLock runtime makes this cheap.
    let (_g, root) = fixture_tree();
    let to = TransferOptions::default();
    for i in 0..24 {
        let (_sg, store) = file_store();
        let id = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
            .await
            .unwrap_or_else(|e| panic!("sequential push #{i} failed: {e}"));
        snapdir_api::verify(&id, &store, &VerifyOptions::default())
            .await
            .unwrap_or_else(|e| panic!("sequential verify #{i} failed: {e}"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sixteen_concurrent_pushes_against_distinct_stores() {
    // §7.1: 16 CONCURRENT pushes (distinct stores) all complete — proves the
    // facade does not serialize on / re-create a runtime per call, and does not
    // deadlock. Each task owns its store + tree (move) so it is 'static.
    const N: usize = 16;
    let mut tasks = Vec::with_capacity(N);
    for _ in 0..N {
        tasks.push(tokio::spawn(async move {
            let (g, root) = fixture_tree();
            let (sg, store) = file_store();
            let to = TransferOptions::default();
            let id = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
                .await
                .expect("concurrent push");
            // Keep guards alive until after verify.
            let vr = snapdir_api::verify(&id, &store, &VerifyOptions::default())
                .await
                .expect("concurrent verify");
            drop((g, sg));
            (id, vr.ok)
        }));
    }
    for t in tasks {
        let (id, ok) = t.await.expect("join concurrent task");
        assert!(is_64_lower_hex(&id.to_hex()));
        assert!(ok, "each concurrent push verifies ok");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tokio_join_runs_async_fns_in_parallel() {
    // §7.1: tokio::join! of several facade calls completes without a
    // "cannot start a runtime from within a runtime" panic and without deadlock.
    let (_g1, r1) = fixture_tree();
    let (_g2, r2) = fixture_tree();
    let (_g3, r3) = fixture_tree();
    let (_s1, st1) = file_store();
    let (_s2, st2) = file_store();
    let (_s3, st3) = file_store();
    let to = TransferOptions::default();
    let (a, b, c) = tokio::join!(
        snapdir_api::push(PushSource::Path(r1.as_path()), &st1, &to),
        snapdir_api::push(PushSource::Path(r2.as_path()), &st2, &to),
        snapdir_api::push(PushSource::Path(r3.as_path()), &st3, &to),
    );
    a.expect("join push 1");
    b.expect("join push 2");
    c.expect("join push 3");
}

// ===========================================================================
// DROPPED-FUTURE SAFETY — §7 bullet 2 ("dropping a returned future is safe").
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_a_future_before_completion_leaves_runtime_usable() {
    // §7.2: create an async op future, DROP it before completion, then run
    // another async op successfully. The spawned blocking task completes
    // harmlessly; the shared runtime is not poisoned.
    let (_g, root) = fixture_tree();
    let (_sg, store) = file_store();
    let to = TransferOptions::default();

    {
        // Build the future but never poll it to completion; drop at scope end.
        let fut = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to);
        drop(fut);
    }

    // The runtime must still be fully usable for a fresh op.
    let (_g2, root2) = fixture_tree();
    let (_sg2, store2) = file_store();
    let id = snapdir_api::push(PushSource::Path(root2.as_path()), &store2, &to)
        .await
        .expect("runtime still usable after dropping a future");
    assert!(is_64_lower_hex(&id.to_hex()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn timeout_cancelled_future_does_not_poison_runtime() {
    // §7.2: cancellation via tokio::time::timeout with a tiny budget — whether
    // it elapses or completes, a SUBSEQUENT async op must still succeed. The
    // cancelled future's spawned_blocking task finishes harmlessly; no panic,
    // no poison. (Requires tokio "time" dev-feature — see header note.)
    let (_g, root) = fixture_tree();
    let (_sg, store) = file_store();
    let to = TransferOptions::default();

    // Tiny budget so the op is very likely cancelled mid-flight; either outcome
    // (Ok = finished in time, Err = timed out) is acceptable — what matters is
    // the runtime survives.
    let _ = tokio::time::timeout(
        Duration::from_nanos(1),
        snapdir_api::push(PushSource::Path(root.as_path()), &store, &to),
    )
    .await;

    // Runtime must remain usable after the (likely) cancellation.
    let (_g2, root2) = fixture_tree();
    let (_sg2, store2) = file_store();
    let id = snapdir_api::push(PushSource::Path(root2.as_path()), &store2, &to)
        .await
        .expect("runtime usable after a timeout-cancelled future");
    snapdir_api::verify(&id, &store2, &VerifyOptions::default())
        .await
        .expect("verify after cancellation");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_drop_and_cancel_then_full_roundtrip() {
    // §7.2: stress the cancellation path — repeatedly create+drop and
    // timeout-cancel futures, then prove a FULL round-trip still works end to
    // end. A per-call-runtime or a poisoned shared runtime would fail here.
    let to = TransferOptions::default();
    for _ in 0..8 {
        let (_g, root) = fixture_tree();
        let (_sg, store) = file_store();
        drop(snapdir_api::push(
            PushSource::Path(root.as_path()),
            &store,
            &to,
        ));
        let (_g2, root2) = fixture_tree();
        let (_sg2, store2) = file_store();
        let _ = tokio::time::timeout(
            Duration::from_nanos(1),
            snapdir_api::fetch(
                &snapdir_api::id(root2.as_path(), &ManifestOptions::default()).expect("id"),
                &store2,
                &to,
            ),
        )
        .await;
    }

    let (_g, root) = fixture_tree();
    let (_sg, store) = file_store();
    let id = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
        .await
        .expect("push after drop/cancel stress");
    snapdir_api::fetch(&id, &store, &to).await.expect("fetch");
    let dest = tempfile::tempdir().expect("dest");
    snapdir_api::pull(&id, &store, dest.path(), &CheckoutOptions::default())
        .await
        .expect("pull after stress");
    let reid = snapdir_api::id(dest.path(), &ManifestOptions::default()).expect("re-id");
    assert_eq!(reid, id, "full round-trip survives drop/cancel stress");
}

// ===========================================================================
// CONCURRENCY / NO DEADLOCK — §7 (spawn_blocking over the shared runtime).
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_fetches_against_distinct_stores_complete() {
    // §7: N concurrent fetches (each its own pushed store) all complete — no
    // deadlock, no runtime exhaustion. Distinct stores so there is no shared
    // file-lock contention masking a concurrency bug.
    const N: usize = 12;
    let mut tasks = Vec::with_capacity(N);
    for _ in 0..N {
        tasks.push(tokio::spawn(async move {
            let (g, root) = fixture_tree();
            let (sg, store) = file_store();
            let to = TransferOptions::default();
            let id = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
                .await
                .expect("push");
            snapdir_api::fetch(&id, &store, &to)
                .await
                .expect("concurrent fetch");
            drop((g, sg));
        }));
    }
    for t in tasks {
        t.await.expect("join fetch task");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn many_concurrent_manifest_like_async_calls_on_small_worker_pool() {
    // §7.1: even with only 2 worker threads, 16 concurrent async ops complete.
    // A facade that block_on'd inline (instead of spawn_blocking) would starve
    // the 2-thread reactor and deadlock. Use verify (read-only async) for speed.
    const N: usize = 16;
    let (_g, root) = fixture_tree();
    let (_sg, store) = file_store();
    let to = TransferOptions::default();
    let id = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
        .await
        .expect("seed push");

    let mut tasks = Vec::with_capacity(N);
    for _ in 0..N {
        let store = store.clone();
        let id = id;
        tasks.push(tokio::spawn(async move {
            snapdir_api::verify(&id, &store, &VerifyOptions::default())
                .await
                .expect("concurrent verify")
                .ok
        }));
    }
    for t in tasks {
        assert!(t.await.expect("join verify"), "each concurrent verify ok");
    }
}

// ===========================================================================
// ERROR MAPPING — §1/§7 (typed SnapdirError, never a panic).
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fetch_missing_id_yields_typed_error_not_panic() {
    // §1/§7: an async op against a store missing the requested id yields a typed
    // SnapdirError (not a panic / not Ok). The empty store has no objects for a
    // synthetic all-zero id.
    let (_sg, store) = file_store();
    let to = TransferOptions::default();
    let bogus = SnapshotId::from_hex(&"0".repeat(64)).expect("all-zero id parses");

    let err = snapdir_api::fetch(&bogus, &store, &to)
        .await
        .expect_err("fetch of a missing id must be a typed error");
    // code() must be one of the documented stable codes — not an empty/garbage.
    let code = err.code();
    assert!(
        matches!(
            code,
            "STORE_ERROR" | "IO_ERROR" | "CATALOG_ERROR" | "HASH_MISMATCH" | "INVALID_ID"
        ),
        "missing-id fetch maps to a documented error code, got {code:?}: {err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn verify_missing_id_is_typed_error_or_not_ok() {
    // §1/§7: verify against a store with no such id must NOT panic — it either
    // returns a typed SnapdirError or a VerifyResult with ok == false. Either is
    // an acceptable contract; a panic is not.
    let (_sg, store) = file_store();
    let bogus = SnapshotId::from_hex(&"f".repeat(64)).expect("all-f id parses");
    match snapdir_api::verify(&bogus, &store, &VerifyOptions::default()).await {
        Ok(vr) => assert!(!vr.ok, "verify of a missing id is not ok"),
        Err(e) => {
            let _typed: &SnapdirError = &e;
            assert!(!e.code().is_empty(), "typed error carries a code");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_to_unwritable_store_path_yields_typed_error() {
    // §1/§7: push whose file:// store points at a path that cannot be created
    // (a regular FILE used where a dir must live) yields a typed SnapdirError,
    // never a panic. The reactor/runtime stays usable afterward.
    let td = tempfile::tempdir().expect("tempdir");
    let file_path = td.path().join("not-a-dir");
    std::fs::write(&file_path, b"x").unwrap(); // occupy the path with a file
    let nested = format!("file://{}/store", file_path.display());
    let store = StoreUri::parse(&nested).expect("uri parses (scheme valid)");

    let (_g, root) = fixture_tree();
    let to = TransferOptions::default();
    let res = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to).await;
    assert!(
        res.is_err(),
        "push into a file-occupied store path must error"
    );
    let _typed: SnapdirError = res.unwrap_err();

    // Runtime survives the error: a fresh push to a good store still works.
    let (_g2, root2) = fixture_tree();
    let (_sg2, good) = file_store();
    snapdir_api::push(PushSource::Path(root2.as_path()), &good, &to)
        .await
        .expect("runtime usable after a typed push error");
}

// ===========================================================================
// SYNC STAYS SYNC — §6/§2/§7.3 (manifest/id/stage callable WITHOUT a runtime).
// These are plain `#[test]`s (NO tokio runtime in scope), proving the sync fns
// do not require / spin up any async machinery.
// ===========================================================================

#[test]
fn id_is_callable_outside_any_runtime() {
    // §6/§2: id() is SYNC — callable from a plain #[test] with no tokio runtime.
    let (_g, root) = fixture_tree();
    let id = snapdir_api::id(root.as_path(), &ManifestOptions::default())
        .expect("id() works with no runtime present");
    assert!(is_64_lower_hex(&id.to_hex()));
}

#[test]
fn manifest_is_callable_outside_any_runtime() {
    // §6/§2: manifest() is SYNC — no runtime required.
    let (_g, root) = fixture_tree();
    let m = snapdir_api::manifest(root.as_path(), &ManifestOptions::default())
        .expect("manifest() works with no runtime present");
    assert!(
        !m.entries.is_empty(),
        "manifest has entries for a real tree"
    );
    assert!(!m.raw.is_empty(), "manifest carries its rendered raw text");
}

#[test]
fn stage_is_callable_outside_any_runtime_and_matches_id() {
    // §6/§2: stage() is SYNC and returns the same id as id() — no runtime needed.
    let (_g, root) = fixture_tree();
    let staged = snapdir_api::stage(root.as_path(), &StageOptions::default())
        .expect("stage() works with no runtime present");
    let computed = snapdir_api::id(root.as_path(), &ManifestOptions::default()).expect("id");
    assert_eq!(staged, computed, "stage()==id(), both sync, no runtime");
}

#[test]
fn id_from_manifest_is_pure_sync_and_consistent() {
    // §6: id_from_manifest(&Manifest) is pure/infallible/sync and equals id().
    let (_g, root) = fixture_tree();
    let m = snapdir_api::manifest(root.as_path(), &ManifestOptions::default()).expect("manifest");
    let from_m: SnapshotId = snapdir_api::id_from_manifest(&m);
    let direct = snapdir_api::id(root.as_path(), &ManifestOptions::default()).expect("id");
    assert_eq!(from_m, direct, "id_from_manifest == id, pure & sync");
}

#[test]
fn sync_fns_do_not_require_async_for_empty_tree_edge_case() {
    // §6/§2 edge: an EMPTY directory still ids synchronously (degenerate input),
    // proving the sync path has no hidden async dependency even at the boundary.
    let td = tempfile::tempdir().expect("tempdir");
    let _: SnapshotId =
        snapdir_api::id(td.path(), &ManifestOptions::default()).expect("id of empty tree (sync)");
    let path: &Path = td.path();
    let _ =
        snapdir_api::manifest(path, &ManifestOptions::default()).expect("manifest of empty tree");
}

// ===========================================================================
// REVIEW-GATE STRENGTHENING (gate: m0-async-facade-tests-review, phase 34).
//
// Added now that the runtime wiring in crates/snapdir-api/src/lib.rs is
// VISIBLE. The impl gate made NO src changes — the existing async impl already
// routes every async fn through `tokio::task::spawn_blocking` over a process-
// wide shared `OnceLock<tokio::runtime::Runtime>` (`shared_runtime()`), with no
// per-call `Runtime::new().block_on()` anywhere. These cases HARDEN §7 along the
// axes the impl reveals: runtime-singleton observability (indirect — the
// OnceLock is `pub(crate)`, so it is proven by behavior, not by handle
// identity), spawn_blocking non-stall of the reactor, deeper cancellation /
// high-concurrency edges, and error-after-error runtime survival.
//
// Respecting the adversary's own F1–F4 spec flags: cancellation here pins
// RUNTIME-SURVIVAL (a later op + a full round-trip still work), NOT side-effect
// rollback (spawn_blocking tasks are not abortable — §7 does not promise
// rollback). Error cases assert only that a typed `SnapdirError` with a
// documented `code()` surfaces and the runtime stays usable — never a specific
// unpinned variant (F2/F3).
// ===========================================================================

// ---------------------------------------------------------------------------
// RUNTIME-SINGLETON OBSERVABILITY — §7.1 ("ONE shared runtime, never per-call").
// The OnceLock runtime is crate-private, so singleton-ness is proven INDIRECTLY:
// a per-call `Runtime::new()` would (a) panic the moment a facade call is made
// from inside a runtime ("Cannot start a runtime from within a runtime") and
// (b) accumulate threads/handles without bound. A single shared runtime
// survives a very high call volume from many nested async contexts cheaply.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn high_volume_calls_from_nested_async_contexts_never_recreate_a_runtime() {
    // §7.1: 64 async ops, each launched from its OWN nested `tokio::spawn`
    // (a distinct async context), all complete with no "runtime within a
    // runtime" panic. If each call constructed its own `Runtime`, the first
    // nested `block_on` would panic; if it leaked a Runtime per call, this
    // many would balloon thread count. One shared runtime makes it trivial.
    const N: usize = 64;
    let mut tasks = Vec::with_capacity(N);
    for _ in 0..N {
        tasks.push(tokio::spawn(async {
            // Nest one MORE level so the facade is reached from a doubly-nested
            // async context — still must not spin a fresh runtime.
            tokio::spawn(async {
                let (g, root) = fixture_tree();
                let (sg, store) = file_store();
                let to = TransferOptions::default();
                let id = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
                    .await
                    .expect("nested-context push reuses the shared runtime");
                let ok = snapdir_api::verify(&id, &store, &VerifyOptions::default())
                    .await
                    .expect("nested-context verify")
                    .ok;
                drop((g, sg));
                ok
            })
            .await
            .expect("inner join")
        }));
    }
    for t in tasks {
        assert!(
            t.await.expect("outer join"),
            "every nested-context op verifies ok"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_runtime_survives_many_blocking_calls_from_a_blocking_thread() {
    // §7.1 / §7.3: drive the facade from INSIDE a `spawn_blocking` thread that
    // itself uses `Handle::current().block_on(...)`. This is the exact shape a
    // SYNC language binding takes (block on an async facade fn from a worker
    // thread). A per-call `Runtime::new()` would panic ("Cannot start a runtime
    // from within a runtime" is avoided by block_on-on-a-handle, but a *new*
    // Runtime's drop in a blocking context is itself forbidden) — the shared
    // runtime + spawn_blocking design tolerates it. Do it repeatedly to prove
    // no per-call runtime is created/dropped under the hood.
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        for _ in 0..8 {
            let (g, root) = fixture_tree();
            let (sg, store) = file_store();
            let to = TransferOptions::default();
            let id = handle
                .block_on(snapdir_api::push(
                    PushSource::Path(root.as_path()),
                    &store,
                    &to,
                ))
                .expect("block_on push from a blocking thread");
            let ok = handle
                .block_on(snapdir_api::verify(&id, &store, &VerifyOptions::default()))
                .expect("block_on verify from a blocking thread")
                .ok;
            assert!(ok, "round-trip ok when driven via block_on from a worker");
            drop((g, sg));
        }
    })
    .await
    .expect("blocking-thread driver completes");
}

// ---------------------------------------------------------------------------
// SPAWN_BLOCKING NON-STALL — §7.1 ("the reactor never blocks").
// The whole point of spawn_blocking: a slow/CPU-heavy blocking facade op must
// NOT stall the reactor — other async work (timers, other facade ops) makes
// progress concurrently. Proven by running a reactor-only timer loop alongside
// an in-flight facade op and asserting the timer ticked while the op ran.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_blocking_op_does_not_stall_the_reactor() {
    // §7.1: push a LARGE tree (many objects -> a genuinely slow blocking task)
    // while a pure-reactor timer loop runs concurrently. If the facade blocked
    // the reactor inline (instead of spawn_blocking), the timer could not tick
    // until the push finished. We assert the timer made MULTIPLE ticks before
    // the push completed -> the reactor stayed live during the blocking op.
    let td = tempfile::tempdir().expect("tempdir");
    let root = td.path().to_path_buf();
    // ~200 small files: enough hashing+IO that the push clearly outlasts a few
    // 1ms timer ticks, without making the test slow.
    for i in 0..200 {
        std::fs::write(
            root.join(format!("f{i:04}.bin")),
            vec![(i % 251) as u8; 4096],
        )
        .unwrap();
    }
    let (_sg, store) = file_store();
    let to = TransferOptions::default();

    let ticks = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let ticks_bg = ticks.clone();
    // A reactor-only ticker: it can ONLY advance if the reactor is not blocked.
    let ticker = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(1));
        loop {
            interval.tick().await;
            ticks_bg.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    });

    let id = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
        .await
        .expect("slow push completes");
    let observed = ticks.load(std::sync::atomic::Ordering::SeqCst);
    ticker.abort();

    // The reactor ticked while the blocking push ran. Even on a 2-worker pool,
    // a non-blocking (spawn_blocking) impl yields many ticks; an inline-blocking
    // impl on the SAME thread as the ticker would yield ~0–1. Require >= 2 to be
    // robust against scheduling jitter while still failing a stalling impl.
    assert!(
        observed >= 2,
        "reactor must keep ticking during a slow blocking op (got {observed} ticks); \
         a spawn_blocking impl does not stall the reactor"
    );
    // And the op itself succeeded.
    let vr = snapdir_api::verify(&id, &store, &VerifyOptions::default())
        .await
        .expect("verify the slow-pushed snapshot");
    assert!(vr.ok);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_async_op_makes_progress_while_a_blocking_op_is_in_flight() {
    // §7.1: while one slow push is in flight, a SECOND independent facade op
    // (push to a different store) also completes — concurrently, not serialized
    // behind the first. On a 2-worker pool an inline-block impl would pin a
    // worker and could not interleave two blocking facade calls under join!.
    let td = tempfile::tempdir().expect("tempdir");
    let big = td.path().to_path_buf();
    for i in 0..150 {
        std::fs::write(big.join(format!("b{i:04}.bin")), vec![1u8; 4096]).unwrap();
    }
    let (_sg1, store1) = file_store();
    let (_g2, small) = fixture_tree();
    let (_sg2, store2) = file_store();
    let to = TransferOptions::default();

    let (a, b) = tokio::join!(
        snapdir_api::push(PushSource::Path(big.as_path()), &store1, &to),
        snapdir_api::push(PushSource::Path(small.as_path()), &store2, &to),
    );
    let id_a = a.expect("big push under join");
    let id_b = b.expect("small push under join (made progress alongside the big one)");
    assert!(is_64_lower_hex(&id_a.to_hex()));
    assert!(is_64_lower_hex(&id_b.to_hex()));
}

// ---------------------------------------------------------------------------
// DEEPER CANCELLATION / HIGH-CONCURRENCY EDGES — §7.1/§7.2.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thirtytwo_concurrent_pushes_to_distinct_stores_no_deadlock() {
    // §7.1: 32 simultaneous push+fetch+verify chains against DISTINCT stores —
    // double the staged suite's concurrency. No deadlock, no runtime exhaustion,
    // every chain re-ids/verifies. Distinct stores so a real concurrency bug
    // cannot hide behind file-lock contention.
    const N: usize = 32;
    let mut tasks = Vec::with_capacity(N);
    for _ in 0..N {
        tasks.push(tokio::spawn(async {
            let (g, root) = fixture_tree();
            let (sg, store) = file_store();
            let to = TransferOptions::default();
            let id = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
                .await
                .expect("concurrent push (32-way)");
            snapdir_api::fetch(&id, &store, &to)
                .await
                .expect("concurrent fetch (32-way)");
            let ok = snapdir_api::verify(&id, &store, &VerifyOptions::default())
                .await
                .expect("concurrent verify (32-way)")
                .ok;
            drop((g, sg));
            (id, ok)
        }));
    }
    for t in tasks {
        let (id, ok) = t.await.expect("join 32-way task");
        assert!(is_64_lower_hex(&id.to_hex()));
        assert!(ok, "each of 32 concurrent chains verifies ok");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_multiple_inflight_ops_then_full_roundtrip_still_works() {
    // §7.2: start MANY async ops concurrently, then cancel them all (abort the
    // spawned tasks / let tight timeouts elapse) while they are in flight, and
    // prove the shared runtime + a full round-trip survive. Pins runtime-
    // SURVIVAL after multi-cancellation (per F1: not rollback).
    let to = TransferOptions::default();

    // (a) Spawn 16 ops then abort them mid-flight.
    let mut handles = Vec::new();
    for _ in 0..16 {
        let to = to.clone();
        handles.push(tokio::spawn(async move {
            let (g, root) = fixture_tree();
            let (sg, store) = file_store();
            let r = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to).await;
            drop((g, sg));
            r.map(|_| ())
        }));
    }
    // Abort every in-flight task immediately (cancellation of the future).
    for h in &handles {
        h.abort();
    }
    // Joining aborted tasks yields Cancelled or a completed Ok/Err — either is
    // fine; what matters is no panic and the runtime survives.
    for h in handles {
        let _ = h.await; // Err(JoinError::cancelled) is acceptable.
    }

    // (b) Also fire tight-timeout cancellations against several ops.
    for _ in 0..8 {
        let (_g, root) = fixture_tree();
        let (_sg, store) = file_store();
        let _ = tokio::time::timeout(
            Duration::from_nanos(1),
            snapdir_api::push(PushSource::Path(root.as_path()), &store, &to),
        )
        .await;
    }

    // (c) The runtime must be fully usable: a complete push->fetch->pull chain.
    let (_g, root) = fixture_tree();
    let (_sg, store) = file_store();
    let src_id = snapdir_api::id(root.as_path(), &ManifestOptions::default()).expect("src id");
    let id = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
        .await
        .expect("push after mass-cancellation");
    snapdir_api::fetch(&id, &store, &to)
        .await
        .expect("fetch after mass-cancellation");
    let dest = tempfile::tempdir().expect("dest");
    snapdir_api::pull(&id, &store, dest.path(), &CheckoutOptions::default())
        .await
        .expect("pull after mass-cancellation");
    let reid = snapdir_api::id(dest.path(), &ManifestOptions::default()).expect("re-id");
    assert_eq!(reid, src_id, "full round-trip survives mass cancellation");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_and_immediately_drop_futures_in_a_tight_loop_no_leak_or_poison() {
    // §7.2: build a facade future and drop it WITHOUT awaiting, repeatedly in a
    // tight loop (256x). Each dropped future's spawn_blocking task must finish
    // harmlessly; nothing accumulates that poisons the shared runtime. After the
    // loop, an async op + verify must still succeed.
    let to = TransferOptions::default();
    for _ in 0..256 {
        let (_g, root) = fixture_tree();
        let (_sg, store) = file_store();
        // Construct then drop the future immediately (never polled).
        let fut = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to);
        drop(fut);
    }
    // Runtime still healthy.
    let (_g, root) = fixture_tree();
    let (_sg, store) = file_store();
    let id = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
        .await
        .expect("push after 256 dropped futures");
    let vr = snapdir_api::verify(&id, &store, &VerifyOptions::default())
        .await
        .expect("verify after 256 dropped futures");
    assert!(vr.ok, "runtime not poisoned by repeated dropped futures");
}

// ---------------------------------------------------------------------------
// ERROR-AFTER-ERROR — §1/§7 (two consecutive failing async ops both yield a
// typed SnapdirError and leave the runtime usable).
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_consecutive_failing_async_ops_both_typed_then_runtime_usable() {
    // §1/§7: two back-to-back failing async ops each surface a typed
    // SnapdirError with a documented code (never a panic), and the runtime is
    // still fully usable for a healthy round-trip afterward. Per F2/F3 we do NOT
    // pin the exact variant — only that code() is one of the documented codes.
    let (_sg, empty) = file_store();
    let to = TransferOptions::default();
    let bogus1 = SnapshotId::from_hex(&"0".repeat(64)).expect("zero id");
    let bogus2 = SnapshotId::from_hex(&"1".repeat(64)).expect("one id");

    fn assert_documented_code(e: &SnapdirError) {
        let code = e.code();
        assert!(
            matches!(
                code,
                "STORE_ERROR"
                    | "IO_ERROR"
                    | "CATALOG_ERROR"
                    | "HASH_MISMATCH"
                    | "IN_FLUX"
                    | "INVALID_ID"
                    | "INVALID_STORE"
                    | "CONFLICT"
            ),
            "failing async op maps to a documented error code, got {code:?}: {e}"
        );
    }

    // Error #1.
    let e1 = snapdir_api::fetch(&bogus1, &empty, &to)
        .await
        .expect_err("first failing fetch is a typed error");
    assert_documented_code(&e1);
    // Error #2 (consecutive) — runtime must not be poisoned by the first error.
    let e2 = snapdir_api::fetch(&bogus2, &empty, &to)
        .await
        .expect_err("second consecutive failing fetch is a typed error");
    assert_documented_code(&e2);

    // Runtime still usable: a real push->verify round-trip succeeds.
    let (_g, root) = fixture_tree();
    let (_sg2, good) = file_store();
    let id = snapdir_api::push(PushSource::Path(root.as_path()), &good, &to)
        .await
        .expect("runtime usable after two consecutive async errors");
    let vr = snapdir_api::verify(&id, &good, &VerifyOptions::default())
        .await
        .expect("verify after error-after-error");
    assert!(vr.ok);
}
