//! `snapdir-node` — napi-rs binding for `@snapdir/snapdir`.
//!
//! Wraps the frozen `snapdir-api` §6 surface. Every exported symbol maps
//! 1:1 to a `snapdir_api` function or type — no reimplementation of logic.
//!
//! # BigInt / u64
//!
//! napi-rs maps Rust `u64` → JS `BigInt`. `ManifestEntry.size` is `u64` in
//! `snapdir-api` and therefore surfaces as `bigint` in JS — the headline
//! Node idiom requirement.
//!
//! # SnapdirError
//!
//! napi-rs errors are `instanceof Error`. We embed the error code in the
//! napi Error message as `[CODE] message` so the JS wrapper (`index.js`)
//! can parse and re-throw as `SnapdirError { code, message }`.
//!
//! # Async / Sync split
//!
//! - `manifest`, `id`, `stage`: ASYNC in the Node binding. They use the
//!   `Task` pattern (napi_queue_async_work → libuv thread pool) so that
//!   the completion fires via the libuv event loop (check phase), which
//!   guarantees that setTimeout(0) timers fire while the work is in flight.
//! - `idFromManifest`, `version`: SYNC (pure compute / infallible).
//! - `push`, `fetch`, `pull`, `checkout`, `sync`, `diff`, `verify`: ASYNC
//!   (drive the `snapdir-api` async fns on the tokio runtime).

#![deny(clippy::all)]

use napi::bindgen_prelude::*;
use napi_derive::napi;

// ---------------------------------------------------------------------------
// Helpers: encode SnapdirError into a napi Error with "[CODE] message" prefix
// ---------------------------------------------------------------------------

fn snapdir_err_to_napi(e: snapdir_api::SnapdirError) -> napi::Error {
    let code = e.code().to_owned();
    let message = format!("[{}] {}", code, e);
    napi::Error::new(napi::Status::GenericFailure, message)
}

fn api_result<T>(r: snapdir_api::Result<T>) -> napi::Result<T> {
    r.map_err(snapdir_err_to_napi)
}

// ---------------------------------------------------------------------------
// PathType
// ---------------------------------------------------------------------------

/// Whether a manifest entry is a file or a directory.
#[napi(string_enum)]
pub enum PathType {
    File,
    Directory,
}

impl From<snapdir_api::PathType> for PathType {
    fn from(v: snapdir_api::PathType) -> Self {
        match v {
            snapdir_api::PathType::File => PathType::File,
            snapdir_api::PathType::Directory => PathType::Directory,
        }
    }
}

// ---------------------------------------------------------------------------
// ManifestEntry
// ---------------------------------------------------------------------------

/// A single entry in a directory snapshot manifest.
///
/// `size` is `bigint` (`u64`) — file sizes can exceed `Number.MAX_SAFE_INTEGER`.
/// `checksum` is the 64-lowercase-hex BLAKE3 checksum string.
#[napi(object)]
pub struct ManifestEntry {
    /// Path relative to the snapshot root.
    pub path: String,
    /// Whether this entry is a `'File'` or `'Directory'`.
    #[napi(js_name = "pathType")]
    pub path_type: PathType,
    /// Octal permission bits (e.g. `0o700` → `448`).
    pub permissions: u32,
    /// 64-char lowercase hex BLAKE3 content checksum.
    pub checksum: String,
    /// Content size in bytes — `bigint` because `u64` exceeds
    /// `Number.MAX_SAFE_INTEGER`.
    pub size: BigInt,
}

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

/// An ordered collection of manifest entries plus the raw manifest text.
#[napi(object)]
pub struct Manifest {
    /// Ordered list of entries (files and directories).
    pub entries: Vec<ManifestEntry>,
    /// Raw manifest text (the Display output of the core Manifest).
    pub raw: String,
}

fn api_manifest_to_napi(m: snapdir_api::Manifest) -> Manifest {
    let entries = m
        .entries
        .into_iter()
        .map(|e| {
            // [u8;32] → 64-hex-lowercase string
            let checksum = e
                .checksum
                .iter()
                .fold(String::with_capacity(64), |mut s, b| {
                    use std::fmt::Write;
                    write!(s, "{b:02x}").unwrap();
                    s
                });
            ManifestEntry {
                path: e.path.to_string_lossy().into_owned(),
                path_type: PathType::from(e.path_type),
                permissions: e.permissions,
                checksum,
                // u64 → BigInt (napi-rs maps u64 to JS bigint)
                size: BigInt::from(e.size),
            }
        })
        .collect();
    Manifest { entries, raw: m.raw }
}

// ---------------------------------------------------------------------------
// DiffEntry
// ---------------------------------------------------------------------------

