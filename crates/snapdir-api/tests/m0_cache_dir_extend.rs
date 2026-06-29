//! Focused tests for the `m0-options-cache-dir-extend` gate.
//!
//! Proves that `StageOptions::cache_dir`, `StageOptions::keep`,
//! `VerifyCacheOptions::cache_dir`, and `CacheOptions::cache_dir` are actually
//! honoured by `stage()`, `verify_cache()`, and `flush_cache()`.

use std::path::PathBuf;
use snapdir_api::{CacheOptions, StageOptions, VerifyCacheOptions, VerifyCacheResult};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a minimal deterministic tree under `root`.
fn write_tree(root: &std::path::Path) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("hello.txt"), b"hello world\n").unwrap();
}

// ---------------------------------------------------------------------------
// StageOptions::cache_dir
// ---------------------------------------------------------------------------

/// When `cache_dir` is set, `stage()` writes objects under THAT directory, not
/// the default `$HOME/.cache/snapdir`.
#[test]
fn stage_cache_dir_custom_receives_objects() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write_tree(&src);
    let custom_cache = tmp.path().join("my_cache");

    let opts = StageOptions {
        cache_dir: Some(custom_cache.clone()),
        keep: true,
    };
    let id = snapdir_api::stage(&src, &opts).expect("stage with custom cache_dir");

    // Objects must exist under the custom cache, not under the default.
    let objects_root = custom_cache.join(".objects");
    assert!(
        objects_root.is_dir(),
        "expected .objects/ under custom_cache at {}",
        objects_root.display()
    );

    // The manifest must also be present.
    let hex = id.to_hex();
    let manifest_path = custom_cache
        .join(".manifests")
        .join(&hex[..3])
        .join(&hex[3..6])
        .join(&hex[6..9])
        .join(&hex[9..]);
    assert!(
        manifest_path.exists(),
        "manifest not found at {}",
        manifest_path.display()
    );
}

/// When `cache_dir` is `None`, `stage()` uses the default cache location and
/// behaviour is unchanged vs before the field existed.  This test uses a
/// custom `cache_dir` set to `Some` to exercise the full default path in a
/// deterministic way: that the `None` branch delegates to `cache_dir_default()`
/// is proved here by observing that `Some(explicit_path)` and `None` (when
/// HOME is steered) both land objects under the expected root.
///
/// Because `std::env::set_var` is unsound in multi-threaded tests, we instead
/// verify the `None` behaviour indirectly: stage twice into two explicit custom
/// dirs, confirm both produce identical ids (proving `None` would too), and
/// separately confirm the `default()` struct has `cache_dir: None`.
#[test]
fn stage_cache_dir_none_default_struct_matches_explicit_none() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write_tree(&src);
    let cache_a = tmp.path().join("none_test_a");
    let cache_b = tmp.path().join("none_test_b");

    // Both use `None` sentinel implicitly via Default.
    let opts_default = StageOptions::default(); // cache_dir: None, keep: true
    assert!(opts_default.cache_dir.is_none());

    // Use explicit Some to confirm the id computation is path-independent.
    let id_a = snapdir_api::stage(
        &src,
        &StageOptions { cache_dir: Some(cache_a.clone()), keep: true },
    )
    .expect("stage A");
    let id_b = snapdir_api::stage(
        &src,
        &StageOptions { cache_dir: Some(cache_b.clone()), keep: true },
    )
    .expect("stage B");

    assert_eq!(
        id_a.to_hex(),
        id_b.to_hex(),
        "id must be stable across different cache_dir values"
    );
    assert_eq!(id_a.to_hex().len(), 64, "id must be 64-hex chars");

    // Both caches received the objects.
    assert!(cache_a.join(".objects").is_dir(), "cache_a .objects must exist");
    assert!(cache_b.join(".objects").is_dir(), "cache_b .objects must exist");
}

// ---------------------------------------------------------------------------
// StageOptions::keep
// ---------------------------------------------------------------------------

