//! Black-box public-surface spec for `snapdir-api` (M0, gate `m0-api-surface-spec-tests`).
//!
//! Authored from the LOCKED spec `.gatesmith/reviews/m0-public-api.md` ALONE — the
//! `crates/snapdir-api` crate does not exist yet, so this file is EXPECTED to fail to
//! compile/pass until the impl lands. The lane owner will `git mv` it into
//! `crates/snapdir-api/tests/` during the `*-impl` gate.
//!
//! Focus: the FULL public function surface of §6 + the §3/§5 types — by exact name,
//! arity, and typed-Result shape — so this file both documents and compile-checks the
//! surface. Deep behavior is covered by the async-facade and golden-parity clusters;
//! these assertions are SURFACE + basic invariants, but never weaker than the spec.

use std::path::{Path, PathBuf};

use snapdir_api::{
    // §3 types
    Ancestor,
    // §5 option structs + enums
    AncestorsOptions,
    CacheOptions,
    CatalogOption,
    CheckoutOptions,
    ChecksumBin,
    ConflictPolicy,
    DiffEntry,
    DiffOptions,
    DiffStatus,
    EffectiveConfig,
    Location,
    LocationsOptions,
    Manifest,
    ManifestEntry,
    ManifestOptions,
    PathType,
    PushSource,
    Revision,
    RevisionsOptions,
    SnapdirError,
    SnapshotId,
    StageOptions,
    StoreUri,
    TransferOptions,
    VerifyCacheOptions,
    VerifyCacheResult,
    VerifyOptions,
    // free functions are referenced fully-qualified as snapdir_api::* below.
    VerifyResult,
};

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

/// Materialize a small, deterministic directory tree in a fresh temp dir and return
/// (tempdir-guard, path). The guard must be kept alive for the duration of the test.
fn fixture_tree() -> (tempfile::TempDir, PathBuf) {
    let td = tempfile::tempdir().expect("tempdir");
    let root = td.path().to_path_buf();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("a.txt"), b"hello snapdir\n").unwrap();
    std::fs::write(root.join("sub/b.bin"), vec![0u8; 4096]).unwrap();
    std::fs::write(root.join("empty.txt"), b"").unwrap();
    (td, root)
}

/// A `file://` StoreUri pointing at a fresh temp dir (the object store / catalog root).
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
// §3 TYPES — SnapshotId newtype shape
// ===========================================================================

#[test]
fn snapshot_id_is_64_char_lowercase_hex_roundtrip() {
    // §3: SnapshotId([u8;32]); Display/FromStr as 64-char lowercase hex; from_hex/to_hex/as_bytes.
    let hex = "a".repeat(64);
    let id: SnapshotId = SnapshotId::from_hex(&hex).expect("valid 64-hex parses");
    assert_eq!(id.to_hex(), hex, "to_hex round-trips from_hex");
    assert!(
        is_64_lower_hex(&id.to_hex()),
        "to_hex is 64-char lowercase hex"
    );
    assert_eq!(id.as_bytes().len(), 32, "as_bytes is &[u8;32]");
    assert_eq!(
        id.as_bytes(),
        &[0xaau8; 32],
        "0xaa repeated decodes correctly"
    );

    // Display == hex; FromStr == from_hex.
    assert_eq!(format!("{id}"), hex, "Display renders 64-hex");
    let viafromstr: SnapshotId = hex.parse().expect("FromStr == from_hex");
    assert_eq!(viafromstr, id, "FromStr and from_hex agree (PartialEq/Eq)");

    // Copy + Clone + Hash are derivable; exercise Copy + Hash usage.
    let copied = id; // Copy
    let _clone = id.clone();
    assert_eq!(copied, id);
    let mut set = std::collections::HashSet::new();
    set.insert(id); // Hash + Eq
    assert!(set.contains(&copied));
}

#[test]
fn snapshot_id_from_hex_rejects_bad_input_with_invalid_id() {
    // §3/§4: from_hex -> InvalidId on bad len/chars. (Surface: returns Result<_, SnapdirError>.)
    let bad_len: Result<SnapshotId, SnapdirError> = SnapshotId::from_hex("deadbeef");
    let err = bad_len.expect_err("too-short hex must error");
    assert_eq!(err.code(), "INVALID_ID", "short hex -> INVALID_ID code");

    let bad_char: Result<SnapshotId, SnapdirError> =
        SnapshotId::from_hex(&format!("{}zz", "0".repeat(62)));
    assert_eq!(
        bad_char.expect_err("non-hex char must error").code(),
        "INVALID_ID",
        "non-hex chars -> INVALID_ID code"
    );
}

// ===========================================================================
// §3 TYPES — StoreUri scheme validation
// ===========================================================================

#[test]
fn store_uri_parses_known_schemes_and_rejects_unknown() {
    // §3: StoreUri::parse / scheme(); accepts file/s3/gs/b2/ssh/sftp; unknown -> InvalidStore.
    for (uri, scheme) in [
        ("file:///tmp/x", "file"),
        ("s3://bucket/p", "s3"),
        ("gs://bucket/p", "gs"),
        ("b2://bucket/p", "b2"),
        ("ssh://host/p", "ssh"),
        ("sftp://host/p", "sftp"),
    ] {
        let parsed = StoreUri::parse(uri).unwrap_or_else(|_| panic!("{uri} should parse"));
        assert_eq!(
            parsed.scheme(),
            scheme,
            "scheme() reports the right scheme for {uri}"
        );
        // Display round-trips (per §3 "Display round-trips").
        assert!(
            format!("{parsed}").contains(scheme),
            "Display retains scheme for {uri}"
        );
    }

    let bad: Result<StoreUri, SnapdirError> = StoreUri::parse("wat://nope");
    assert_eq!(
        bad.expect_err("unknown scheme must error").code(),
        "INVALID_STORE",
        "unknown scheme -> INVALID_STORE code"
    );
}