/// A single entry in a diff result.
///
/// `status` is a single-character string: `"A"` (Added), `"D"` (Deleted),
/// `"M"` (Modified), `"="` (Unchanged). The TS wrapper types them as
/// `DiffStatus = 'A'|'D'|'M'|'='`.
#[napi(object)]
pub struct DiffEntry {
    /// Change status: `'A'`, `'D'`, `'M'`, or `'='`.
    pub status: String,
    /// The entry path.
    pub path: String,
}

fn diff_status_to_str(v: snapdir_api::DiffStatus) -> String {
    match v {
        snapdir_api::DiffStatus::Added => "A".to_owned(),
        snapdir_api::DiffStatus::Deleted => "D".to_owned(),
        snapdir_api::DiffStatus::Modified => "M".to_owned(),
        snapdir_api::DiffStatus::Unchanged => "=".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// VerifyResult
// ---------------------------------------------------------------------------

/// Result of a `verify()` call.
#[napi(object)]
pub struct VerifyResult {
    /// `true` when every object in the snapshot verified clean.
    pub ok: bool,
}

// ---------------------------------------------------------------------------
// DiffParams
// ---------------------------------------------------------------------------

/// Options for `diff()`. Pass arrays of store URIs.
#[napi(object)]
pub struct DiffParams {
    /// Source side store URIs.
    pub from: Vec<String>,
    /// Destination side store URIs.
    pub to: Vec<String>,
}

// ---------------------------------------------------------------------------
// version() — SYNC, infallible
// ---------------------------------------------------------------------------

/// Returns the `snapdir-api` crate version string (e.g. `"1.10.0"`).
#[napi]
pub fn version() -> String {
    snapdir_api::version().to_owned()
}

// ---------------------------------------------------------------------------
// idFromManifest() — SYNC, pure/infallible
// ---------------------------------------------------------------------------

/// Derives the snapshot ID from an already-computed `Manifest`.
///
/// SYNC — pure, no I/O, infallible. Result is identical to `id(path)`.
/// Returns a 64-lowercase-hex `SnapshotId` string.
#[napi(js_name = "idFromManifest")]
pub fn id_from_manifest(m: Manifest) -> String {
    // Reconstruct a snapdir_api::Manifest from the raw text.
    // The raw text is the authoritative format; re-parsing it is the same
    // path as id_from_manifest in snapdir-api itself.
    let api_manifest = snapdir_api::Manifest {
        raw: m.raw,
        entries: vec![], // unused by id_from_manifest (it re-parses `raw`)
    };
    snapdir_api::id_from_manifest(&api_manifest).to_hex()
}

// ---------------------------------------------------------------------------
// ManifestOptions — optional 2nd argument for manifest() and id()
// ---------------------------------------------------------------------------

/// Options controlling how a directory manifest is walked.
///
/// All fields are optional. Omitting them (or passing `{}`) reproduces the
/// current default behavior, preserving backward compatibility.
///
/// napi-rs camelCases snake_case field names automatically, so the JS/TS
/// surface is `noFollow`, `absolute`, `exclude`.
#[napi(object)]
pub struct ManifestOptions {
    /// When `true`, symbolic links are recorded as links rather than being
    /// dereferenced. Mirrors `--no-follow`. Default: `false` (follow).
    pub no_follow: Option<bool>,
    /// When `true`, paths are rendered as absolute paths instead of the
    /// default `./`-relative form. Mirrors `--absolute`. Default: `false`.
    pub absolute: Option<bool>,
    /// Extended-regex patterns (OR-combined) whose matching entries are
    /// excluded from the manifest. Mirrors `--exclude`. Default: none.
    pub exclude: Option<Vec<String>>,
}

/// Folds an `Option<ManifestOptions>` into a `snapdir_api::ManifestOptions`.
///
/// `None` and `Some({})` both reproduce `snapdir_api::ManifestOptions::default()`
/// exactly, ensuring backward-compatibility for callers that omit the argument.
fn resolve_manifest_options(opts: Option<ManifestOptions>) -> snapdir_api::ManifestOptions {
    let mut api_opts = snapdir_api::ManifestOptions::default();
    if let Some(o) = opts {
        api_opts.no_follow = o.no_follow.unwrap_or(false);
        api_opts.absolute = o.absolute.unwrap_or(false);
        api_opts.exclude = o.exclude.unwrap_or_default();
    }
    api_opts
}

// ---------------------------------------------------------------------------
// AsyncTask implementations for manifest / id / stage
//
// Using napi's Task trait (backed by napi_queue_async_work / libuv thread pool)
// instead of `#[napi] async fn` (which uses the tokio runtime). The libuv
// thread pool completion fires in the poll/check phase of the libuv event loop,
// AFTER the timers phase. This guarantees that a setTimeout(0) timer set right
// before the async work will fire while the work is in flight — the key
// event-loop-safety contract.
// ---------------------------------------------------------------------------

// ManifestTask ---------------------------------------------------------------

pub struct ManifestTask {
    path: String,
    options: snapdir_api::ManifestOptions,
}

impl Task for ManifestTask {
    type Output = Manifest;
    type JsValue = Manifest;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        api_result(snapdir_api::manifest(
            std::path::Path::new(&self.path),
            &self.options,
        ))
        .map(api_manifest_to_napi)
    }

    fn resolve(&mut self, _env: napi::Env, output: Self::Output) -> napi::Result<Self::JsValue> {
        Ok(output)
    }
}

/// Walks `path` and returns a typed `Manifest`.
///
/// ASYNC — the BLAKE3 walk runs on the libuv thread pool so the event loop
/// is never blocked. setTimeout(0) timers fire while the walk is in flight.
///
/// The optional `options` argument controls symlink handling (`noFollow`),
/// path rendering (`absolute`), and entry filtering (`exclude`). Omitting
/// it (or passing `{}`) reproduces the current default behavior.
#[napi]
pub fn manifest(path: String, options: Option<ManifestOptions>) -> AsyncTask<ManifestTask> {
    AsyncTask::new(ManifestTask {
        path,
        options: resolve_manifest_options(options),
    })
}

// IdTask ---------------------------------------------------------------------

pub struct IdTask {
    path: String,
    options: snapdir_api::ManifestOptions,
}

impl Task for IdTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        api_result(snapdir_api::id(
            std::path::Path::new(&self.path),
            &self.options,
        ))
        .map(|sid| sid.to_hex())
    }

    fn resolve(&mut self, _env: napi::Env, output: Self::Output) -> napi::Result<Self::JsValue> {
        Ok(output)
    }
}