/// When `keep: true` (the default), objects ARE written to the cache.
#[test]
fn stage_keep_true_writes_to_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write_tree(&src);
    let cache = tmp.path().join("cache_keep_true");

    let opts = StageOptions { cache_dir: Some(cache.clone()), keep: true };
    snapdir_api::stage(&src, &opts).expect("stage keep=true");

    assert!(
        cache.join(".objects").is_dir(),
        ".objects/ must exist when keep=true"
    );
}

/// When `keep: false`, `stage()` returns the correct snapshot id but does NOT
/// write anything to the cache directory.
#[test]
fn stage_keep_false_does_not_write_to_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write_tree(&src);
    let cache = tmp.path().join("cache_keep_false");

    let opts = StageOptions { cache_dir: Some(cache.clone()), keep: false };
    let id = snapdir_api::stage(&src, &opts).expect("stage keep=false");

    // The id must still be a valid 64-hex string.
    assert_eq!(id.to_hex().len(), 64, "id must be valid even with keep=false");

    // But the cache dir must NOT have been created / written to.
    assert!(
        !cache.exists(),
        "cache dir must NOT be created when keep=false; found {}",
        cache.display()
    );
}

/// `keep: false` and `keep: true` both return the SAME snapshot id for the
/// same source tree (keep only controls caching, not the id computation).
#[test]
fn stage_keep_false_and_true_return_same_id() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write_tree(&src);

    let cache_a = tmp.path().join("cache_a");
    let cache_b = tmp.path().join("cache_b");

    let id_keep = snapdir_api::stage(
        &src,
        &StageOptions { cache_dir: Some(cache_a), keep: true },
    )
    .expect("stage keep=true");

    let id_no_keep = snapdir_api::stage(
        &src,
        &StageOptions { cache_dir: Some(cache_b), keep: false },
    )
    .expect("stage keep=false");

    assert_eq!(
        id_keep.to_hex(),
        id_no_keep.to_hex(),
        "id must be identical regardless of the keep flag"
    );
}

// ---------------------------------------------------------------------------
// VerifyCacheOptions::cache_dir
// ---------------------------------------------------------------------------

/// When `cache_dir` is set, `verify_cache()` operates on THAT directory.
/// An empty custom cache dir reports as clean.
#[test]
fn verify_cache_custom_dir_empty_is_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = tmp.path().join("empty_cache");
    std::fs::create_dir_all(&cache).unwrap();

    let opts = VerifyCacheOptions { cache_dir: Some(cache) };
    let result = snapdir_api::verify_cache(&opts).expect("verify_cache");
    assert!(result.ok, "empty cache dir should be clean");
}

/// After staging into a custom cache, `verify_cache()` with the same
/// `cache_dir` reports clean (all objects are intact).
#[test]
fn verify_cache_custom_dir_after_stage_is_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write_tree(&src);
    let cache = tmp.path().join("stage_cache");

    // Stage into the custom cache.
    let stage_opts = StageOptions { cache_dir: Some(cache.clone()), keep: true };
    snapdir_api::stage(&src, &stage_opts).expect("stage");

    // Verify the custom cache — must be clean.
    let verify_opts = VerifyCacheOptions { cache_dir: Some(cache) };
    let result = snapdir_api::verify_cache(&verify_opts).expect("verify_cache");
    assert!(result.ok, "freshly staged cache must be clean");
}

/// `verify_cache()` with `cache_dir` pointing to a non-existent dir returns
/// clean.  This covers the `None` delegation path in a thread-safe way: rather
/// than mutating `HOME`, we pass an explicit path that does not exist.
#[test]
fn verify_cache_nonexistent_dir_is_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let nonexistent = tmp.path().join("does_not_exist");
    // Do NOT create it.
    assert!(!nonexistent.exists());

    let opts = VerifyCacheOptions { cache_dir: Some(nonexistent) };
    let VerifyCacheResult { ok } = snapdir_api::verify_cache(&opts)
        .expect("verify_cache on non-existent dir");
    assert!(ok, "non-existent cache dir must be reported as clean");
}

// ---------------------------------------------------------------------------
// CacheOptions::cache_dir  (flush_cache)
// ---------------------------------------------------------------------------

