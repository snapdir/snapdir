//! `snapdir_api::{manifest_size, size}` — logical vs physical (deduplicated)
//! byte sizes. Objects are stored uncompressed, so the manifest-derived physical
//! size MUST equal the on-disk `.objects/` bytes.

use std::path::{Path, PathBuf};

use snapdir_api::{ManifestOptions, PushSource, StoreUri, TransferOptions};

/// A tree with a duplicated file (shared object) + a unique file:
/// logical = 12 + 12 + 17 = 41; physical (deduped) = 12 + 17 = 29.
fn dup_tree() -> (tempfile::TempDir, PathBuf) {
    let td = tempfile::tempdir().expect("tempdir");
    let root = td.path().join("seed");
    std::fs::create_dir_all(root.join("a")).unwrap();
    std::fs::create_dir_all(root.join("b")).unwrap();
    std::fs::write(root.join("a/x.txt"), b"hello world\n").unwrap(); // 12
    std::fs::write(root.join("b/dup.txt"), b"hello world\n").unwrap(); // 12 (dup)
    std::fs::write(root.join("a/y.txt"), b"unique data here\n").unwrap(); // 17
    (td, root)
}

fn dir_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                total += dir_bytes(&p);
            } else {
                total += std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    total
}

#[test]
fn manifest_size_dedups_and_sums() {
    let (_td, root) = dup_tree();
    let m = snapdir_api::manifest(root.as_path(), &ManifestOptions::default()).unwrap();
    let s = snapdir_api::manifest_size(&m);
    assert_eq!(s.files, 3, "three file entries");
    assert_eq!(s.objects, 2, "two files share one object");
    assert_eq!(s.logical_bytes, 41, "12 + 12 + 17");
    assert_eq!(s.physical_bytes, 29, "12 + 17 (deduped)");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn store_size_matches_manifest_and_on_disk() {
    let (_td, root) = dup_tree();
    let store_td = tempfile::tempdir().expect("store tempdir");
    let store_path = store_td.path().join("store");
    let store = StoreUri::parse(&format!("file://{}", store_path.display())).unwrap();
    let to = TransferOptions::default();

    let id = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
        .await
        .expect("push");

    // A snapshot's size equals the local manifest's size.
    let m = snapdir_api::manifest(root.as_path(), &ManifestOptions::default()).unwrap();
    let by_id = snapdir_api::size(&store, Some(&id)).await.expect("size --id");
    assert_eq!(by_id, snapdir_api::manifest_size(&m));
    assert_eq!(by_id.physical_bytes, 29);
    assert_eq!(by_id.logical_bytes, 41);

    // Whole-store (one snapshot) equals the single-snapshot figure.
    let whole = snapdir_api::size(&store, None).await.expect("size whole store");
    assert_eq!(whole, by_id);

    // ACCEPTANCE: physical == on-disk object bytes (store is uncompressed).
    let on_disk = dir_bytes(&store_path.join(".objects"));
    assert_eq!(
        whole.physical_bytes, on_disk,
        "manifest-derived physical must equal on-disk .objects/ bytes"
    );
}