/// Computes the snapshot ID for the directory at `path`.
///
/// ASYNC — walk runs on the libuv thread pool.
/// Returns a 64-lowercase-hex `SnapshotId` string.
///
/// The optional `options` argument controls symlink handling (`noFollow`),
/// path rendering (`absolute`), and entry filtering (`exclude`). Omitting
/// it (or passing `{}`) reproduces the current default behavior.
#[napi]
pub fn id(path: String, options: Option<ManifestOptions>) -> AsyncTask<IdTask> {
    AsyncTask::new(IdTask {
        path,
        options: resolve_manifest_options(options),
    })
}

// StageTask ------------------------------------------------------------------

pub struct StageTask {
    path: String,
}

impl Task for StageTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> napi::Result<Self::Output> {
        api_result(snapdir_api::stage(
            std::path::Path::new(&self.path),
            &snapdir_api::StageOptions::default(),
        ))
        .map(|sid| sid.to_hex())
    }

    fn resolve(&mut self, _env: napi::Env, output: Self::Output) -> napi::Result<Self::JsValue> {
        Ok(output)
    }
}

/// Stages `path` in the local cache and returns the snapshot ID.
///
/// ASYNC — walk + cache write run on the libuv thread pool.
/// Returns a 64-lowercase-hex `SnapshotId` string.
#[napi]
pub fn stage(path: String) -> AsyncTask<StageTask> {
    AsyncTask::new(StageTask { path })
}

// ---------------------------------------------------------------------------
// push() — ASYNC (tokio async fn)
// ---------------------------------------------------------------------------

/// Pushes a snapshot from `path` to `storeUri` and returns the snapshot ID.
///
/// ASYNC — network / I/O-bound.
/// Returns a 64-lowercase-hex `SnapshotId` string.
#[napi]
pub async fn push(path: String, store_uri: String) -> napi::Result<String> {
    let store = api_result(snapdir_api::StoreUri::parse(&store_uri))?;
    let path_buf = std::path::PathBuf::from(&path);
    let result = snapdir_api::push(
        snapdir_api::PushSource::Path(&path_buf),
        &store,
        &snapdir_api::TransferOptions::default(),
    )
    .await;
    api_result(result).map(|sid| sid.to_hex())
}

// ---------------------------------------------------------------------------
// fetch() — ASYNC
// ---------------------------------------------------------------------------

