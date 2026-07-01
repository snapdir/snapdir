//! snapdir-api multi-store dispatch contract (black-box).
//!
//! These integration tests pin the invariant that the async facade —
//! `snapdir_api::{push, fetch, pull, checkout, sync}` — routes a **non-`file://`**
//! store URI to the **real** backend (`S3Store`, `B2Store`, the SSH/SFTP path,
//! `GcsStore`), and never silently falls back to a local `FileStore`.
//!
//! ## The bug under test
//! `StoreUri::parse` accepts `file/s3/gs/b2/ssh/sftp`, but the file-only facade
//! constructs `FileStore::new(store_str)` for **every** scheme. Because
//! `FileStore`'s path parser only strips a `file:` prefix, an `s3://bucket/prefix`
//! URI is taken verbatim as a *relative* directory and written to a literal
//! `./s3:` tree on local disk — so nothing ever reaches S3. Each scheme leaks a
//! `<scheme>:` directory (`s3:`, `gs:`, `b2:`, `ssh:`, `sftp:`) in the process cwd.
//!
//! Authored from the snapdir-api PUBLIC surface + the `snapdir-stores`
//! `Store`/`StreamStore` contract only (no facade `src/` was read).
//!
//! ## Sidecars
//! The positive "objects actually landed in the store" round-trips need the
//! minio sidecar (`SNAPDIR_S3_STORE_ENDPOINT_URL=http://127.0.0.1:9000`,
//! `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`/`AWS_DEFAULT_REGION`). When that
//! env is absent each minio-backed test prints a visible `SKIP` and returns, so
//! it is meaningful only when the sidecar is up. The *local-disk-leak* checks
//! (the core bug discriminator) need NO sidecar and run everywhere.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use snapdir_api::{
    self as api, CheckoutOptions, ManifestOptions, PushSource, SnapshotId, StoreUri,
    TransferOptions,
};
use snapdir_core::manifest::PathType;
use snapdir_core::store::Store; // get_manifest
use snapdir_stores::stream::StreamStore; // has_object
use snapdir_stores::{B2Store, S3Store};

// ---------------------------------------------------------------------------
// Shared test scaffolding
// ---------------------------------------------------------------------------

/// Serializes every cwd-sensitive `push` (the leak check temporarily `chdir`s
/// into a throwaway tempdir so a `FileStore` fallback writes its `<scheme>:`
/// turd there — fully contained — instead of polluting the crate dir).
static CWD_LOCK: Mutex<()> = Mutex::new(());

/// A per-process unique token (`pid-nanos-counter`) for hermetic bucket
/// prefixes / temp names, so repeat runs never collide.
fn unique(tag: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{tag}-{}-{nanos}-{n}", std::process::id())
}

/// Points the snapdir cache (`cache_dir_default` reads `XDG_CACHE_HOME`) at a
/// throwaway dir so `fetch` → `checkout`/`pull` round-trips stay hermetic and
/// never touch the developer's real `~/.cache/snapdir`.
fn ensure_isolated_cache() {
    static CELL: OnceLock<PathBuf> = OnceLock::new();
    let dir = CELL.get_or_init(|| {
        let d = std::env::temp_dir().join(unique("snapdir-api-multistore-xdg"));
        std::fs::create_dir_all(&d).expect("create isolated cache dir");
        d
    });
    std::env::set_var("XDG_CACHE_HOME", dir);
}

/// Builds a small deterministic tree: a text file, a 0-byte file (object-store
/// boundary), and a subdir holding a binary file + a unicode/space-named file.
/// Deterministic content => a stable snapshot id across runs.
fn build_tree(root: &Path) {
    use std::fs;
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("alpha.txt"), b"alpha-contents\n").unwrap();
    fs::write(root.join("empty"), b"").unwrap();
    let sub = root.join("sub dir");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("beta.bin"), [0u8, 1, 2, 3, 4, 5, 255, 7]).unwrap();
    fs::write(sub.join("ünïcödé.txt"), "snÅpdir-π\n".as_bytes()).unwrap();
}

/// Drives a future to completion on a throwaway current-thread runtime, then
/// detaches it (`shutdown_background`) so a backend that hangs on a missing
/// network/credential can never wedge the test on runtime drop.
fn block<F: std::future::Future>(fut: F) -> F::Output {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");
    let out = rt.block_on(fut);
    rt.shutdown_background();
    out
}

