// Regression test for gate api-checkout-restore-perms (Phase 37).
//
// Proves that `pull` and `checkout` restore the permission modes recorded in
// the manifest after materializing the file tree.  Without the fix the dest
// root stays at the pre-existing mode (e.g. 0o700) so the materialized tree
// re-manifests to a DIFFERENT snapshot id (perms are part of the manifest text
// the id hashes).  With the fix the re-id matches the pushed id.

#![allow(clippy::doc_markdown)]

use std::os::unix::fs::PermissionsExt;

use snapdir_api::{
    CheckoutOptions, ManifestOptions, PushSource, StageOptions, StoreUri, TransferOptions,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A small, deterministic directory tree in a fresh temp dir.
fn fixture_tree() -> (tempfile::TempDir, std::path::PathBuf) {
    let td = tempfile::tempdir().expect("tempdir");
    let root = td.path().to_path_buf();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("a.txt"), b"hello perm fix\n").unwrap();
    std::fs::write(root.join("sub/b.bin"), vec![0xab_u8; 512]).unwrap();
    (td, root)
}

/// A `file://` StoreUri pointing at a fresh temp dir.
fn file_store() -> (tempfile::TempDir, StoreUri) {
    let td = tempfile::tempdir().expect("tempdir");
    let uri = format!("file://{}", td.path().display());
    let store = StoreUri::parse(&uri).expect("parse file:// store uri");
    (td, store)
}

// ---------------------------------------------------------------------------
// `pull` regression
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pull_into_restricted_dest_restores_perms_and_reids_correctly() {
    // Push a small tree, pull into a pre-existing dest at mode 0o700.
    // After pull the tree must re-manifest to the original id — proving that
    // `restore_permissions` applied the manifest modes on the dest root.
    let (_g, root) = fixture_tree();
    let (_sg, store) = file_store();
    let to = TransferOptions::default();

    let src_id = snapdir_api::id(root.as_path(), &ManifestOptions::default()).expect("source id");
    let pushed = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
        .await
        .expect("push");
    assert_eq!(pushed, src_id);

    // Create dest with a restrictive mode so the root dir stays 0o700 without the fix.
    let dest_td = tempfile::tempdir().expect("dest tempdir");
    std::fs::set_permissions(dest_td.path(), std::fs::Permissions::from_mode(0o700))
        .expect("set dest mode to 0o700");

    snapdir_api::pull(&pushed, &store, dest_td.path(), &CheckoutOptions::default())
        .await
        .expect("pull");

    let reid = snapdir_api::id(dest_td.path(), &ManifestOptions::default()).expect("re-id pulled");
    assert_eq!(reid, src_id, "pulled tree must re-id to the source id");
}

// ---------------------------------------------------------------------------
// `checkout` regression
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn checkout_into_restricted_dest_restores_perms_and_reids_correctly() {
    // Stage+push then fetch into the local cache, then checkout into a
    // pre-existing dest at mode 0o700.  The re-id must match the source.
    let (_g, root) = fixture_tree();
    let (_sg, store) = file_store();
    let to = TransferOptions::default();

    let src_id = snapdir_api::id(root.as_path(), &ManifestOptions::default()).expect("source id");
    let staged = snapdir_api::stage(root.as_path(), &StageOptions::default()).expect("stage");
    assert_eq!(staged, src_id);

    let pushed = snapdir_api::push(PushSource::StagedId(&staged), &store, &to)
        .await
        .expect("push staged");
    assert_eq!(pushed, src_id);

    // Fetch into the local cache so checkout can find the objects.
    snapdir_api::fetch(&pushed, &store, &to)
        .await
        .expect("fetch into cache");

    // Dest starts at mode 0o700.
    let dest_td = tempfile::tempdir().expect("dest tempdir");
    std::fs::set_permissions(dest_td.path(), std::fs::Permissions::from_mode(0o700))
        .expect("set dest mode to 0o700");

    snapdir_api::checkout(&pushed, dest_td.path(), &CheckoutOptions::default())
        .await
        .expect("checkout");

    let reid =
        snapdir_api::id(dest_td.path(), &ManifestOptions::default()).expect("re-id checked-out");
    assert_eq!(reid, src_id, "checked-out tree must re-id to the source id");
}