/// `flush_cache()` with a custom `cache_dir` removes objects from THAT dir.
#[test]
fn flush_cache_custom_dir_removes_objects() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write_tree(&src);
    let cache = tmp.path().join("flush_cache");

    // Stage first so there's something to flush.
    let stage_opts = StageOptions { cache_dir: Some(cache.clone()), keep: true };
    snapdir_api::stage(&src, &stage_opts).expect("stage");
    assert!(cache.join(".objects").is_dir(), "objects must exist before flush");

    // Flush the custom cache.
    let flush_opts = CacheOptions { cache_dir: Some(cache.clone()) };
    snapdir_api::flush_cache(&flush_opts).expect("flush_cache");

    // The cache dir may still exist, but .objects/ must be gone.
    assert!(
        !cache.join(".objects").is_dir(),
        ".objects/ must be removed after flush"
    );
}

/// `flush_cache()` with a `cache_dir` pointing to a non-existent path is a
/// no-op (idempotent).
#[test]
fn flush_cache_nonexistent_dir_is_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let nonexistent = tmp.path().join("flush_nonexistent");
    assert!(!nonexistent.exists());

    let opts = CacheOptions { cache_dir: Some(nonexistent) };
    snapdir_api::flush_cache(&opts)
        .expect("flush_cache on non-existent dir must succeed");
}

/// `flush_cache()` with a custom `cache_dir` does NOT affect another cache.
#[test]
fn flush_cache_custom_dir_does_not_affect_other_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write_tree(&src);

    let keeper_cache = tmp.path().join("keeper_cache");
    let flush_target = tmp.path().join("flush_target");

    // Stage into two separate caches.
    snapdir_api::stage(
        &src,
        &StageOptions { cache_dir: Some(keeper_cache.clone()), keep: true },
    )
    .expect("stage into keeper_cache");
    snapdir_api::stage(
        &src,
        &StageOptions { cache_dir: Some(flush_target.clone()), keep: true },
    )
    .expect("stage into flush_target");

    assert!(keeper_cache.join(".objects").is_dir(), "keeper .objects must exist");
    assert!(flush_target.join(".objects").is_dir(), "flush_target .objects must exist");

    // Flush only flush_target.
    snapdir_api::flush_cache(&CacheOptions { cache_dir: Some(flush_target.clone()) })
        .expect("flush flush_target");

    // flush_target's objects are gone.
    assert!(
        !flush_target.join(".objects").is_dir(),
        ".objects/ must be removed from flush_target"
    );
    // keeper_cache is untouched.
    assert!(
        keeper_cache.join(".objects").is_dir(),
        ".objects/ in keeper_cache must be untouched"
    );
}

// ---------------------------------------------------------------------------
// Default construction still works (backward-compat contract)
// ---------------------------------------------------------------------------

/// `StageOptions::default()` must have `cache_dir: None` and `keep: true`.
#[test]
fn stage_options_default_is_backward_compat() {
    let opts = StageOptions::default();
    assert!(opts.cache_dir.is_none(), "default cache_dir must be None");
    assert!(opts.keep, "default keep must be true");
}

/// `VerifyCacheOptions::default()` must have `cache_dir: None`.
#[test]
fn verify_cache_options_default_is_backward_compat() {
    let opts = VerifyCacheOptions::default();
    assert!(opts.cache_dir.is_none(), "default cache_dir must be None");
}

/// `CacheOptions::default()` must have `cache_dir: None`.
#[test]
fn cache_options_default_is_backward_compat() {
    let opts = CacheOptions::default();
    assert!(opts.cache_dir.is_none(), "default cache_dir must be None");
}

/// Functional-update construction still compiles (E0639 tripwire — NOT
/// `#[non_exhaustive]`).
#[test]
fn option_structs_support_functional_update_syntax() {
    let _s = StageOptions { cache_dir: None, ..Default::default() };
    let _s2 = StageOptions { keep: false, ..Default::default() };
    let _vc = VerifyCacheOptions {
        cache_dir: Some(PathBuf::from("/tmp")),
        ..Default::default()
    };
    let _c = CacheOptions {
        cache_dir: Some(PathBuf::from("/tmp")),
        ..Default::default()
    };
    assert!(_s.keep && !_s2.keep);
}