/// Runs `push(Path, uri)` with the process cwd temporarily set to a fresh
/// tempdir, bounded by `secs`. Returns `(pushed id or None, leaked)`, where
/// `leaked` is `true` iff a literal `<scheme>:` directory was created on local
/// disk — i.e. the `FileStore` fallback fired. The tempdir (and any leaked
/// turd) is removed on return.
fn push_isolated(src: &Path, uri: &StoreUri, secs: u64) -> (Option<SnapshotId>, bool) {
    let work = tempfile::tempdir().expect("tempdir");
    let scheme = uri.scheme().to_string();
    let src = src.to_path_buf();
    let uri = uri.clone();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build runtime");

    let guard = CWD_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let prev = std::env::current_dir().expect("read cwd");
    std::env::set_current_dir(work.path()).expect("chdir into isolated work dir");
    let outcome = rt.block_on(async {
        tokio::time::timeout(
            std::time::Duration::from_secs(secs),
            api::push(PushSource::Path(&src), &uri, &TransferOptions::default()),
        )
        .await
    });
    std::env::set_current_dir(&prev).expect("restore cwd");
    // A `FileStore` fallback writes `<root>/.objects/...`; for `s3://bucket/...`
    // the first path component is the literal `s3:` segment.
    let leaked = work.path().join(format!("{scheme}:")).exists();
    drop(guard);
    rt.shutdown_background();

    let id = match outcome {
        Ok(Ok(id)) => Some(id),
        _ => None, // push errored OR timed out (network attempt with no backend)
    };
    (id, leaked)
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

// ---------------------------------------------------------------------------
// minio sidecar helpers
// ---------------------------------------------------------------------------

struct MinioEnv {
    endpoint: String,
    access: String,
    secret: String,
}

/// Resolves the minio sidecar env, or `None` when the sidecar is not wired.
fn minio_env() -> Option<MinioEnv> {
    let endpoint = std::env::var("SNAPDIR_S3_STORE_ENDPOINT_URL").ok()?;
    if endpoint.trim().is_empty() {
        return None;
    }
    let access = std::env::var("AWS_ACCESS_KEY_ID").ok()?;
    let secret = std::env::var("AWS_SECRET_ACCESS_KEY").ok()?;
    if access.is_empty() || secret.is_empty() {
        return None;
    }
    Some(MinioEnv {
        endpoint,
        access,
        secret,
    })
}

/// Creates an S3 bucket on the minio endpoint (idempotent), mirroring the
/// Python-stdlib SigV4 fallback in `tests/golden/run_parity.sh::create_s3_bucket`
/// (no `aws`/`mc`/SDK dev-dep needed). Returns `true` on success or 409-exists.
fn create_bucket(endpoint: &str, bucket: &str, access: &str, secret: &str) -> bool {
    // Indented exactly as a -c program; argv[1..]=endpoint,bucket,access,secret.
    const PY: &str = r#"
import hashlib, hmac, datetime, urllib.request, urllib.error, sys
endpoint, bucket, access, secret = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
region = 'us-east-1'; service = 's3'
now = datetime.datetime.utcnow()
date = now.strftime('%Y%m%d'); amz_dt = now.strftime('%Y%m%dT%H%M%SZ')
host = endpoint.split('//')[1]
payload_hash = hashlib.sha256(b'').hexdigest()
can_hdrs = f'host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_dt}\n'
signed_hdrs = 'host;x-amz-content-sha256;x-amz-date'
can_req = f'PUT\n/{bucket}\n\n{can_hdrs}\n{signed_hdrs}\n{payload_hash}'
cred_scope = f'{date}/{region}/{service}/aws4_request'
sts = f'AWS4-HMAC-SHA256\n{amz_dt}\n{cred_scope}\n' + hashlib.sha256(can_req.encode()).hexdigest()
def sign(key, msg): return hmac.new(key, msg.encode(), hashlib.sha256).digest()
k = sign(sign(sign(sign(f'AWS4{secret}'.encode(), date), region), service), 'aws4_request')
sig = hmac.new(k, sts.encode(), hashlib.sha256).hexdigest()
auth = f'AWS4-HMAC-SHA256 Credential={access}/{cred_scope}, SignedHeaders={signed_hdrs}, Signature={sig}'
req = urllib.request.Request(f'{endpoint}/{bucket}', data=b'', method='PUT')
req.add_header('Authorization', auth); req.add_header('X-Amz-Date', amz_dt)
req.add_header('X-Amz-Content-Sha256', payload_hash)
try:
    urllib.request.urlopen(req); sys.exit(0)
except urllib.error.HTTPError as e:
    sys.exit(0 if e.code == 409 else 1)
except Exception:
    sys.exit(1)
"#;
    std::process::Command::new("python3")
        .arg("-c")
        .arg(PY)
        .arg(endpoint)
        .arg(bucket)
        .arg(secret_arg(access))
        .arg(secret_arg(secret))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// Tiny indirection so the access/secret args are passed positionally without
// being mistaken for flags (they are loopback-only non-secrets anyway).
fn secret_arg(s: &str) -> String {
    s.to_string()
}

/// Asserts the manifest for `id` AND every File object it references are
/// physically present in `store` (the real bucket), not on local disk.
fn assert_objects_in_store<S: Store + StreamStore>(store: &S, id: &SnapshotId) {
    let manifest = store
        .get_manifest(&id.to_hex())
        .expect("manifest must be present (and id-consistent) in the remote store");
    let files: Vec<_> = manifest
        .entries()
        .iter()
        .filter(|e| e.path_type == PathType::File)
        .collect();
    assert!(
        !files.is_empty(),
        "round-trip tree must contain file objects to verify"
    );
    for e in files {
        assert!(
            store.has_object(&e.checksum).expect("has_object query"),
            "object {} (path {}) is missing from the remote store — push did not \
             reach the real backend",
            e.checksum,
            e.path
        );
    }
}

// ---------------------------------------------------------------------------
// 1+2. S3: real round-trip + objects-actually-in-S3 + no local s3: leak
// ---------------------------------------------------------------------------

/// CONTRACT: push/fetch/checkout against an `s3://` URI use the real `S3Store`
/// — objects + manifest land in the bucket, the snapshot byte-exactly
/// round-trips through S3, and NO local `s3:` directory is created.
#[test]
fn s3_round_trip_uses_real_store_not_local_filestore() {
    let Some(env) = minio_env() else {
        eprintln!(
            "SKIP s3_round_trip_uses_real_store_not_local_filestore: minio sidecar env \
             (SNAPDIR_S3_STORE_ENDPOINT_URL + AWS creds) absent"
        );
        return;
    };
    ensure_isolated_cache();

    let bucket = "snapdir-api-multistore";
    if !create_bucket(&env.endpoint, bucket, &env.access, &env.secret) {
        eprintln!(
            "SKIP s3_round_trip_uses_real_store_not_local_filestore: could not create minio \
             bucket {bucket} at {}",
            env.endpoint
        );
        return;
    }

    let prefix = unique("s3rt");
    let uri_str = format!("s3://{bucket}/{prefix}");
    let uri = StoreUri::parse(&uri_str).expect("valid s3 uri");

    let src_dir = tempfile::tempdir().expect("src tempdir");
    let src = src_dir.path().join("tree");
    build_tree(&src);
    let local_id = api::id(&src, &ManifestOptions::default()).expect("local id");

    // push (isolated cwd so a FileStore fallback would be caught as a leak).
    let (pushed, leaked) = push_isolated(&src, &uri, 180);
    assert!(
        !leaked,
        "BUG: push to {uri_str} created a literal `s3:` directory on local disk \
         (FileStore fallback) instead of using S3Store"
    );
    let pushed = pushed.expect("push to s3 must succeed against minio");
    assert!(is_hex64(&pushed.to_hex()), "push must return a 64-hex id");
    assert_eq!(
        pushed, local_id,
        "pushed id must equal the locally computed id"
    );

    // The objects + manifest must actually be in the bucket (the core bug check).
    let verify = S3Store::connect(&uri_str, Some(&env.endpoint)).expect("connect verify S3Store");
    assert_objects_in_store(&verify, &local_id);

    // fetch (S3 -> cache) then checkout (cache -> dest) must byte-exactly restore.
    block(api::fetch(&local_id, &uri, &TransferOptions::default())).expect("fetch from s3");
    let co_dir = tempfile::tempdir().expect("checkout tempdir");
    let co_dest = co_dir.path().join("out");
    block(api::checkout(
        &local_id,
        &co_dest,
        &CheckoutOptions::default(),
    ))
    .expect("checkout");
    let co_id = api::id(&co_dest, &ManifestOptions::default()).expect("re-id checkout dest");
    assert_eq!(
        co_id, local_id,
        "checkout of the S3-fetched snapshot must re-id to the same snapshot id"
    );

    // pull (one-shot fetch+materialize straight from S3) must also round-trip.
    let pull_dir = tempfile::tempdir().expect("pull tempdir");
    let pull_dest = pull_dir.path().join("out");
    block(api::pull(
        &local_id,
        &uri,
        &pull_dest,
        &CheckoutOptions::default(),
    ))
    .expect("pull from s3");
    let pull_id = api::id(&pull_dest, &ManifestOptions::default()).expect("re-id pull dest");
    assert_eq!(
        pull_id, local_id,
        "pull straight from S3 must re-id to the same snapshot id"
    );
}

// ---------------------------------------------------------------------------
// 3a. B2 (minio-backed): real round-trip through B2Store + no local b2: leak
// ---------------------------------------------------------------------------

/// CONTRACT: a `b2://` URI dispatches to the real `B2Store` (S3-compatible),
/// objects land in the bucket, the snapshot round-trips, and NO local `b2:`
/// directory is created. B2Store reads its endpoint from `SNAPDIR_B2_TEST_ENDPOINT`,
/// which we point at the same minio sidecar for a hermetic, emulator-backed run.
#[test]
fn b2_round_trip_uses_real_store_not_local_filestore() {
    let Some(env) = minio_env() else {
        eprintln!(
            "SKIP b2_round_trip_uses_real_store_not_local_filestore: minio sidecar env absent"
        );
        return;
    };
    ensure_isolated_cache();
    // Point the B2 backend (S3-compatible) at the same minio endpoint so the
    // facade's b2:// dispatch is exercised against the local emulator.
    std::env::set_var("SNAPDIR_B2_TEST_ENDPOINT", &env.endpoint);

    let bucket = "snapdir-api-multistore-b2";
    if !create_bucket(&env.endpoint, bucket, &env.access, &env.secret) {
        eprintln!(
            "SKIP b2_round_trip_uses_real_store_not_local_filestore: could not create minio \
             bucket {bucket}"
        );
        return;
    }

    let prefix = unique("b2rt");
    let uri_str = format!("b2://{bucket}/{prefix}");
    let uri = StoreUri::parse(&uri_str).expect("valid b2 uri");

    let src_dir = tempfile::tempdir().expect("src tempdir");
    let src = src_dir.path().join("tree");
    build_tree(&src);
    let local_id = api::id(&src, &ManifestOptions::default()).expect("local id");

    let (pushed, leaked) = push_isolated(&src, &uri, 180);
    assert!(
        !leaked,
        "BUG: push to {uri_str} created a literal `b2:` directory on local disk \
         (FileStore fallback) instead of using B2Store"
    );
    let pushed = pushed.expect("push to b2 must succeed against minio");
    assert_eq!(pushed, local_id, "pushed id must equal the local id");

    let verify =
        B2Store::connect(&uri_str, Some(&env.endpoint), None).expect("connect verify B2Store");
    assert_objects_in_store(&verify, &local_id);

    block(api::fetch(&local_id, &uri, &TransferOptions::default())).expect("fetch from b2");
    let co_dir = tempfile::tempdir().expect("checkout tempdir");
    let co_dest = co_dir.path().join("out");
    block(api::checkout(
        &local_id,
        &co_dest,
        &CheckoutOptions::default(),
    ))
    .expect("checkout");
    let co_id = api::id(&co_dest, &ManifestOptions::default()).expect("re-id checkout dest");
    assert_eq!(
        co_id, local_id,
        "checkout of the B2-fetched snapshot must re-id to the same id"
    );
}

// ---------------------------------------------------------------------------
// 3b. SSH / SFTP: dispatch must not write to local disk
// ---------------------------------------------------------------------------

/// CONTRACT: `ssh://` and `sftp://` URIs are NOT handled by a local `FileStore`
/// — a push attempt must never create a literal `ssh:`/`sftp:` directory on
/// local disk (it must reach the SSH/SFTP transport, succeeding or failing as a
/// network op). The full sshd-sidecar round-trip (key + known_hosts wiring) is
/// DEFERRED to the parity harness (`run_parity.sh` sftp leg); this pins the
/// no-local-fallback half of the contract, which is exactly the bug symptom.
#[test]
fn ssh_sftp_push_never_writes_to_local_disk() {
    let src_dir = tempfile::tempdir().expect("src tempdir");
    let src = src_dir.path().join("tree");
    build_tree(&src);

    for scheme in ["ssh", "sftp"] {
        // A loopback target with no listener: a real transport fails fast; a
        // FileStore fallback would instead "succeed" by writing `./{scheme}:`.
        let uri_str = format!("{scheme}://127.0.0.1/tmp/{}", unique("rt"));
        let uri = StoreUri::parse(&uri_str).expect("valid ssh/sftp uri");
        let (_pushed, leaked) = push_isolated(&src, &uri, 20);
        assert!(
            !leaked,
            "BUG: push to {uri_str} created a literal `{scheme}:` directory on local disk \
             (FileStore fallback) instead of using the SSH/SFTP transport"
        );
    }

    if std::env::var_os("SNAPDIR_SFTP_STORE_IDENTITY_FILE").is_none() {
        eprintln!(
            "NOTE ssh_sftp_push_never_writes_to_local_disk: full sftp round-trip deferred \
             (no SNAPDIR_SFTP_STORE_IDENTITY_FILE / sshd sidecar); local-disk-leak half asserted"
        );
    }
}

// ---------------------------------------------------------------------------
// 3c. GCS (no local emulator): dispatch must not write to local disk
// ---------------------------------------------------------------------------

/// CONTRACT: a `gs://` URI dispatches to `GcsStore`, never to a local
/// `FileStore`. With no GCS emulator we assert the negative that uniquely
/// distinguishes the two: a push attempt must NOT create a literal `gs:`
/// directory on local disk. (Buggy file-only impl => `./gs:` is written and the
/// push "succeeds"; the real `GcsStore` path makes a credential/network attempt
/// and leaves no local tree.) Runs without any sidecar.
#[test]
fn gs_push_never_writes_to_local_disk() {
    let src_dir = tempfile::tempdir().expect("src tempdir");
    let src = src_dir.path().join("tree");
    build_tree(&src);

    let uri_str = format!("gs://snapdir-api-multistore-noexist/{}", unique("gs"));
    let uri = StoreUri::parse(&uri_str).expect("valid gs uri");
    let (_pushed, leaked) = push_isolated(&src, &uri, 25);
    assert!(
        !leaked,
        "BUG: push to {uri_str} created a literal `gs:` directory on local disk \
         (FileStore fallback) instead of constructing a GcsStore"
    );
}

// ---------------------------------------------------------------------------
// 4. sync: store-to-store must not stage via local disk for non-file schemes
// ---------------------------------------------------------------------------

/// CONTRACT: `sync(id, s3-src, s3-dst)` copies one bucket/prefix to another
/// through the real stores; the destination ends up holding the snapshot and no
/// local `s3:` directory is created. Skips (visibly) without the minio sidecar.
#[test]
fn sync_s3_to_s3_uses_real_stores() {
    let Some(env) = minio_env() else {
        eprintln!("SKIP sync_s3_to_s3_uses_real_stores: minio sidecar env absent");
        return;
    };
    ensure_isolated_cache();

    let bucket = "snapdir-api-multistore";
    if !create_bucket(&env.endpoint, bucket, &env.access, &env.secret) {
        eprintln!("SKIP sync_s3_to_s3_uses_real_stores: could not create minio bucket {bucket}");
        return;
    }

    let src_prefix = unique("syncsrc");
    let dst_prefix = unique("syncdst");
    let src_uri_str = format!("s3://{bucket}/{src_prefix}");
    let dst_uri_str = format!("s3://{bucket}/{dst_prefix}");
    let src_uri = StoreUri::parse(&src_uri_str).expect("valid src uri");
    let dst_uri = StoreUri::parse(&dst_uri_str).expect("valid dst uri");

    let tree_dir = tempfile::tempdir().expect("src tempdir");
    let tree = tree_dir.path().join("tree");
    build_tree(&tree);
    let local_id = api::id(&tree, &ManifestOptions::default()).expect("local id");

    // Seed the source bucket via push (already covered for leaks above).
    let (pushed, _leaked) = push_isolated(&tree, &src_uri, 180);
    assert_eq!(pushed.expect("seed push"), local_id);

    // sync src bucket -> dst bucket. A FileStore fallback would create `./s3:`.
    let pre_existed = Path::new("s3:").exists();
    block(api::sync(
        &local_id,
        &src_uri,
        &dst_uri,
        &TransferOptions::default(),
    ))
    .expect("sync s3 -> s3");
    let leaked = !pre_existed && Path::new("s3:").exists();
    if Path::new("s3:").exists() && !pre_existed {
        let _ = std::fs::remove_dir_all("s3:");
    }
    assert!(
        !leaked,
        "BUG: sync wrote a literal `s3:` directory on local disk instead of using S3Store"
    );

    // The destination bucket/prefix must now hold the snapshot + its objects.
    let dst = S3Store::connect(&dst_uri_str, Some(&env.endpoint)).expect("connect dst S3Store");
    assert_objects_in_store(&dst, &local_id);
}