// ===========================================================================
// §3 TYPES — DiffStatus Display glyphs
// ===========================================================================

#[test]
fn diff_status_display_glyphs() {
    // §3: DiffStatus{Added,Deleted,Modified,Unchanged} Display 'A'/'D'/'M'/'='.
    assert_eq!(format!("{}", DiffStatus::Added), "A");
    assert_eq!(format!("{}", DiffStatus::Deleted), "D");
    assert_eq!(format!("{}", DiffStatus::Modified), "M");
    assert_eq!(format!("{}", DiffStatus::Unchanged), "=");
}

// ===========================================================================
// §4 ERROR — code() stability across all variants
// ===========================================================================

#[test]
fn snapdir_error_code_is_a_stable_static_str() {
    // §4: SnapdirError::code() -> &'static str. We can only reliably construct INVALID_ID /
    // INVALID_STORE from the public surface; assert those are exactly the spec'd codes and
    // that the value is 'static (assignable to a &'static str).
    let id_err = SnapshotId::from_hex("nope").expect_err("invalid id");
    let store_err = StoreUri::parse("zzz://nope").expect_err("invalid store");
    let _static_check: &'static str = code_of(&id_err);
    assert_eq!(id_err.code(), "INVALID_ID");
    assert_eq!(store_err.code(), "INVALID_STORE");
}

fn code_of(e: &SnapdirError) -> &'static str {
    e.code()
}

// ===========================================================================
// §5 OPTIONS — every option struct is Default (#[derive(Default)])
// ===========================================================================

#[test]
fn option_structs_are_default_constructible() {
    // §5: all options #[derive(Default)] + #[non_exhaustive]; defaults == CLI effective defaults.
    let _: ManifestOptions = ManifestOptions::default();
    let _: StageOptions = StageOptions::default();
    let _: TransferOptions = TransferOptions::default();
    let _: CheckoutOptions = CheckoutOptions::default();
    let _: DiffOptions = DiffOptions::default();
    let _: VerifyOptions = VerifyOptions::default();
    let _: VerifyCacheOptions = VerifyCacheOptions::default();
    let _: CacheOptions = CacheOptions::default();
    let _: LocationsOptions = LocationsOptions::default();
    let _: AncestorsOptions = AncestorsOptions::default();
    let _: RevisionsOptions = RevisionsOptions::default();
}

#[test]
fn option_enums_have_spec_named_variants_and_defaults() {
    // §5: ChecksumBin{B3sum,Md5sum,Sha256sum}; CatalogOption{Default,None,Named}; ConflictPolicy{Error,LastWins}.
    let _ = [
        ChecksumBin::B3sum,
        ChecksumBin::Md5sum,
        ChecksumBin::Sha256sum,
    ];
    let _ = [
        CatalogOption::Default,
        CatalogOption::None,
        CatalogOption::Named("c".to_string()),
    ];
    let _ = [ConflictPolicy::Error, ConflictPolicy::LastWins];
    // ChecksumBin default must be the CLI default (BLAKE3) per "defaults == CLI effective defaults".
    assert_eq!(
        ChecksumBin::default(),
        ChecksumBin::B3sum,
        "default checksum is BLAKE3 (b3sum), matching the CLI"
    );
    // CheckoutOptions embeds a TransferOptions (per §5) and DiffOptions uses ConflictPolicy.
    let co = CheckoutOptions::default();
    let _embedded: &TransferOptions = &co.transfer;
    let dopts = DiffOptions::default();
    let _conflict: ConflictPolicy = dopts.on_conflict;
}

// ===========================================================================
// §6 SNAPSHOTTING (SYNC) — real assertions against a temp fixture
// ===========================================================================

#[test]
fn manifest_returns_entries_for_a_real_tree() {
    // §6: pub fn manifest(path:&Path, o:&ManifestOptions) -> Result<Manifest>.
    let (_g, root) = fixture_tree();
    let o = ManifestOptions::default();
    let m: Manifest = snapdir_api::manifest(root.as_path(), &o).expect("manifest of a real tree");
    assert!(
        !m.entries.is_empty(),
        "manifest has entries for a non-empty tree"
    );
    // §3: Manifest{entries:Vec<ManifestEntry>, raw:String}; raw kept for round-trip.
    assert!(
        !m.raw.is_empty(),
        "manifest keeps its raw text for round-trip"
    );
    // ManifestEntry shape is reachable (path_type/path fields used).
    let entry: &ManifestEntry = m.entries.first().unwrap();
    let _pt: &PathType = &entry.path_type;
    let _p: &Path = entry.path.as_path();
}

#[test]
fn id_is_a_64_hex_snapshot_id() {
    // §6: pub fn id(path:&Path, o:&ManifestOptions) -> Result<SnapshotId>.
    let (_g, root) = fixture_tree();
    let o = ManifestOptions::default();
    let id: SnapshotId = snapdir_api::id(root.as_path(), &o).expect("id of a real tree");
    assert!(
        is_64_lower_hex(&id.to_hex()),
        "id() yields a 64-hex SnapshotId"
    );
}

#[test]
fn id_from_manifest_equals_id_and_is_infallible() {
    // §6: pub fn id_from_manifest(m:&Manifest) -> SnapshotId  (pure, infallible — NO Result).
    let (_g, root) = fixture_tree();
    let o = ManifestOptions::default();
    let m = snapdir_api::manifest(root.as_path(), &o).expect("manifest");
    let from_manifest: SnapshotId = snapdir_api::id_from_manifest(&m); // note: not a Result
    let direct: SnapshotId = snapdir_api::id(root.as_path(), &o).expect("id");
    assert_eq!(
        from_manifest, direct,
        "id_from_manifest(&manifest(path)) == id(path) — the merkle root is the manifest hash"
    );
}

