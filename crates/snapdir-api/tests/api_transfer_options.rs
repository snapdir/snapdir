//! Public-contract regression tests: pull routes split stores and explicit caches;
//! checkout remains local, with no cross-tenant or default-cache fallback.
//! Authored independently before implementation; no target src was read.

use snapdir_api::{
    CheckoutOptions, ManifestOptions, PathType, SnapshotId, StageOptions, StoreUri, TransferOptions,
};
use std::fmt::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

// Every case runs in a fresh process: environment mutation in a multithreaded
// test runner is unsound, and tests must neither read nor change the user's cache.
fn isolated(name: &str) -> bool {
    if std::env::var("SNAPDIR_TRANSFER_TEST_CHILD").as_deref() == Ok(name) {
        return false;
    }
    let temp = tempfile::tempdir().unwrap();
    // A hermetic external adapter exercises the pre-existing direct-pull path.
    let bin = temp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let mock =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../snapdir-stores/tests/snapdir-mock-store");
    let adapter = bin.join("snapdir-ssh-store");
    let script = std::fs::read_to_string(mock)
        .unwrap()
        .replace("mock://", "ssh://");
    let (prefix, fetch) = script.split_once("\nget-fetch-files-command)\n").unwrap();
    // The stores-level fixture populates an object cache. For this API dispatch
    // regression, the mock fulfills Store::fetch_files by materializing paths.
    // Shipped emit-command adapters' object-cache-only behavior is a separate,
    // pre-existing API limitation (not changed by this transfer-options fix).
    let fetch = fetch
        .replace(
            r#"${cache_dir}/$(dirname "${rel}")"#,
            r#"${cache_dir}/$(dirname "${_path}")"#,
        )
        .replace("${cache_dir}/${rel}", "${cache_dir}/${_path}");
    std::fs::write(
        &adapter,
        format!("{prefix}\nget-fetch-files-command)\n{fetch}"),
    )
    .unwrap();
    std::fs::set_permissions(&adapter, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .env("PATH", std::env::join_paths(paths).unwrap())
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env("SNAPDIR_TRANSFER_TEST_CHILD", name)
        .env("HOME", temp.path().join("home"))
        .env("XDG_CACHE_HOME", temp.path().join("xdg"))
        .env("SNAPDIR_CACHE_DIR", temp.path().join("xdg/snapdir"))
        .env_remove("SNAPDIR_OBJECTS_STORE")
        .env_remove("SNAPDIR_STORE")
        .env_remove("SNAPDIR_CATALOG")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "isolated {name} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

fn run(future: impl std::future::Future<Output = ()>) {
    tokio::runtime::Runtime::new().unwrap().block_on(future);
}

fn uri(path: &Path) -> StoreUri {
    StoreUri::parse(&format!("file://{}", path.display())).unwrap()
}

fn shard(root: &Path, namespace: &str, hex: &str) -> PathBuf {
    root.join(namespace)
        .join(&hex[..3])
        .join(&hex[3..6])
        .join(&hex[6..9])
        .join(&hex[9..])
}

fn options(cache: &Path, objects: Option<&Path>) -> CheckoutOptions {
    CheckoutOptions {
        transfer: TransferOptions {
            cache_dir: Some(cache.to_path_buf()),
            objects_store: objects.map(uri),
            ..TransferOptions::default()
        },
        keep: true,
        ..CheckoutOptions::default()
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    source: PathBuf,
    store: PathBuf,
    pool: PathBuf,
    cache: PathBuf,
    dest: PathBuf,
    id: SnapshotId,
    object_hex: String,
}

impl Fixture {
    fn new(split: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let store = temp.path().join("manifest store");
        let pool = temp.path().join("object pool");
        let cache = temp.path().join("tenant cache");
        let dest = temp.path().join("destination");
        std::fs::create_dir_all(source.join("nested space/日本語")).unwrap();
        std::fs::write(source.join("payload.bin"), b"transfer-contract\0\xff\x01").unwrap();
        std::fs::write(source.join("nested space/日本語/empty"), b"").unwrap();
        std::fs::write(source.join("nested space/日本語/one"), b"x").unwrap();
        // Duplicate content pins object dedup without relying on traversal order.
        std::fs::write(source.join("duplicate.bin"), b"transfer-contract\0\xff\x01").unwrap();
        let manifest = snapdir_api::manifest(&source, &ManifestOptions::default()).unwrap();
        let entry = manifest
            .entries
            .iter()
            .find(|entry| {
                entry.path_type == PathType::File
                    && entry
                        .path
                        .file_name()
                        .is_some_and(|name| name == "payload.bin")
            })
            .unwrap();
        let mut object_hex = String::new();
        for byte in &entry.checksum {
            write!(object_hex, "{byte:02x}").unwrap();
        }
        let id = snapdir_api::stage(
            &source,
            &StageOptions {
                cache_dir: Some(store.clone()),
                keep: true,
            },
        )
        .unwrap();
        std::fs::create_dir_all(&pool).unwrap();
        if split {
            std::fs::rename(store.join(".objects"), pool.join(".objects")).unwrap();
        }
        Self {
            _temp: temp,
            source,
            store,
            pool,
            cache,
            dest,
            id,
            object_hex,
        }
    }

    fn assert_tree(&self, dest: &Path) {
        assert_eq!(
            std::fs::read(dest.join("payload.bin")).unwrap(),
            b"transfer-contract\0\xff\x01"
        );
        assert_eq!(
            std::fs::read(dest.join("duplicate.bin")).unwrap(),
            b"transfer-contract\0\xff\x01"
        );
        assert!(std::fs::read(dest.join("nested space/日本語/empty"))
            .unwrap()
            .is_empty());
        assert_eq!(
            std::fs::read(dest.join("nested space/日本語/one")).unwrap(),
            b"x"
        );
        assert_eq!(
            snapdir_api::id(dest, &ManifestOptions::default()).unwrap(),
            self.id
        );
    }
}

// SPEC: pull reads manifests and objects from distinct stores and retains its explicit cache for offline checkout.
#[test]
fn split_pull_populates_explicit_cache_and_checkout_is_offline() {
    if isolated("split_pull_populates_explicit_cache_and_checkout_is_offline") {
        return;
    }
    run(async {
        let f = Fixture::new(true);
        let opts = options(&f.cache, Some(&f.pool));
        snapdir_api::pull(&f.id, &uri(&f.store), &f.dest, &opts)
            .await
            .unwrap();
        f.assert_tree(&f.dest);
        assert!(shard(&f.cache, ".manifests", &f.id.to_hex()).is_file());
        assert_eq!(
            std::fs::read(shard(&f.cache, ".objects", &f.object_hex)).unwrap(),
            b"transfer-contract\0\xff\x01"
        );
        assert!(!f.store.join(".objects").exists());
        assert!(!f.pool.join(".manifests").exists());
        std::fs::remove_dir_all(&f.store).unwrap();
        std::fs::remove_dir_all(&f.pool).unwrap();
        let offline = f.dest.with_file_name("offline");
        snapdir_api::checkout(&f.id, &offline, &opts).await.unwrap();
        f.assert_tree(&offline);
    });
}

// SPEC: a colocated store still works with an explicit cache, and repeated pulls remain valid.
#[test]
fn colocated_pull_with_explicit_cache_is_repeatable() {
    if isolated("colocated_pull_with_explicit_cache_is_repeatable") {
        return;
    }
    run(async {
        let f = Fixture::new(false);
        let opts = options(&f.cache, None);
        for index in 0..2 {
            let dest = f.dest.with_file_name(format!("round-{index}"));
            snapdir_api::pull(&f.id, &uri(&f.store), &dest, &opts)
                .await
                .unwrap();
            f.assert_tree(&dest);
        }
        assert!(shard(&f.cache, ".manifests", &f.id.to_hex()).is_file());
    });
}

// SPEC: an explicit object store is authoritative; do not silently fall back to colocated objects.
#[test]
fn missing_explicit_pool_object_refuses_colocated_fallback() {
    if isolated("missing_explicit_pool_object_refuses_colocated_fallback") {
        return;
    }
    run(async {
        let f = Fixture::new(false);
        let result = snapdir_api::pull(
            &f.id,
            &uri(&f.store),
            &f.dest,
            &options(&f.cache, Some(&f.pool)),
        )
        .await;
        assert!(
            result.is_err(),
            "empty explicit pool must not borrow colocated objects"
        );
        assert!(!f.dest.join("payload.bin").exists());
    });
}

// SPEC: corrupted object bytes are refused even when a valid colocated copy exists.
#[test]
fn corrupt_explicit_pool_object_is_refused() {
    if isolated("corrupt_explicit_pool_object_is_refused") {
        return;
    }
    run(async {
        let f = Fixture::new(true);
        snapdir_api::stage(
            &f.source,
            &StageOptions {
                cache_dir: Some(f.store.clone()),
                keep: true,
            },
        )
        .unwrap();
        std::fs::write(shard(&f.pool, ".objects", &f.object_hex), b"corrupt").unwrap();
        let result = snapdir_api::pull(
            &f.id,
            &uri(&f.store),
            &f.dest,
            &options(&f.cache, Some(&f.pool)),
        )
        .await;
        assert!(result.is_err(), "corrupt explicit pool must not be ignored");
        assert!(!f.dest.join("payload.bin").exists());
    });
}

// SPEC: the manifest store remains authoritative; the object pool is not a manifest fallback.
#[test]
fn missing_manifest_refuses_pool_manifest_fallback() {
    if isolated("missing_manifest_refuses_pool_manifest_fallback") {
        return;
    }
    run(async {
        let f = Fixture::new(true);
        std::fs::rename(f.store.join(".manifests"), f.pool.join(".manifests")).unwrap();
        let result = snapdir_api::pull(
            &f.id,
            &uri(&f.store),
            &f.dest,
            &options(&f.cache, Some(&f.pool)),
        )
        .await;
        assert!(result.is_err());
        assert!(!f.dest.join("payload.bin").exists());
    });
}

// SPEC: checkout materializes exactly the explicit local cache, without needing any store.
#[test]
fn checkout_reads_explicit_staged_cache() {
    if isolated("checkout_reads_explicit_staged_cache") {
        return;
    }
    run(async {
        let f = Fixture::new(false);
        snapdir_api::checkout(&f.id, &f.dest, &options(&f.store, None))
            .await
            .unwrap();
        f.assert_tree(&f.dest);
    });
}

// SPEC: an unavailable tenant cache must never borrow the same snapshot from the default cache.
#[test]
fn explicit_missing_cache_refuses_default_cache_fallback() {
    if isolated("explicit_missing_cache_refuses_default_cache_fallback") {
        return;
    }
    run(async {
        let f = Fixture::new(false);
        snapdir_api::stage(&f.source, &StageOptions::default()).unwrap();
        let default_dest = f.dest.with_file_name("default-check");
        snapdir_api::checkout(&f.id, &default_dest, &CheckoutOptions::default())
            .await
            .unwrap();
        f.assert_tree(&default_dest);
        assert!(
            snapdir_api::checkout(&f.id, &f.dest, &options(&f.cache, None))
                .await
                .is_err()
        );
        assert!(!f.dest.join("payload.bin").exists());
    });
}

// SPEC: populating tenant A cannot make an empty tenant B succeed for the same snapshot.
#[test]
fn pulled_cache_is_not_visible_to_another_tenant() {
    if isolated("pulled_cache_is_not_visible_to_another_tenant") {
        return;
    }
    run(async {
        let f = Fixture::new(true);
        snapdir_api::pull(
            &f.id,
            &uri(&f.store),
            &f.dest,
            &options(&f.cache, Some(&f.pool)),
        )
        .await
        .unwrap();
        let other = f.cache.with_file_name("tenant B");
        std::fs::create_dir_all(&other).unwrap();
        let dest = f.dest.with_file_name("tenant B destination");
        assert!(snapdir_api::checkout(&f.id, &dest, &options(&other, None))
            .await
            .is_err());
        assert!(!dest.join("payload.bin").exists());
        assert!(shard(&f.cache, ".manifests", &f.id.to_hex()).is_file());
        assert!(!shard(&other, ".manifests", &f.id.to_hex()).exists());
    });
}

// SPEC: local corruption must be refused, including when the default cache has valid bytes.
#[test]
fn checkout_refuses_corrupt_explicit_cache_object() {
    if isolated("checkout_refuses_corrupt_explicit_cache_object") {
        return;
    }
    run(async {
        let f = Fixture::new(false);
        snapdir_api::stage(&f.source, &StageOptions::default()).unwrap();
        std::fs::write(shard(&f.store, ".objects", &f.object_hex), b"corrupt").unwrap();
        assert!(
            snapdir_api::checkout(&f.id, &f.dest, &options(&f.store, None))
                .await
                .is_err()
        );
        assert!(!f.dest.join("payload.bin").exists());
    });
}

// SPEC: missing local objects must be refused rather than fetched from any other cache.
#[test]
fn checkout_refuses_missing_explicit_cache_object() {
    if isolated("checkout_refuses_missing_explicit_cache_object") {
        return;
    }
    run(async {
        let f = Fixture::new(false);
        snapdir_api::stage(&f.source, &StageOptions::default()).unwrap();
        std::fs::remove_file(shard(&f.store, ".objects", &f.object_hex)).unwrap();
        assert!(
            snapdir_api::checkout(&f.id, &f.dest, &options(&f.store, None))
                .await
                .is_err()
        );
        assert!(!f.dest.join("payload.bin").exists());
    });
}

// SPEC: an unusable explicit cache is an error, never permission to use another cache.
#[test]
fn explicit_cache_path_that_is_a_file_is_refused() {
    if isolated("explicit_cache_path_that_is_a_file_is_refused") {
        return;
    }
    run(async {
        let f = Fixture::new(false);
        std::fs::write(&f.cache, b"do not overwrite").unwrap();
        let opts = options(&f.cache, None);
        assert!(snapdir_api::pull(&f.id, &uri(&f.store), &f.dest, &opts)
            .await
            .is_err());
        assert_eq!(std::fs::read(&f.cache).unwrap(), b"do not overwrite");
        assert!(!f.dest.join("payload.bin").exists());
        assert!(snapdir_api::checkout(&f.id, &f.dest, &opts).await.is_err());
    });
}

// SPEC: defaults preserve colocated pull and default-cache checkout behavior.
#[test]
fn default_options_preserve_colocated_pull_and_checkout() {
    if isolated("default_options_preserve_colocated_pull_and_checkout") {
        return;
    }
    run(async {
        let f = Fixture::new(false);
        let mut opts = CheckoutOptions::default();
        assert!(opts.transfer.objects_store.is_none());
        assert!(opts.transfer.cache_dir.is_none());
        opts.keep = true;
        snapdir_api::pull(&f.id, &uri(&f.store), &f.dest, &opts)
            .await
            .unwrap();
        f.assert_tree(&f.dest);
        let dest = f.dest.with_file_name("default offline");
        std::fs::remove_dir_all(&f.store).unwrap();
        snapdir_api::checkout(&f.id, &dest, &opts).await.unwrap();
        f.assert_tree(&dest);
    });
}

// SPEC: a complete warm cache does not override the requested manifest location.
#[test]
fn warm_cache_refuses_missing_or_corrupt_source_manifest() {
    if isolated("warm_cache_refuses_missing_or_corrupt_source_manifest") {
        return;
    }
    run(async {
        for corrupt in [false, true] {
            let f = Fixture::new(true);
            let opts = options(&f.cache, Some(&f.pool));
            snapdir_api::pull(&f.id, &uri(&f.store), &f.dest, &opts)
                .await
                .unwrap();
            let manifest = shard(&f.store, ".manifests", &f.id.to_hex());
            if corrupt {
                std::fs::write(manifest, b"not a valid manifest\n").unwrap();
            } else {
                std::fs::remove_file(manifest).unwrap();
            }
            let dest = f.dest.with_file_name("must remain absent");
            assert!(snapdir_api::pull(&f.id, &uri(&f.store), &dest, &opts)
                .await
                .is_err());
            assert!(!dest.exists());
        }
    });
}

// SPEC: a cached manifest cannot certify corrupt or absent cached objects on repeat pull.
#[test]
fn warm_cache_refuses_corrupt_or_missing_object_before_writing_destination() {
    if isolated("warm_cache_refuses_corrupt_or_missing_object_before_writing_destination") {
        return;
    }
    run(async {
        for corrupt in [false, true] {
            let f = Fixture::new(true);
            let opts = options(&f.cache, Some(&f.pool));
            snapdir_api::pull(&f.id, &uri(&f.store), &f.dest, &opts)
                .await
                .unwrap();
            let object = shard(&f.cache, ".objects", &f.object_hex);
            if corrupt {
                std::fs::write(object, b"corrupt").unwrap();
            } else {
                std::fs::remove_file(object).unwrap();
            }
            let dest = f.dest.with_file_name("preserved destination");
            std::fs::create_dir(&dest).unwrap();
            std::fs::write(dest.join("sentinel"), b"untouched").unwrap();
            assert!(snapdir_api::pull(&f.id, &uri(&f.store), &dest, &opts)
                .await
                .is_err());
            assert_eq!(std::fs::read(dest.join("sentinel")).unwrap(), b"untouched");
            assert_eq!(std::fs::read_dir(dest).unwrap().count(), 1);
        }
    });
}

// SPEC: corrupt explicit cache manifests cannot be used or substituted by default cache data.
#[test]
fn checkout_refuses_corrupt_cache_manifest_before_writing_destination() {
    if isolated("checkout_refuses_corrupt_cache_manifest_before_writing_destination") {
        return;
    }
    run(async {
        let f = Fixture::new(false);
        snapdir_api::stage(&f.source, &StageOptions::default()).unwrap();
        std::fs::write(
            shard(&f.store, ".manifests", &f.id.to_hex()),
            b"corrupt manifest\n",
        )
        .unwrap();
        assert!(
            snapdir_api::checkout(&f.id, &f.dest, &options(&f.store, None))
                .await
                .is_err()
        );
        assert!(!f.dest.exists());
    });
}

// SPEC: default behavior preserves external adapters' direct pull without streaming support.
#[test]
fn external_adapter_default_pull_keeps_direct_path() {
    if isolated("external_adapter_default_pull_keeps_direct_path") {
        return;
    }
    run(async {
        let f = Fixture::new(false);
        let store = StoreUri::parse(&format!("ssh://{}", f.store.display())).unwrap();
        snapdir_api::pull(&f.id, &store, &f.dest, &CheckoutOptions::default())
            .await
            .unwrap();
        f.assert_tree(&f.dest);
    });
}

// SPEC: unsupported streaming overrides fail explicitly; never silently ignore a requested cache/pool.
#[test]
fn external_adapter_overrides_are_rejected_before_transfer() {
    if isolated("external_adapter_overrides_are_rejected_before_transfer") {
        return;
    }
    run(async {
        let f = Fixture::new(false);
        let external = StoreUri::parse(&format!("ssh://{}", f.store.display())).unwrap();
        let mut pool_opts = options(&f.cache, None);
        pool_opts.transfer.objects_store = Some(external.clone());
        for (store, opts) in [
            (external.clone(), options(&f.cache, None)),
            (uri(&f.store), pool_opts),
            (external, options(&f.cache, Some(&f.pool))),
        ] {
            let result = snapdir_api::pull(&f.id, &store, &f.dest, &opts).await;
            assert_eq!(result.unwrap_err().code(), "INVALID_STORE");
            assert!(!f.dest.exists());
            assert!(!f.cache.exists());
        }
    });
}

// SPEC: filesystem materialization failure must be returned, not reported as a successful transfer.
#[test]
fn destination_io_failure_is_returned_and_existing_bytes_preserved() {
    if isolated("destination_io_failure_is_returned_and_existing_bytes_preserved") {
        return;
    }
    run(async {
        let f = Fixture::new(false);
        std::fs::write(&f.dest, b"preserve file").unwrap();
        assert!(
            snapdir_api::pull(&f.id, &uri(&f.store), &f.dest, &options(&f.cache, None))
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&f.dest).unwrap(), b"preserve file");
        assert!(
            snapdir_api::checkout(&f.id, &f.dest, &options(&f.store, None))
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&f.dest).unwrap(), b"preserve file");
    });
}

// SPEC: fetch honors both split object stores and explicit cache, then checkout works offline.
#[test]
fn fetch_split_pool_populates_only_explicit_cache_for_offline_checkout() {
    if isolated("fetch_split_pool_populates_only_explicit_cache_for_offline_checkout") {
        return;
    }
    run(async {
        let f = Fixture::new(true);
        let opts = options(&f.cache, Some(&f.pool));
        snapdir_api::fetch(&f.id, &uri(&f.store), &opts.transfer)
            .await
            .unwrap();
        assert!(shard(&f.cache, ".manifests", &f.id.to_hex()).is_file());
        assert_eq!(
            std::fs::read(shard(&f.cache, ".objects", &f.object_hex)).unwrap(),
            b"transfer-contract\0\xff\x01"
        );
        assert!(!f.dest.exists(), "fetch must not materialize a destination");
        std::fs::remove_dir_all(&f.store).unwrap();
        std::fs::remove_dir_all(&f.pool).unwrap();
        snapdir_api::checkout(&f.id, &f.dest, &opts).await.unwrap();
        f.assert_tree(&f.dest);
        let other_dest = f.dest.with_file_name("default must not borrow");
        assert!(
            snapdir_api::checkout(&f.id, &other_dest, &CheckoutOptions::default())
                .await
                .is_err()
        );
        assert!(!other_dest.exists());
    });
}

// SPEC: even a warm fetch must validate the requested source manifest location.
#[test]
fn fetch_warm_cache_refuses_missing_or_corrupt_source_manifest() {
    if isolated("fetch_warm_cache_refuses_missing_or_corrupt_source_manifest") {
        return;
    }
    run(async {
        for corrupt in [false, true] {
            let f = Fixture::new(true);
            let opts = options(&f.cache, Some(&f.pool));
            snapdir_api::fetch(&f.id, &uri(&f.store), &opts.transfer)
                .await
                .unwrap();
            let manifest = shard(&f.store, ".manifests", &f.id.to_hex());
            if corrupt {
                std::fs::write(manifest, b"corrupt manifest\n").unwrap();
            } else {
                std::fs::remove_file(manifest).unwrap();
            }
            assert!(snapdir_api::fetch(&f.id, &uri(&f.store), &opts.transfer)
                .await
                .is_err());
            // The failure must not remove the previously valid offline snapshot.
            snapdir_api::checkout(&f.id, &f.dest, &opts).await.unwrap();
            f.assert_tree(&f.dest);
        }
    });
}

// SPEC: successful fetch guarantees a usable cache; cached manifest alone cannot certify corrupt bytes.
#[test]
fn fetch_warm_cache_refuses_corrupt_or_missing_cached_object() {
    if isolated("fetch_warm_cache_refuses_corrupt_or_missing_cached_object") {
        return;
    }
    run(async {
        let mut refused = Vec::new();
        for corrupt in [false, true] {
            let f = Fixture::new(true);
            let opts = options(&f.cache, Some(&f.pool));
            snapdir_api::fetch(&f.id, &uri(&f.store), &opts.transfer)
                .await
                .unwrap();
            let object = shard(&f.cache, ".objects", &f.object_hex);
            if corrupt {
                std::fs::write(object, b"corrupt").unwrap();
            } else {
                std::fs::remove_file(object).unwrap();
            }
            refused.push(
                snapdir_api::fetch(&f.id, &uri(&f.store), &opts.transfer)
                    .await
                    .is_err(),
            );
            assert!(!f.dest.exists());
        }
        assert_eq!(refused, vec![true, true],
            "fetch must refuse both missing and corrupt selected-cache objects rather than report success");
    });
}

// SPEC: verified warm cache objects may be reused while the source manifest remains authoritative.
#[test]
fn fetch_reuses_verified_warm_cache_when_pool_objects_are_offline() {
    if isolated("fetch_reuses_verified_warm_cache_when_pool_objects_are_offline") {
        return;
    }
    run(async {
        let f = Fixture::new(true);
        let opts = options(&f.cache, Some(&f.pool));
        snapdir_api::fetch(&f.id, &uri(&f.store), &opts.transfer)
            .await
            .unwrap();
        std::fs::remove_dir_all(f.pool.join(".objects")).unwrap();
        snapdir_api::fetch(&f.id, &uri(&f.store), &opts.transfer)
            .await
            .unwrap();
        snapdir_api::checkout(&f.id, &f.dest, &opts).await.unwrap();
        f.assert_tree(&f.dest);
    });
}

// SPEC: cold fetch cannot substitute colocated objects for a missing or corrupt explicit pool.
#[test]
fn fetch_refuses_missing_or_corrupt_explicit_pool_objects() {
    if isolated("fetch_refuses_missing_or_corrupt_explicit_pool_objects") {
        return;
    }
    run(async {
        for corrupt in [false, true] {
            let f = Fixture::new(true);
            snapdir_api::stage(
                &f.source,
                &StageOptions {
                    cache_dir: Some(f.store.clone()),
                    keep: true,
                },
            )
            .unwrap();
            let object = shard(&f.pool, ".objects", &f.object_hex);
            if corrupt {
                std::fs::write(object, b"corrupt").unwrap();
            } else {
                std::fs::remove_file(object).unwrap();
            }
            assert!(snapdir_api::fetch(
                &f.id,
                &uri(&f.store),
                &options(&f.cache, Some(&f.pool)).transfer
            )
            .await
            .is_err());
            assert!(
                !shard(&f.cache, ".manifests", &f.id.to_hex()).exists(),
                "failed fetch must not publish a cache manifest for an incomplete snapshot"
            );
        }
    });
}
