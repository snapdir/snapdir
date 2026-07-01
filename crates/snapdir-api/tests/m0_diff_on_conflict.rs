//! Focused tests for the `m0-diff-on-conflict-honor` gate.
//!
//! Proves that `DiffOptions::on_conflict` is actually honoured by `diff()`:
//!
//! - `ConflictPolicy::Error` → intra-side collision returns
//!   `Err(SnapdirError::Conflict)` with `.code() == "CONFLICT"`.
//! - `ConflictPolicy::LastWins` → resolves deterministically to the LAST
//!   colliding URI's entry.
//! - No-collision diffs (single store per side, or same-content multi-store)
//!   are behaviour-identical to before.

use std::path::PathBuf;

use snapdir_api::{ConflictPolicy, DiffOptions, DiffStatus, SnapdirError, StageOptions, StoreUri};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Writes a minimal tree with the given file entries to `root`.
fn write_tree(root: &std::path::Path, files: &[(&str, &[u8])]) {
    std::fs::create_dir_all(root).unwrap();
    for (name, content) in files {
        std::fs::write(root.join(name), content).unwrap();
    }
}

/// Stages `root` into `cache_dir` and returns a `file://`-scheme `StoreUri`
/// pointing at `cache_dir` (where the manifest + objects were written).
fn stage_into_store(root: &std::path::Path, cache_dir: &std::path::Path) -> StoreUri {
    let opts = StageOptions {
        cache_dir: Some(cache_dir.to_path_buf()),
        keep: true,
    };
    snapdir_api::stage(root, &opts).expect("stage must succeed");
    let uri = format!("file://{}", cache_dir.display());
    StoreUri::parse(&uri).expect("file:// URI must be valid")
}

// ---------------------------------------------------------------------------
// Core collision tests
// ---------------------------------------------------------------------------

/// `ConflictPolicy::Error` (the default) → an intra-side path collision on
/// the FROM side must surface as `Err` whose `.code()` is `"CONFLICT"`.
#[tokio::test]
async fn on_conflict_error_returns_conflict_code() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();

    // Two FROM stores both carrying "clash.txt" but with DIFFERENT content.
    let from1_src = base.join("from1_src");
    write_tree(&from1_src, &[("clash.txt", b"left version")]);
    let from1_store = base.join("from1_store");
    let from1_uri = stage_into_store(&from1_src, &from1_store);

    let from2_src = base.join("from2_src");
    write_tree(&from2_src, &[("clash.txt", b"right version")]);
    let from2_store = base.join("from2_store");
    let from2_uri = stage_into_store(&from2_src, &from2_store);

    // TO side is unambiguous (no collision there).
    let to_src = base.join("to_src");
    write_tree(&to_src, &[("z.txt", b"z")]);
    let to_store = base.join("to_store");
    let to_uri = stage_into_store(&to_src, &to_store);

    let opts = DiffOptions {
        from: vec![from1_uri, from2_uri],
        to: vec![to_uri],
        on_conflict: ConflictPolicy::Error, // explicit (also the default)
        ..DiffOptions::default()
    };

    let result = snapdir_api::diff(&opts).await;
    assert!(
        result.is_err(),
        "an intra-side collision with ConflictPolicy::Error must return Err; got Ok"
    );
    let err = result.unwrap_err();
    assert_eq!(
        err.code(),
        "CONFLICT",
        "the stable error code must be CONFLICT; got {:?}",
        err.code()
    );
    // The error message must name the colliding path.
    let msg = err.to_string();
    assert!(
        msg.contains("clash.txt"),
        "the error message must name the colliding path; got: {msg}"
    );
}

/// `ConflictPolicy::Error` is the default value — calling `diff()` with
/// `DiffOptions::default()` (which has `on_conflict: ConflictPolicy::Error`)
/// must also error on collision.
#[tokio::test]
async fn on_conflict_error_is_the_default() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();

    let from1_src = base.join("f1s");
    write_tree(&from1_src, &[("file.txt", b"version A")]);
    let from1_store = base.join("f1st");
    let from1_uri = stage_into_store(&from1_src, &from1_store);

    let from2_src = base.join("f2s");
    write_tree(&from2_src, &[("file.txt", b"version B")]);
    let from2_store = base.join("f2st");
    let from2_uri = stage_into_store(&from2_src, &from2_store);

    let to_src = base.join("ts");
    write_tree(&to_src, &[("other.txt", b"other")]);
    let to_store = base.join("tst");
    let to_uri = stage_into_store(&to_src, &to_store);

    // Use all-default options: on_conflict defaults to ConflictPolicy::Error.
    let opts = DiffOptions {
        from: vec![from1_uri, from2_uri],
        to: vec![to_uri],
        ..DiffOptions::default()
    };

    let result = snapdir_api::diff(&opts).await;
    assert!(
        result.is_err(),
        "default DiffOptions must treat intra-side collision as error; got Ok"
    );
    assert_eq!(result.unwrap_err().code(), "CONFLICT");
}