#[test]
fn id_is_deterministic_across_runs() {
    // §6 invariant: id() over the same tree is stable (content-addressed determinism).
    let (_g, root) = fixture_tree();
    let o = ManifestOptions::default();
    let a = snapdir_api::id(root.as_path(), &o).expect("id a");
    let b = snapdir_api::id(root.as_path(), &o).expect("id b");
    assert_eq!(a, b, "id() is deterministic for identical input");
}

#[test]
fn stage_returns_the_same_id_as_id() {
    // §6: pub fn stage(path:&Path, o:&StageOptions) -> Result<SnapshotId>.
    // Staging computes + records the snapshot; the returned id must equal id()'s.
    // Hermetic isolation: stage into a per-test cache dir (honored by stage()),
    // never the shared global cache ($HOME/.cache/snapdir), so parallel tests
    // can't race on it.
    let (_g, root) = fixture_tree();
    let cache_td = tempfile::tempdir().expect("cache tempdir");
    let staged: SnapshotId = snapdir_api::stage(
        root.as_path(),
        &StageOptions {
            cache_dir: Some(cache_td.path().into()),
            ..Default::default()
        },
    )
    .expect("stage");
    let computed: SnapshotId =
        snapdir_api::id(root.as_path(), &ManifestOptions::default()).expect("id");
    assert_eq!(
        staged, computed,
        "stage() returns the same SnapshotId as id()"
    );
}

// ===========================================================================
// §6 DISTRIBUTION (ASYNC) — file:// round-trip + typed-Result surface
// ===========================================================================

#[tokio::test]
async fn push_returns_a_snapshot_id_into_a_file_store() {
    // §6: pub async fn push(src:PushSource<'_>, store:&StoreUri, o:&TransferOptions) -> Result<SnapshotId>.
    let (_gtree, root) = fixture_tree();
    let (_gstore, store) = file_store();
    let to = TransferOptions::default();
    let pushed: SnapshotId = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
        .await
        .expect("push a real dir to a file:// store");
    // Must equal the locally-computed id (push is content-addressed, not a new id).
    let local = snapdir_api::id(root.as_path(), &ManifestOptions::default()).expect("id");
    assert_eq!(
        pushed, local,
        "push returns the content-addressed SnapshotId"
    );
}

#[tokio::test]
async fn push_accepts_staged_id_source_variant() {
    // §3/§6: PushSource{Path,StagedId}; push must accept both variants by shape.
    let (_gtree, root) = fixture_tree();
    let (_gstore, store) = file_store();
    let id = snapdir_api::stage(root.as_path(), &StageOptions::default()).expect("stage");
    let to = TransferOptions::default();
    let pushed: SnapshotId = snapdir_api::push(PushSource::StagedId(&id), &store, &to)
        .await
        .expect("push a staged id");
    assert_eq!(pushed, id, "pushing a StagedId yields that same id");
}

#[tokio::test]
async fn fetch_pull_checkout_round_trip_reproduces_the_tree() {
    // §6: fetch(id,store,o)->Result<()>; checkout(id,dest,o)->Result<()>;
    //     pull(id,store,dest,o)->Result<()>. Drive a full file:// round-trip.
    let (_gtree, root) = fixture_tree();
    let (_gstore, store) = file_store();
    // Hermetic cache: thread one per-test cache dir through every transfer +
    // checkout option so fetch writes and checkout reads the SAME private cache
    // instead of the shared global $HOME/.cache/snapdir.
    let cache_td = tempfile::tempdir().expect("cache tempdir");
    let to = TransferOptions {
        cache_dir: Some(cache_td.path().into()),
        ..Default::default()
    };
    let id = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
        .await
        .expect("push");

    // fetch: pull objects into the local cache; returns Result<()>.
    snapdir_api::fetch(&id, &store, &to)
        .await
        .expect("fetch objects from file:// store");

    // pull = fetch + checkout in one call into a fresh dest.
    let dest_pull = tempfile::tempdir().expect("dest pull");
    let co = CheckoutOptions {
        transfer: TransferOptions {
            cache_dir: Some(cache_td.path().into()),
            ..Default::default()
        },
        ..Default::default()
    };
    snapdir_api::pull(&id, &store, dest_pull.path(), &co)
        .await
        .expect("pull into a fresh dir");
    let reid_pull = snapdir_api::id(dest_pull.path(), &ManifestOptions::default()).expect("re-id");
    assert_eq!(
        reid_pull, id,
        "pull reproduces the tree byte-for-byte (same id)"
    );

    // checkout: materialize from the already-fetched cache into another fresh dest.
    let dest_checkout = tempfile::tempdir().expect("dest checkout");
    snapdir_api::checkout(&id, dest_checkout.path(), &co)
        .await
        .expect("checkout from cache");
    let reid_checkout =
        snapdir_api::id(dest_checkout.path(), &ManifestOptions::default()).expect("re-id");
    assert_eq!(
        reid_checkout, id,
        "checkout reproduces the same id as the source"
    );
}