/// Fetches a snapshot from `storeUri` into the local cache.
///
/// ASYNC — network / I/O-bound.
#[napi(js_name = "fetch")]
pub async fn fetch_snapshot(snapshot_id: String, store_uri: String) -> napi::Result<()> {
    let sid = api_result(snapdir_api::SnapshotId::from_hex(&snapshot_id))?;
    let store = api_result(snapdir_api::StoreUri::parse(&store_uri))?;
    let result = snapdir_api::fetch(&sid, &store, &snapdir_api::TransferOptions::default()).await;
    api_result(result)
}

// ---------------------------------------------------------------------------
// pull() — ASYNC
// ---------------------------------------------------------------------------

/// Pulls a snapshot from `storeUri` into `dest`, materializing its files.
///
/// ASYNC — network / I/O-bound.
#[napi]
pub async fn pull(snapshot_id: String, store_uri: String, dest: String) -> napi::Result<()> {
    let sid = api_result(snapdir_api::SnapshotId::from_hex(&snapshot_id))?;
    let store = api_result(snapdir_api::StoreUri::parse(&store_uri))?;
    let dest_path = std::path::PathBuf::from(dest);
    let result = snapdir_api::pull(
        &sid,
        &store,
        &dest_path,
        &snapdir_api::CheckoutOptions::default(),
    )
    .await;
    api_result(result)
}

// ---------------------------------------------------------------------------
// checkout() — ASYNC
// ---------------------------------------------------------------------------

/// Materializes a snapshot from the local cache to `dest`.
///
/// ASYNC — I/O-bound.
#[napi]
pub async fn checkout(snapshot_id: String, dest: String) -> napi::Result<()> {
    let sid = api_result(snapdir_api::SnapshotId::from_hex(&snapshot_id))?;
    let dest_path = std::path::PathBuf::from(dest);
    let result = snapdir_api::checkout(
        &sid,
        &dest_path,
        &snapdir_api::CheckoutOptions::default(),
    )
    .await;
    api_result(result)
}

// ---------------------------------------------------------------------------
// sync() — ASYNC
// ---------------------------------------------------------------------------

/// Copies a snapshot from `srcUri` to `dstUri`.
///
/// ASYNC — network / I/O-bound.
#[napi(js_name = "sync")]
pub async fn sync_snapshot(
    snapshot_id: String,
    src_uri: String,
    dst_uri: String,
) -> napi::Result<()> {
    let sid = api_result(snapdir_api::SnapshotId::from_hex(&snapshot_id))?;
    let src = api_result(snapdir_api::StoreUri::parse(&src_uri))?;
    let dst = api_result(snapdir_api::StoreUri::parse(&dst_uri))?;
    let result = snapdir_api::sync(
        &sid,
        &src,
        &dst,
        &snapdir_api::TransferOptions::default(),
    )
    .await;
    api_result(result)
}

// ---------------------------------------------------------------------------
// diff() — ASYNC
// ---------------------------------------------------------------------------

/// Diffs two sets of stores, returning structured change entries.
///
/// ASYNC — I/O-bound.
#[napi]
pub async fn diff(params: DiffParams) -> napi::Result<Vec<DiffEntry>> {
    let from_uris: Vec<snapdir_api::StoreUri> = params
        .from
        .iter()
        .map(|s| snapdir_api::StoreUri::parse(s))
        .collect::<snapdir_api::Result<Vec<_>>>()
        .map_err(snapdir_err_to_napi)?;
    let to_uris: Vec<snapdir_api::StoreUri> = params
        .to
        .iter()
        .map(|s| snapdir_api::StoreUri::parse(s))
        .collect::<snapdir_api::Result<Vec<_>>>()
        .map_err(snapdir_err_to_napi)?;

    let opts = snapdir_api::DiffOptions {
        from: from_uris,
        to: to_uris,
        ..snapdir_api::DiffOptions::default()
    };

    let result = snapdir_api::diff(&opts).await;
    api_result(result).map(|entries| {
        entries
            .into_iter()
            .map(|e| DiffEntry {
                status: diff_status_to_str(e.status),
                path: e.path.to_string_lossy().into_owned(),
            })
            .collect()
    })
}

// ---------------------------------------------------------------------------
// verify() — ASYNC
// ---------------------------------------------------------------------------

/// Verifies a snapshot in `storeUri`.
///
/// ASYNC — I/O-bound.
#[napi]
pub async fn verify(snapshot_id: String, store_uri: String) -> napi::Result<VerifyResult> {
    let sid = api_result(snapdir_api::SnapshotId::from_hex(&snapshot_id))?;
    let store = api_result(snapdir_api::StoreUri::parse(&store_uri))?;
    let result = snapdir_api::verify(
        &sid,
        &store,
        &snapdir_api::VerifyOptions::default(),
    )
    .await;
    api_result(result).map(|r| VerifyResult { ok: r.ok })
}