/// `ConflictPolicy::LastWins` → resolves to the LAST colliding URI's entry
/// deterministically.  If FROM = [store_A(clash=LEFT), store_B(clash=RIGHT)],
/// then last-wins resolves FROM.clash to RIGHT.  If TO also carries clash=RIGHT,
/// the diff is EMPTY (paths equal → hidden).  If TO carries clash=LEFT, the diff
/// shows M (FROM resolved to RIGHT, TO has LEFT → Modified).
#[tokio::test]
async fn on_conflict_last_wins_selects_last_uri() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();

    let from1_src = base.join("lw_f1s");
    write_tree(&from1_src, &[("clash.txt", b"LEFT loses")]);
    let from1_store = base.join("lw_f1st");
    let from1_uri = stage_into_store(&from1_src, &from1_store);

    // from2 is the LAST from store → its content wins.
    let from2_src = base.join("lw_f2s");
    write_tree(&from2_src, &[("clash.txt", b"RIGHT-WINS")]);
    let from2_store = base.join("lw_f2st");
    let from2_uri = stage_into_store(&from2_src, &from2_store);

    // TO carries the WINNING (last-from) content → diff must be empty.
    let to_src_match = base.join("lw_ts_match");
    write_tree(&to_src_match, &[("clash.txt", b"RIGHT-WINS")]);
    let to_store_match = base.join("lw_tst_match");
    let to_uri_match = stage_into_store(&to_src_match, &to_store_match);

    let opts = DiffOptions {
        from: vec![from1_uri.clone(), from2_uri.clone()],
        to: vec![to_uri_match],
        on_conflict: ConflictPolicy::LastWins,
        ..DiffOptions::default()
    };
    let entries = snapdir_api::diff(&opts)
        .await
        .expect("last-wins must not error");
    assert!(
        entries
            .iter()
            .all(|e| e.path != PathBuf::from("./clash.txt")),
        "last-wins resolved FROM.clash=RIGHT-WINS == TO.clash → must be hidden; \
         got entries: {entries:?}"
    );

    // Cross-check: if TO carries the LOSING (first) content, last-wins yields M.
    let to_src_lose = base.join("lw_ts_lose");
    write_tree(&to_src_lose, &[("clash.txt", b"LEFT loses")]);
    let to_store_lose = base.join("lw_tst_lose");
    let to_uri_lose = stage_into_store(&to_src_lose, &to_store_lose);

    let opts2 = DiffOptions {
        from: vec![from1_uri, from2_uri],
        to: vec![to_uri_lose],
        on_conflict: ConflictPolicy::LastWins,
        ..DiffOptions::default()
    };
    let entries2 = snapdir_api::diff(&opts2)
        .await
        .expect("last-wins must not error");
    let clash_entry = entries2
        .iter()
        .find(|e| e.path == PathBuf::from("./clash.txt"));
    assert!(
        clash_entry.is_some(),
        "clash.txt must appear in the diff when FROM resolves to RIGHT and TO has LEFT"
    );
    assert_eq!(
        clash_entry.unwrap().status,
        DiffStatus::Modified,
        "clash.txt must be Modified (FROM=RIGHT, TO=LEFT)"
    );
}

// ---------------------------------------------------------------------------
// No-collision cases must be behaviour-identical
// ---------------------------------------------------------------------------