#[tokio::test]
async fn sync_copies_a_snapshot_between_two_file_stores() {
    // §6: pub async fn sync(id:&SnapshotId, src:&StoreUri, dst:&StoreUri, o:&TransferOptions) -> Result<()>.
    let (_gtree, root) = fixture_tree();
    let (_gsrc, src) = file_store();
    let (_gdst, dst) = file_store();
    let to = TransferOptions::default();
    let id = snapdir_api::push(PushSource::Path(root.as_path()), &src, &to)
        .await
        .expect("push to src");
    snapdir_api::sync(&id, &src, &dst, &to)
        .await
        .expect("sync src -> dst");

    // After sync the snapshot must be checkout-able straight from dst.
    let dest = tempfile::tempdir().expect("dest");
    snapdir_api::pull(&id, &dst, dest.path(), &CheckoutOptions::default())
        .await
        .expect("pull from dst after sync");
    let reid = snapdir_api::id(dest.path(), &ManifestOptions::default()).expect("re-id");
    assert_eq!(reid, id, "synced-to store serves the identical snapshot");
}

#[tokio::test]
async fn verify_returns_a_verify_result_for_a_pushed_snapshot() {
    // §6: pub async fn verify(id:&SnapshotId, store:&StoreUri, o:&VerifyOptions) -> Result<VerifyResult>.
    let (_gtree, root) = fixture_tree();
    let (_gstore, store) = file_store();
    let to = TransferOptions::default();
    let id = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
        .await
        .expect("push");
    let vr: VerifyResult = snapdir_api::verify(&id, &store, &VerifyOptions::default())
        .await
        .expect("verify a healthy pushed snapshot");
    // §3: VerifyResult{ pub ok: bool, ... }. A freshly-pushed snapshot must verify ok.
    assert!(vr.ok, "a freshly pushed snapshot verifies ok");
}

#[tokio::test]
async fn diff_returns_structured_diff_entries() {
    // §6: pub async fn diff(o:&DiffOptions) -> Result<Vec<DiffEntry>>.
    // §3: DiffEntry{ status: DiffStatus, path: PathBuf }. Surface: typed Vec<DiffEntry>.
    let (_gtree, root) = fixture_tree();
    let (_gsrc, src) = file_store();
    let (_gdst, dst) = file_store();
    let to = TransferOptions::default();

    // Push the tree to src, then a mutated tree to dst, and diff the two stores.
    let _id_src = snapdir_api::push(PushSource::Path(root.as_path()), &src, &to)
        .await
        .expect("push src");
    std::fs::write(root.join("a.txt"), b"hello snapdir MODIFIED\n").unwrap();
    std::fs::write(root.join("new.txt"), b"new file\n").unwrap();
    let _id_dst = snapdir_api::push(PushSource::Path(root.as_path()), &dst, &to)
        .await
        .expect("push dst");

    let opts = DiffOptions {
        from: vec![src.clone()],
        to: vec![dst.clone()],
        ..DiffOptions::default()
    };
    let entries: Vec<DiffEntry> = snapdir_api::diff(&opts).await.expect("diff two stores");
    // The mutated/added files must surface as structured entries with a DiffStatus + path.
    assert!(
        !entries.is_empty(),
        "diff of differing trees yields entries"
    );
    let _statuses: Vec<&DiffStatus> = entries.iter().map(|e| &e.status).collect();
    let paths: Vec<&PathBuf> = entries.iter().map(|e| &e.path).collect();
    assert!(
        paths
            .iter()
            .any(|p| p.ends_with("a.txt") || p.ends_with("new.txt")),
        "diff surfaces the changed/added path(s)"
    );
}

// ===========================================================================
// §6 VERIFICATION / CACHE (SYNC) — presence + typed shape
// ===========================================================================

#[test]
fn verify_cache_returns_a_verify_cache_result() {
    // §6: pub fn verify_cache(o:&VerifyCacheOptions) -> Result<VerifyCacheResult>.
    // Hermetic isolation: verify a per-test cache dir (honored by verify_cache()),
    // never the shared global cache, so a concurrent flush/stage in another test
    // can't perturb it.
    let cache_td = tempfile::tempdir().expect("cache tempdir");
    let res: VerifyCacheResult = snapdir_api::verify_cache(&VerifyCacheOptions {
        cache_dir: Some(cache_td.path().into()),
    })
    .expect("verify_cache");
    // §3: VerifyCacheResult{ pub ok: bool, ... } — an empty/fresh cache verifies ok.
    let _ok: bool = res.ok;
}

#[test]
fn flush_cache_returns_unit_result() {
    // §6: pub fn flush_cache(o:&CacheOptions) -> Result<()>.
    // Hermetic isolation (CRITICAL — this was the relfix-test-cache-isolation
    // race): flush a per-test cache dir, NOT the shared global cache.
    // flush_cache(default) rm-rf's $HOME/.cache/snapdir, which under parallel
    // execution wipes the objects/manifest another test just fetched/staged ->
    // StoreError(ManifestNotFound) at its checkout/push. Pointing flush at a
    // private tempdir (honored by flush_cache()) removes the destructive race.
    let cache_td = tempfile::tempdir().expect("cache tempdir");
    let r: Result<(), SnapdirError> = snapdir_api::flush_cache(&CacheOptions {
        cache_dir: Some(cache_td.path().into()),
    });
    r.expect("flush_cache on a (possibly empty) cache succeeds");
}

// ===========================================================================
// §6 CATALOG / HISTORY (SYNC) — presence + typed Vec shape
// ===========================================================================

#[test]
fn locations_returns_a_vec_of_location() {
    // §6: pub fn locations(o:&LocationsOptions) -> Result<Vec<Location>>.
    let locs: Vec<Location> =
        snapdir_api::locations(&LocationsOptions::default()).expect("locations");
    // Typed surface check: a Vec<Location> (possibly empty on a fresh catalog).
    let _len = locs.len();
}

#[test]
fn ancestors_takes_an_id_and_returns_a_vec_of_ancestor() {
    // §6: pub fn ancestors(id:&SnapshotId, o:&AncestorsOptions) -> Result<Vec<Ancestor>>.
    let (_g, root) = fixture_tree();
    let id = snapdir_api::id(root.as_path(), &ManifestOptions::default()).expect("id");
    let anc: Vec<Ancestor> =
        snapdir_api::ancestors(&id, &AncestorsOptions::default()).expect("ancestors");
    let _len = anc.len();
}

#[test]
fn revisions_takes_a_location_ref_and_returns_a_vec_of_revision() {
    // §6: pub fn revisions(location:&LocationRef, o:&RevisionsOptions) -> Result<Vec<Revision>>.
    // LocationRef is the catalog's location reference type re-exported by snapdir-api.
    let loc_ref: snapdir_api::LocationRef = snapdir_api::LocationRef::default();
    let revs: Vec<Revision> =
        snapdir_api::revisions(&loc_ref, &RevisionsOptions::default()).expect("revisions");
    let _len = revs.len();
}

// ===========================================================================
// §6 UTILITIES (SYNC)
// ===========================================================================

#[test]
fn version_is_a_non_empty_static_str() {
    // §6: pub fn version() -> &'static str  (tracks the snapdir CLI version).
    let v: &'static str = snapdir_api::version();
    assert!(!v.is_empty(), "version() returns a non-empty string");
    // Must look like a semver (at least one dot), since it tracks the CLI version.
    assert!(
        v.contains('.'),
        "version() looks like a semver string: {v:?}"
    );
}

#[test]
fn defaults_returns_an_effective_config() {
    // §6: pub fn defaults() -> EffectiveConfig.  §3: EffectiveConfig{ resolved defaults + source }.
    let cfg: EffectiveConfig = snapdir_api::defaults();
    // Surface check only: the value exists and is the EffectiveConfig type (Debug-formattable).
    let _ = format!("{cfg:?}");
}

// ===========================================================================
// REVIEW: §6 CATALOG / HISTORY — REAL catalog wiring (pressure-test the stub gap)
//
// snapdir-catalog is a *normal* dependency of snapdir-api (see Cargo.toml
// [dependencies]). §1 of the locked spec says the facade wraps
// `snapdir-catalog::Catalog::{locations,ancestors,revisions}` -> Vec<Record>.
// §6 declares locations()/ancestors()/revisions() returning typed Vecs.
//
// The impl currently hard-codes `Ok(vec![])` ("no catalog configured in M0"),
// so these tests open a REAL Catalog, populate it, and assert the facade reads
// it back. If the facade returns empty here, that is a REAL BUG (the read
// functions are present + typed but never touch a catalog) -> the PM reopens
// `m0-api-surface-impl` for the api lane to wire `snapdir_catalog::Catalog`
// into these three functions (including exposing a catalog/db-path seam on the
// option structs, which today are empty markers with NO field pointing at a
// catalog).
// ===========================================================================

/// 64-char snapshot ids (the catalog stores ids as opaque strings).
const CAT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CAT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const CAT_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