/// A single-store-per-side diff (no collision possible) must be unchanged from
/// before: `Added`, `Deleted`, `Modified`, `Unchanged` work as before.
#[tokio::test]
async fn no_collision_single_store_per_side_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();

    // FROM has a.txt and b.txt.
    let from_src = base.join("nc_from_src");
    write_tree(&from_src, &[("a.txt", b"aaa"), ("b.txt", b"bbb")]);
    let from_store = base.join("nc_from_st");
    let from_uri = stage_into_store(&from_src, &from_store);

    // TO has b.txt (same) and c.txt (new).  a.txt is gone.
    let to_src = base.join("nc_to_src");
    write_tree(&to_src, &[("b.txt", b"bbb"), ("c.txt", b"ccc")]);
    let to_store = base.join("nc_to_st");
    let to_uri = stage_into_store(&to_src, &to_store);

    let opts = DiffOptions {
        from: vec![from_uri],
        to: vec![to_uri],
        ..DiffOptions::default()
    };
    let entries = snapdir_api::diff(&opts)
        .await
        .expect("single-store diff must succeed");

    let find = |name: &str| {
        entries
            .iter()
            .find(|e| e.path == PathBuf::from(format!("./{name}")))
            .map(|e| e.status)
    };

    assert_eq!(
        find("a.txt"),
        Some(DiffStatus::Deleted),
        "a.txt must be Deleted"
    );
    assert_eq!(
        find("c.txt"),
        Some(DiffStatus::Added),
        "c.txt must be Added"
    );
    // b.txt is Unchanged — hidden by default (all=false).
    assert_eq!(
        find("b.txt"),
        None,
        "b.txt must be hidden (Unchanged, all=false)"
    );
}

/// Same-content collision (same path, SAME fingerprint on one side from multiple
/// stores) is NOT a conflict — it must succeed silently under ConflictPolicy::Error.
#[tokio::test]
async fn same_content_multi_store_not_a_collision() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();

    // Both FROM stores carry "shared.txt" with IDENTICAL content → not a collision.
    let from1_src = base.join("sc_f1s");
    write_tree(&from1_src, &[("shared.txt", b"same content")]);
    let from1_store = base.join("sc_f1st");
    let from1_uri = stage_into_store(&from1_src, &from1_store);

    let from2_src = base.join("sc_f2s");
    write_tree(&from2_src, &[("shared.txt", b"same content")]);
    let from2_store = base.join("sc_f2st");
    let from2_uri = stage_into_store(&from2_src, &from2_store);

    let to_src = base.join("sc_ts");
    write_tree(&to_src, &[("shared.txt", b"same content")]);
    let to_store = base.join("sc_tst");
    let to_uri = stage_into_store(&to_src, &to_store);

    let opts = DiffOptions {
        from: vec![from1_uri, from2_uri],
        to: vec![to_uri],
        on_conflict: ConflictPolicy::Error, // must NOT error for same-content
        ..DiffOptions::default()
    };
    let entries = snapdir_api::diff(&opts)
        .await
        .expect("same-content multi-store union must succeed (not a collision)");

    // shared.txt is identical on both sides → Unchanged → hidden (all=false).
    assert!(
        entries
            .iter()
            .all(|e| e.path != PathBuf::from("./shared.txt")),
        "shared.txt with identical content on both sides must be hidden; got: {entries:?}"
    );
}

/// TO-side collision also errors (the policy applies to BOTH sides independently).
#[tokio::test]
async fn on_conflict_error_applies_to_to_side() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();

    // FROM is simple.
    let from_src = base.join("ts_fsrc");
    write_tree(&from_src, &[("other.txt", b"other")]);
    let from_store = base.join("ts_fst");
    let from_uri = stage_into_store(&from_src, &from_store);

    // TO has two stores with different content for the same path → collision.
    let to1_src = base.join("ts_t1s");
    write_tree(&to1_src, &[("clash.txt", b"to-left")]);
    let to1_store = base.join("ts_t1st");
    let to1_uri = stage_into_store(&to1_src, &to1_store);

    let to2_src = base.join("ts_t2s");
    write_tree(&to2_src, &[("clash.txt", b"to-right")]);
    let to2_store = base.join("ts_t2st");
    let to2_uri = stage_into_store(&to2_src, &to2_store);

    let opts = DiffOptions {
        from: vec![from_uri],
        to: vec![to1_uri, to2_uri],
        on_conflict: ConflictPolicy::Error,
        ..DiffOptions::default()
    };
    let result = snapdir_api::diff(&opts).await;
    assert!(
        result.is_err(),
        "a collision on the TO side must also return Err under ConflictPolicy::Error"
    );
    assert_eq!(result.unwrap_err().code(), "CONFLICT");
}

/// The `SnapdirError::Conflict` variant's `.code()` returns `"CONFLICT"` (the
/// stable error-code contract frozen at M0, now first made triggerable).
#[test]
fn conflict_error_code_is_stable() {
    let err = SnapdirError::Conflict {
        message: "test".to_owned(),
    };
    assert_eq!(err.code(), "CONFLICT");
}