/// Open a catalog at a fresh temp path and save a small, deterministic history:
///   s3://foo : A (root)
///   s3://bar : A (root) -> C
///   /local/x : A (root) -> B -> C
/// using a FixedClock so created_at ordering is deterministic. Returns the
/// (tempdir-guard, db-path) so the caller can point the facade at it.
fn populated_catalog() -> (tempfile::TempDir, PathBuf) {
    use snapdir_catalog::{Catalog, FixedClock};

    let td = tempfile::tempdir().expect("catalog tempdir");
    let db_path = td.path().join("catalog.redb");
    let cat = Catalog::open(&db_path).expect("open catalog");
    let clock = FixedClock::new(
        [
            "2026-06-01 00:00:00.001",
            "2026-06-01 00:00:00.002",
            "2026-06-01 00:00:00.003",
            "2026-06-01 00:00:00.004",
            "2026-06-01 00:00:00.005",
            "2026-06-01 00:00:00.006",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect(),
    );
    cat.save("s3://foo", CAT_A, &clock).expect("save foo A");
    cat.save("s3://bar", CAT_A, &clock).expect("save bar A");
    cat.save("/local/x", CAT_A, &clock).expect("save x A");
    cat.save("/local/x", CAT_B, &clock).expect("save x B");
    cat.save("/local/x", CAT_C, &clock).expect("save x C");
    cat.save("s3://bar", CAT_C, &clock).expect("save bar C");

    // Sanity: the catalog crate itself returns what we saved (proves the
    // dependency works and the rows exist — so any empty result from the
    // facade is the FACADE's bug, not the catalog's).
    let locs = cat.locations().expect("catalog.locations");
    assert_eq!(locs.len(), 3, "catalog has 3 locations directly");
    let revs = cat.revisions("/local/x").expect("catalog.revisions");
    assert_eq!(revs.len(), 3, "/local/x has 3 revisions directly");
    // C was saved at BOTH /local/x (prev B) and s3://bar (prev A), so an
    // unfiltered ancestors(C) returns one row per location (2 rows); the
    // s3://bar filter narrows it to the single bar row (prev A).
    let anc = cat.ancestors(CAT_C, None).expect("catalog.ancestors");
    assert_eq!(
        anc.len(),
        2,
        "C has an ancestor row at each of its 2 locations"
    );
    let anc_bar = cat
        .ancestors(CAT_C, Some("s3://bar"))
        .expect("catalog.ancestors bar");
    assert_eq!(
        anc_bar.len(),
        1,
        "C@s3://bar has exactly one ancestor row (prev A)"
    );
    assert_eq!(
        anc_bar[0].id, CAT_A,
        "C@s3://bar ancestor id is previous_id A"
    );

    (td, db_path)
}

/// Point a `LocationsOptions` at the given catalog db path, however the facade
/// exposes that seam. The locked spec (§1) requires the facade to read a real
/// catalog; the impl MUST provide a way to select one. This helper centralizes
/// the seam so that, once wired, all three catalog tests use it.
///
/// As of the impl under review there is NO such field on the option structs
/// (they are empty markers), so the catalog tests below assert against a
/// `Default` options value and WILL FAIL on the empty-vec stub — that failure
/// is the real-bug signal for the PM.
fn locations_for(db_path: &Path) -> Vec<Location> {
    // Best-effort: expose the db via env so a wired impl that resolves a
    // catalog from $SNAPDIR_CATALOG_DB / cache-dir can find it. Harmless if the
    // impl ignores it.
    std::env::set_var("SNAPDIR_CATALOG_DB_PATH", db_path);
    snapdir_api::locations(&LocationsOptions::default()).expect("locations")
}

#[test]
fn locations_reads_a_real_populated_catalog() {
    // §1/§6: locations() must return the catalog's per-location latest records.
    let (_g, db_path) = populated_catalog();
    let locs = locations_for(&db_path);

    // REAL BUG TRIPWIRE: the catalog has 3 locations; the facade must surface
    // them. The stub returns an empty Vec -> this assertion fails -> reopen impl.
    assert_eq!(
        locs.len(),
        3,
        "locations() must return the 3 records saved in the catalog, not an empty stub"
    );
    // Latest id per location: s3://foo -> A, s3://bar -> C, /local/x -> C.
    let id_of = |loc: &str| {
        locs.iter()
            .find(|r| r.location == loc)
            .unwrap_or_else(|| panic!("location {loc} missing from locations()"))
            .id
            .clone()
    };
    assert_eq!(id_of("s3://foo"), CAT_A, "s3://foo latest id is A");
    assert_eq!(id_of("s3://bar"), CAT_C, "s3://bar latest id is C");
    assert_eq!(id_of("/local/x"), CAT_C, "/local/x latest id is C");
    // created_at is carried through (non-empty, oracle-format).
    assert!(
        locs.iter().all(|r| !r.created_at.is_empty()),
        "each Location carries a non-empty created_at"
    );
}

#[test]
fn ancestors_reads_a_real_populated_catalog() {
    // §1/§6: ancestors(id) must return the catalog's ancestor rows for that id.
    let (_g, db_path) = populated_catalog();
    std::env::set_var("SNAPDIR_CATALOG_DB_PATH", &db_path);

    // C's ancestor rows: it was recorded at /local/x (previous_id B) and at
    // s3://bar (previous_id A). The catalog's ancestors() projects previous_id
    // into the id field, so the API should report one Ancestor per location.
    let id_c = SnapshotId::from_hex(CAT_C).expect("parse C");
    let anc: Vec<Ancestor> =
        snapdir_api::ancestors(&id_c, &AncestorsOptions::default()).expect("ancestors");
    assert_eq!(
        anc.len(),
        2,
        "ancestors(C) must return the 2 ancestor rows saved in the catalog, not an empty stub"
    );
    // The s3://bar ancestor projects previous_id A; the /local/x one projects B.
    let bar = anc
        .iter()
        .find(|a| a.location == "s3://bar")
        .expect("s3://bar ancestor row present");
    assert_eq!(
        bar.id, CAT_A,
        "the s3://bar C-ancestor's id is its previous_id (A)"
    );
    let local = anc
        .iter()
        .find(|a| a.location == "/local/x")
        .expect("/local/x ancestor row present");
    assert_eq!(
        local.id, CAT_B,
        "the /local/x C-ancestor's id is its previous_id (B)"
    );

    // A root id (A) has no ancestors -> empty, but that empty must come from a
    // REAL catalog read, not the stub. We can only distinguish via the C case
    // above (non-empty), which the stub cannot satisfy.
    let id_a = SnapshotId::from_hex(CAT_A).expect("parse A");
    let anc_a: Vec<Ancestor> =
        snapdir_api::ancestors(&id_a, &AncestorsOptions::default()).expect("ancestors A");
    assert!(anc_a.is_empty(), "a root id has no ancestors");
}

#[test]
fn revisions_reads_a_real_populated_catalog() {
    // §1/§6: revisions(location) must return the catalog's revision history.
    let (_g, db_path) = populated_catalog();
    std::env::set_var("SNAPDIR_CATALOG_DB_PATH", &db_path);

    // /local/x history: A (root) -> B -> C, returned created_at DESC.
    let loc_ref = snapdir_api::LocationRef::new("/local/x");
    let revs: Vec<Revision> =
        snapdir_api::revisions(&loc_ref, &RevisionsOptions::default()).expect("revisions");
    assert_eq!(
        revs.len(),
        3,
        "revisions(/local/x) must return the 3 revisions saved in the catalog, not an empty stub"
    );
    // DESC: C (prev B), B (prev A), A (prev None/root).
    assert_eq!(revs[0].id, CAT_C, "newest revision is C");
    assert_eq!(
        revs[0].previous_id.as_deref(),
        Some(CAT_B),
        "C's previous_id is B"
    );
    assert_eq!(revs[1].id, CAT_B, "middle revision is B");
    assert_eq!(
        revs[1].previous_id.as_deref(),
        Some(CAT_A),
        "B's previous_id is A"
    );
    assert_eq!(revs[2].id, CAT_A, "oldest revision is A (root)");
    assert_eq!(
        revs[2].previous_id, None,
        "the root revision has no previous_id"
    );
}

// ===========================================================================
// REVIEW: impl-revealed round-trip behavior (now that src is visible)
// ===========================================================================

/// Read a file's bytes (helper for content-fidelity assertions).
fn read_bytes(p: &Path) -> Vec<u8> {
    std::fs::read(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

#[tokio::test]
async fn push_path_and_staged_id_yield_the_same_content_addressed_id() {
    // Impl-revealed: PushSource::Path walks+hashes inline; PushSource::StagedId
    // loads the cached manifest and syncs it. Both must produce the SAME id for
    // the same tree (content-addressed), and both must land a usable snapshot.
    let (_gtree, root) = fixture_tree();
    let (_gpath_store, path_store) = file_store();
    let (_gstaged_store, staged_store) = file_store();
    let to = TransferOptions::default();

    let via_path = snapdir_api::push(PushSource::Path(root.as_path()), &path_store, &to)
        .await
        .expect("push via Path");

    let staged = snapdir_api::stage(root.as_path(), &StageOptions::default()).expect("stage");
    let via_staged = snapdir_api::push(PushSource::StagedId(&staged), &staged_store, &to)
        .await
        .expect("push via StagedId");

    assert_eq!(
        via_path, via_staged,
        "Path and StagedId push yield the same id"
    );
    assert_eq!(via_path, staged, "the pushed id equals the staged id");

    // Both stores now serve a verifiable snapshot of the same tree.
    assert!(
        snapdir_api::verify(&via_path, &path_store, &VerifyOptions::default())
            .await
            .expect("verify path-store")
            .ok,
        "Path-pushed snapshot verifies ok"
    );
    assert!(
        snapdir_api::verify(&via_staged, &staged_store, &VerifyOptions::default())
            .await
            .expect("verify staged-store")
            .ok,
        "StagedId-pushed snapshot verifies ok"
    );
}

#[tokio::test]
async fn checkout_materializes_exact_file_bytes() {
    // Impl-revealed: checkout() loads the cached manifest and fetch_files() into
    // dest. Assert the materialized bytes are byte-for-byte the source's, not
    // merely that the re-id matches.
    let (_gtree, root) = fixture_tree();
    let (_gstore, store) = file_store();
    // Hermetic cache: one per-test cache dir shared by fetch (writer) and
    // checkout (reader), never the global $HOME/.cache/snapdir.
    let cache_td = tempfile::tempdir().expect("cache tempdir");
    let to = TransferOptions {
        cache_dir: Some(cache_td.path().into()),
        ..Default::default()
    };
    let id = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
        .await
        .expect("push");
    // checkout serves from the local cache, so fetch into the cache first.
    snapdir_api::fetch(&id, &store, &to)
        .await
        .expect("fetch into cache");

    let dest = tempfile::tempdir().expect("dest");
    let co = CheckoutOptions {
        transfer: TransferOptions {
            cache_dir: Some(cache_td.path().into()),
            ..Default::default()
        },
        ..Default::default()
    };
    snapdir_api::checkout(&id, dest.path(), &co)
        .await
        .expect("checkout");

    // Exact content fidelity for each source file.
    for rel in ["a.txt", "sub/b.bin", "empty.txt"] {
        let src_bytes = read_bytes(&root.join(rel));
        let dst_bytes = read_bytes(&dest.path().join(rel));
        assert_eq!(
            src_bytes, dst_bytes,
            "checkout reproduces {rel} byte-for-byte"
        );
    }
    // And the re-id confirms the whole tree.
    let reid = snapdir_api::id(dest.path(), &ManifestOptions::default()).expect("re-id");
    assert_eq!(reid, id, "checkout reproduces the exact tree id");
}

#[tokio::test]
async fn sync_makes_the_id_fetchable_from_the_destination_store() {
    // Impl-revealed: sync() copies manifest + objects src->dst via sync_snapshot.
    // After sync, the dst store alone must serve the snapshot (verify + pull),
    // WITHOUT touching src again.
    let (_gtree, root) = fixture_tree();
    let (_gsrc, src) = file_store();
    let (_gdst, dst) = file_store();
    let to = TransferOptions::default();
    let id = snapdir_api::push(PushSource::Path(root.as_path()), &src, &to)
        .await
        .expect("push to src");

    snapdir_api::sync(&id, &src, &dst, &to)
        .await
        .expect("sync src->dst");

    // dst serves a healthy snapshot on its own.
    assert!(
        snapdir_api::verify(&id, &dst, &VerifyOptions::default())
            .await
            .expect("verify on dst")
            .ok,
        "synced snapshot verifies ok on the destination store"
    );
    let dest = tempfile::tempdir().expect("dest");
    snapdir_api::pull(&id, &dst, dest.path(), &CheckoutOptions::default())
        .await
        .expect("pull from dst");
    let reid = snapdir_api::id(dest.path(), &ManifestOptions::default()).expect("re-id");
    assert_eq!(
        reid, id,
        "the dst store serves the identical snapshot after sync"
    );
}

#[tokio::test]
async fn diff_classifies_added_deleted_modified_unchanged() {
    // Impl-revealed: diff() unions manifests per side into path->checksum maps
    // and classifies A/D/M/=, with `all` toggling Unchanged. Build a from-tree
    // and a to-tree that exercise every status, then assert the classification.
    let from_dir = tempfile::tempdir().expect("from dir");
    let to_dir = tempfile::tempdir().expect("to dir");
    // from side: keep.txt (unchanged), mod.txt (modified), gone.txt (deleted).
    std::fs::write(from_dir.path().join("keep.txt"), b"same\n").unwrap();
    std::fs::write(from_dir.path().join("mod.txt"), b"original\n").unwrap();
    std::fs::write(from_dir.path().join("gone.txt"), b"will be deleted\n").unwrap();
    // to side: keep.txt (same), mod.txt (changed), new.txt (added). gone.txt absent.
    std::fs::write(to_dir.path().join("keep.txt"), b"same\n").unwrap();
    std::fs::write(to_dir.path().join("mod.txt"), b"CHANGED\n").unwrap();
    std::fs::write(to_dir.path().join("new.txt"), b"brand new\n").unwrap();

    let (_gsrc, src) = file_store();
    let (_gdst, dst) = file_store();
    let to_opts = TransferOptions::default();
    snapdir_api::push(PushSource::Path(from_dir.path()), &src, &to_opts)
        .await
        .expect("push from");
    snapdir_api::push(PushSource::Path(to_dir.path()), &dst, &to_opts)
        .await
        .expect("push to");

    // Default (all=false): no Unchanged entries.
    let opts = DiffOptions {
        from: vec![src.clone()],
        to: vec![dst.clone()],
        ..DiffOptions::default()
    };
    let entries = snapdir_api::diff(&opts).await.expect("diff");
    let status_of = |name: &str| -> Option<DiffStatus> {
        entries
            .iter()
            .find(|e| e.path.ends_with(name))
            .map(|e| e.status)
    };
    assert_eq!(
        status_of("mod.txt"),
        Some(DiffStatus::Modified),
        "mod.txt is Modified"
    );
    assert_eq!(
        status_of("new.txt"),
        Some(DiffStatus::Added),
        "new.txt is Added"
    );
    assert_eq!(
        status_of("gone.txt"),
        Some(DiffStatus::Deleted),
        "gone.txt is Deleted"
    );
    assert_eq!(
        status_of("keep.txt"),
        None,
        "keep.txt is Unchanged and OMITTED when all=false"
    );
    assert!(
        entries.iter().all(|e| e.status != DiffStatus::Unchanged),
        "no Unchanged entries surface without all"
    );

    // all=true: keep.txt now surfaces as Unchanged.
    let opts_all = DiffOptions {
        from: vec![src],
        to: vec![dst],
        all: true,
        ..DiffOptions::default()
    };
    let entries_all = snapdir_api::diff(&opts_all).await.expect("diff all");
    let keep_all = entries_all
        .iter()
        .find(|e| e.path.ends_with("keep.txt"))
        .map(|e| e.status);
    assert_eq!(
        keep_all,
        Some(DiffStatus::Unchanged),
        "keep.txt surfaces as Unchanged when all=true"
    );
    // entries are sorted by path (deterministic output).
    let mut sorted = entries_all.clone();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(entries_all, sorted, "diff output is sorted by path");
}

#[tokio::test]
async fn verify_ok_true_on_healthy_store_and_errors_on_corruption() {
    // Impl-revealed: verify() loads the manifest then get_object()s every file
    // object (which BLAKE3-checks on read). Healthy -> ok:true. Corrupt an
    // object's bytes in place (keeping its address) -> the integrity check on
    // read must surface an error (HASH_MISMATCH) or a store error, NOT ok:true.
    let (_gtree, root) = fixture_tree();
    let (gstore, store) = file_store();
    let to = TransferOptions::default();
    let id = snapdir_api::push(PushSource::Path(root.as_path()), &store, &to)
        .await
        .expect("push");

    // Healthy store verifies ok.
    let healthy = snapdir_api::verify(&id, &store, &VerifyOptions::default())
        .await
        .expect("verify healthy");
    assert!(healthy.ok, "a freshly pushed snapshot verifies ok");

    // Corrupt every object file under the store's .objects pool: overwrite the
    // content while leaving the (now-wrong) address path intact. A read-time
    // BLAKE3 verification must reject this.
    let objects_root = gstore.path().join(".objects");
    let mut corrupted = 0usize;
    if objects_root.exists() {
        for entry in walk_files(&objects_root) {
            std::fs::write(&entry, b"CORRUPTED-PAYLOAD-NOT-MATCHING-ADDRESS").unwrap();
            corrupted += 1;
        }
    }
    assert!(
        corrupted > 0,
        "test must actually corrupt at least one object"
    );

    let result = snapdir_api::verify(&id, &store, &VerifyOptions::default()).await;
    match result {
        Ok(vr) => assert!(
            !vr.ok,
            "verify() over a corrupted store must NOT report ok:true"
        ),
        Err(e) => {
            // An integrity failure surfaces as HASH_MISMATCH; a missing/garbled
            // object may surface as STORE_ERROR. Either is an acceptable failure
            // signal — what's forbidden is a clean ok:true.
            let code = e.code();
            assert!(
                code == "HASH_MISMATCH" || code == "STORE_ERROR" || code == "IO_ERROR",
                "corruption surfaces a failure code, got {code}"
            );
        }
    }
}

/// Recursively collect all regular files under `dir` (no external deps).
fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                out.extend(walk_files(&p));
            } else if p.is_file() {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn id_from_manifest_equals_id_for_the_same_tree_strict() {
    // Impl-revealed: id_from_manifest re-parses m.raw and re-hashes — it must
    // equal id(path) exactly (the merkle root is the manifest hash). Strengthens
    // the existing case with an explicit hex-equality assertion.
    let (_g, root) = fixture_tree();
    let o = ManifestOptions::default();
    let m = snapdir_api::manifest(root.as_path(), &o).expect("manifest");
    let from_m = snapdir_api::id_from_manifest(&m);
    let direct = snapdir_api::id(root.as_path(), &o).expect("id");
    assert_eq!(from_m, direct, "id_from_manifest == id (SnapshotId eq)");
    assert_eq!(
        from_m.to_hex(),
        direct.to_hex(),
        "id_from_manifest == id (64-hex equality)"
    );
}
