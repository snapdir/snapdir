//! `snapdir-python` — PyO3/maturin binding for `snapdir`.
//!
//! Wraps the frozen `snapdir-api` §6 surface. Every exported symbol maps
//! 1:1 to a `snapdir_api` function or type — no reimplementation of logic.
//!
//! # u64 / size
//!
//! PyO3 0.23 maps Rust `u64` → Python `int` (arbitrary precision). The
//! `ManifestEntry.size` field uses `u64` directly — never cast to `f64`.
//!
//! # Exception hierarchy
//!
//! With `abi3` enabled, PyO3 cannot subclass native Python types via
//! `#[pyclass(extends=PyException)]`. Instead we use `create_exception!` to
//! define the hierarchy in Python-space, and attach `.code` to each raised
//! instance via `setattr` at raise time.
//!
//!   SnapdirError(Exception)  ← base; instances carry .code
//!     HashMismatchError(SnapdirError)
//!     StoreError(SnapdirError)
//!     InFluxError(SnapdirError)
//!     CatalogError(SnapdirError)
//!
//! # Async / Sync split
//!
//! - `manifest`, `id`, `stage`: SYNC in `snapdir-api` but exposed as real
//!   asyncio coroutines (`pyo3_async_runtimes::tokio::future_into_py` +
//!   `tokio::task::spawn_blocking`) so callers can `await`/`gather` them.
//! - `push`, `fetch`, `pull`, `checkout`, `sync`, `diff`, `verify`: native
//!   async in `snapdir-api`; awaited directly inside `future_into_py`.
//! - `id_from_manifest`, `version`: SYNC `#[pyfunction]` — callable without
//!   a running event loop.

#![deny(clippy::all)]

use pyo3::prelude::*;
use pyo3::types::{PyBool, PyList, PyType};

// ---------------------------------------------------------------------------
// Exception hierarchy (create_exception! + runtime .code attribute)
//
// create_exception!(module, ExceptionName, BaseException) registers the class
// in the given module namespace. The base for concrete subtypes is `SnapdirError`
// (itself a subclass of `Exception`). With abi3, this is the only viable way
// to build an exception hierarchy.
//
// `.code` is attached to each raised instance via `instance.setattr("code", ...)`.
// ---------------------------------------------------------------------------

pyo3::create_exception!(snapdir, SnapdirError, pyo3::exceptions::PyException);
pyo3::create_exception!(snapdir, HashMismatchError, SnapdirError);
pyo3::create_exception!(snapdir, StoreError, SnapdirError);
pyo3::create_exception!(snapdir, InFluxError, SnapdirError);
pyo3::create_exception!(snapdir, CatalogError, SnapdirError);

/// Raises the correct Python exception subtype for a `snapdir_api::SnapdirError`,
/// attaching `.code` to the instance.
fn raise_snapdir_err(py: Python<'_>, e: snapdir_api::SnapdirError) -> PyErr {
    let code = e.code().to_owned();
    let msg = e.to_string();

    let py_err = match code.as_str() {
        "HASH_MISMATCH" => PyErr::new::<HashMismatchError, _>(msg.clone()),
        "STORE_ERROR" => PyErr::new::<StoreError, _>(msg.clone()),
        "IN_FLUX" => PyErr::new::<InFluxError, _>(msg.clone()),
        "CATALOG_ERROR" => PyErr::new::<CatalogError, _>(msg.clone()),
        // IO_ERROR, INVALID_ID, INVALID_STORE, CONFLICT → base type
        _ => PyErr::new::<SnapdirError, _>(msg.clone()),
    };

    // Attach .code to the exception instance
    if let Ok(instance) = py_err
        .value(py)
        .downcast::<pyo3::exceptions::PyBaseException>()
    {
        let _ = instance.setattr("code", code.as_str());
    }
    py_err
}

fn api_result<T>(py: Python<'_>, r: snapdir_api::Result<T>) -> PyResult<T> {
    r.map_err(|e| raise_snapdir_err(py, e))
}

// ---------------------------------------------------------------------------
// PathType — exposed as an object with `.name` ("File" / "Directory")
// ---------------------------------------------------------------------------

/// Whether a manifest entry is a regular file or a directory.
///
/// Instances carry a `.name` attribute: `"File"` or `"Directory"`.
#[pyclass(frozen)]
#[derive(Clone)]
pub struct PathType {
    #[pyo3(get)]
    pub name: String,
}

#[pymethods]
impl PathType {
    fn __str__(&self) -> &str {
        &self.name
    }
    fn __repr__(&self) -> String {
        format!("PathType.{}", self.name)
    }
}

impl From<snapdir_api::PathType> for PathType {
    fn from(v: snapdir_api::PathType) -> Self {
        PathType {
            name: match v {
                snapdir_api::PathType::File => "File".to_owned(),
                snapdir_api::PathType::Directory => "Directory".to_owned(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// ManifestEntry
// ---------------------------------------------------------------------------

/// A single entry in a directory snapshot manifest.
///
/// `size` is a Python `int` (arbitrary precision, maps from Rust `u64`).
/// `checksum` is a 64-char lowercase hex string.
/// `path_type` is a `PathType` instance with `.name` == `"File"` or `"Directory"`.
#[pyclass(frozen, get_all)]
#[derive(Clone)]
pub struct ManifestEntry {
    /// Path relative to the snapshot root.
    pub path: String,
    /// `PathType` instance: `.name` is `"File"` or `"Directory"`.
    pub path_type: PathType,
    /// Octal permission bits.
    pub permissions: u32,
    /// 64-char lowercase hex BLAKE3 checksum.
    pub checksum: String,
    /// Content size in bytes (arbitrary-precision Python `int`).
    pub size: u64,
}

#[pymethods]
impl ManifestEntry {
    fn __repr__(&self) -> String {
        format!(
            "ManifestEntry(path={:?}, path_type={}, size={})",
            self.path, self.path_type.name, self.size
        )
    }
}

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

/// An ordered collection of manifest entries plus the raw manifest text.
#[pyclass(frozen, get_all)]
pub struct Manifest {
    /// Ordered list of `ManifestEntry` instances.
    pub entries: Vec<ManifestEntry>,
    /// Raw manifest text (the Display output of the core Manifest).
    pub raw: String,
}

#[pymethods]
impl Manifest {
    fn __repr__(&self) -> String {
        format!(
            "Manifest(entries={}, raw_len={})",
            self.entries.len(),
            self.raw.len()
        )
    }
}

fn api_manifest_to_py(m: snapdir_api::Manifest) -> Manifest {
    let entries = m
        .entries
        .into_iter()
        .map(|e| {
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
                size: e.size,
            }
        })
        .collect();
    Manifest {
        entries,
        raw: m.raw,
    }
}

// ---------------------------------------------------------------------------
// DiffEntry
// ---------------------------------------------------------------------------

/// A single entry in a diff result.
///
/// `status` is a `str`: `"A"` (Added), `"D"` (Deleted), `"M"` (Modified), `"="` (Unchanged).
#[pyclass(frozen, get_all)]
pub struct DiffEntry {
    /// Change status: `"A"`, `"D"`, `"M"`, or `"="`.
    pub status: String,
    /// The entry path.
    pub path: String,
}

#[pymethods]
impl DiffEntry {
    fn __repr__(&self) -> String {
        format!("DiffEntry(status={:?}, path={:?})", self.status, self.path)
    }
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
// SnapshotId — validated 64-hex wrapper, raises SnapdirError on bad input.
// ---------------------------------------------------------------------------

/// A validated 32-byte snapshot identifier (64-char lowercase hex).
///
/// Raises `SnapdirError` with code `"INVALID_ID"` on malformed input.
#[pyclass(frozen)]
pub struct SnapshotId {
    hex: String,
}

#[pymethods]
impl SnapshotId {
    /// Construct from a 64-char lowercase hex string. Raises `SnapdirError`
    /// (code `INVALID_ID`) on wrong length or non-hex characters.
    #[new]
    fn new(s: &str) -> PyResult<Self> {
        Python::with_gil(|py| {
            let sid = api_result(py, snapdir_api::SnapshotId::from_hex(s))?;
            Ok(SnapshotId { hex: sid.to_hex() })
        })
    }

    fn __str__(&self) -> &str {
        &self.hex
    }

    fn __repr__(&self) -> String {
        format!("SnapshotId({:?})", self.hex)
    }
}

// ---------------------------------------------------------------------------
// StoreUri — thin wrapper around a validated store URI string.
// ---------------------------------------------------------------------------

/// A validated store URI (e.g. `"file:///tmp/store"`, `"s3://bucket/path"`).
///
/// Raises `SnapdirError` (code `"INVALID_STORE"`) for unknown or malformed schemes.
#[pyclass(frozen)]
pub struct StoreUri {
    raw: String,
}

#[pymethods]
impl StoreUri {
    /// Construct from a URI string. Validates the scheme.
    #[new]
    fn new(s: &str) -> PyResult<Self> {
        Python::with_gil(|py| {
            let uri = api_result(py, snapdir_api::StoreUri::parse(s))?;
            Ok(StoreUri {
                raw: uri.to_string(),
            })
        })
    }

    fn __str__(&self) -> &str {
        &self.raw
    }

    fn __repr__(&self) -> String {
        format!("StoreUri({:?})", self.raw)
    }
}

// ---------------------------------------------------------------------------
// DiffOptions — carries from/to lists of StoreUri raw strings.
// ---------------------------------------------------------------------------

/// Options for `diff()`. Built via `DiffOptions.from_refs(from_uris, to_uris)`.
#[pyclass(frozen)]
pub struct DiffOptions {
    from_uris: Vec<String>,
    to_uris: Vec<String>,
}

#[pymethods]
impl DiffOptions {
    /// Build `DiffOptions` from lists of `StoreUri` objects.
    ///
    /// Named `from_refs` because `from` is a reserved keyword in Python.
    #[classmethod]
    fn from_refs(
        _cls: &Bound<'_, PyType>,
        from_uris: &Bound<'_, PyAny>,
        to_uris: &Bound<'_, PyAny>,
    ) -> PyResult<Self> {
        let from_list = extract_store_uri_list(from_uris)?;
        let to_list = extract_store_uri_list(to_uris)?;
        Ok(DiffOptions {
            from_uris: from_list,
            to_uris: to_list,
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "DiffOptions(from={:?}, to={:?})",
            self.from_uris, self.to_uris
        )
    }
}

/// Extract a list of `StoreUri` instances (or plain str) from a Python iterable.
fn extract_store_uri_list(obj: &Bound<'_, PyAny>) -> PyResult<Vec<String>> {
    let items: Vec<Bound<'_, PyAny>> = obj.extract()?;
    items
        .iter()
        .map(|item| {
            if let Ok(uri) = item.extract::<PyRef<StoreUri>>() {
                Ok(uri.raw.clone())
            } else if let Ok(s) = item.extract::<String>() {
                snapdir_api::StoreUri::parse(&s)
                    .map(|u| u.to_string())
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))
            } else {
                Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    "expected StoreUri or str",
                ))
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Path helper — accept str or os.PathLike via PyO3's PathBuf extraction
// ---------------------------------------------------------------------------

fn path_arg(obj: &Bound<'_, PyAny>) -> PyResult<std::path::PathBuf> {
    // PyO3 0.23 PathBuf extraction handles str, bytes, and os.PathLike objects.
    obj.extract::<std::path::PathBuf>()
}

// ---------------------------------------------------------------------------
// Helper: convert a Rust value into an unbound PyObject for use inside
// `future_into_py` closures (where the lifetime is 'static).
// ---------------------------------------------------------------------------

/// Convert a PyO3 `Bound<'py, T>` to a `PyObject` (= `Py<PyAny>`).
fn into_py_obj<T: pyo3::PyClass>(bound: Bound<'_, T>) -> PyObject {
    bound.into_any().unbind()
}

// ---------------------------------------------------------------------------
// version() — SYNC, infallible
// ---------------------------------------------------------------------------

/// Returns the `snapdir-api` crate version string (e.g. `"1.10.0"`).
#[pyfunction]
fn version() -> String {
    snapdir_api::version().to_owned()
}

// ---------------------------------------------------------------------------
// id_from_manifest() — SYNC, pure/infallible
// ---------------------------------------------------------------------------

/// Derives the snapshot ID from an already-computed `Manifest`.
///
/// SYNC — pure compute, no I/O, callable without an event loop.
/// Returns a 64-lowercase-hex `str`.
#[pyfunction]
fn id_from_manifest(m: PyRef<'_, Manifest>) -> String {
    let api_manifest = snapdir_api::Manifest {
        raw: m.raw.clone(),
        entries: vec![], // unused — id_from_manifest re-parses `raw`
    };
    snapdir_api::id_from_manifest(&api_manifest).to_hex()
}

// ---------------------------------------------------------------------------
// build_manifest_options() — shared helper for manifest/id keyword options
// ---------------------------------------------------------------------------

fn build_manifest_options(
    no_follow: bool,
    absolute: bool,
    exclude: Option<Vec<String>>,
) -> snapdir_api::ManifestOptions {
    let mut o = snapdir_api::ManifestOptions::default();
    o.no_follow = no_follow;
    o.absolute = absolute;
    o.exclude = exclude.unwrap_or_default();
    o
}

// ---------------------------------------------------------------------------
// manifest() — ASYNC coroutine (wraps sync snapdir_api::manifest)
// ---------------------------------------------------------------------------

/// Walks `path` and returns a typed `Manifest`.
///
/// ASYNC — the BLAKE3 walk runs on a blocking thread pool. Accepts `str` or
/// `pathlib.Path`.
///
/// Keyword-only options (all optional, backward-compatible):
///   - `no_follow`: do not follow symlinks (default `False`)
///   - `absolute`: render absolute paths instead of `./`-relative (default `False`)
///   - `exclude`: list of extended-regex patterns to exclude (default `None`)
#[pyfunction]
#[pyo3(signature = (path, *, no_follow=false, absolute=false, exclude=None))]
fn manifest<'py>(
    py: Python<'py>,
    path: &Bound<'_, PyAny>,
    no_follow: bool,
    absolute: bool,
    exclude: Option<Vec<String>>,
) -> PyResult<Bound<'py, PyAny>> {
    let path_buf = path_arg(path)?;
    let opts = build_manifest_options(no_follow, absolute, exclude);
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let api_result_val =
            tokio::task::spawn_blocking(move || snapdir_api::manifest(&path_buf, &opts))
                .await
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;

        Python::with_gil(|py| {
            let m = api_result(py, api_result_val)?;
            Ok(into_py_obj(Bound::new(py, api_manifest_to_py(m))?))
        })
    })
}

// ---------------------------------------------------------------------------
// id() — ASYNC coroutine
// ---------------------------------------------------------------------------

/// Computes the snapshot ID for `path`.
///
/// ASYNC — walk runs on a blocking thread pool.
/// Returns a 64-lowercase-hex `str`.
///
/// Keyword-only options (all optional, backward-compatible):
///   - `no_follow`: do not follow symlinks (default `False`)
///   - `absolute`: render absolute paths instead of `./`-relative (default `False`)
///   - `exclude`: list of extended-regex patterns to exclude (default `None`)
#[pyfunction]
#[pyo3(signature = (path, *, no_follow=false, absolute=false, exclude=None))]
fn id<'py>(
    py: Python<'py>,
    path: &Bound<'_, PyAny>,
    no_follow: bool,
    absolute: bool,
    exclude: Option<Vec<String>>,
) -> PyResult<Bound<'py, PyAny>> {
    let path_buf = path_arg(path)?;
    let opts = build_manifest_options(no_follow, absolute, exclude);
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let api_result_val = tokio::task::spawn_blocking(move || snapdir_api::id(&path_buf, &opts))
            .await
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;

        Python::with_gil(|py| {
            let sid = api_result(py, api_result_val)?;
            Ok(sid.to_hex().into_pyobject(py)?.into_any().unbind())
        })
    })
}

// ---------------------------------------------------------------------------
// stage() — ASYNC coroutine
// ---------------------------------------------------------------------------

/// Stages `path` in the local cache and returns the snapshot ID.
///
/// ASYNC — walk + cache write run on a blocking thread pool.
/// Returns a 64-lowercase-hex `str`.
#[pyfunction]
fn stage<'py>(py: Python<'py>, path: &Bound<'_, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let path_buf = path_arg(path)?;
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let api_result_val = tokio::task::spawn_blocking(move || {
            snapdir_api::stage(&path_buf, &snapdir_api::StageOptions::default())
        })
        .await
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;

        Python::with_gil(|py| {
            let sid = api_result(py, api_result_val)?;
            Ok(sid.to_hex().into_pyobject(py)?.into_any().unbind())
        })
    })
}

// ---------------------------------------------------------------------------
// push() — ASYNC
// ---------------------------------------------------------------------------

/// Pushes a snapshot from `path` to `store` and returns the snapshot ID.
///
/// ASYNC — network / I/O-bound.
/// `store` is a `StoreUri` instance. Returns a 64-lowercase-hex `str`.
#[pyfunction]
fn push<'py>(
    py: Python<'py>,
    path: &Bound<'_, PyAny>,
    store: PyRef<'_, StoreUri>,
) -> PyResult<Bound<'py, PyAny>> {
    let path_buf = path_arg(path)?;
    let store_raw = store.raw.clone();
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let store = snapdir_api::StoreUri::parse(&store_raw)
            .map_err(|e| Python::with_gil(|py| raise_snapdir_err(py, e)))?;
        let result = snapdir_api::push(
            snapdir_api::PushSource::Path(&path_buf),
            &store,
            &snapdir_api::TransferOptions::default(),
        )
        .await;
        Python::with_gil(|py| {
            let sid = api_result(py, result)?;
            Ok(sid.to_hex().into_pyobject(py)?.into_any().unbind())
        })
    })
}

// ---------------------------------------------------------------------------
// fetch() — ASYNC
// ---------------------------------------------------------------------------

/// Fetches a snapshot from `store` into the local cache.
///
/// ASYNC — network / I/O-bound.
/// `snapshot_id` is a `SnapshotId`; `store` is a `StoreUri`.
#[pyfunction]
fn fetch<'py>(
    py: Python<'py>,
    snapshot_id: PyRef<'_, SnapshotId>,
    store: PyRef<'_, StoreUri>,
) -> PyResult<Bound<'py, PyAny>> {
    let sid_hex = snapshot_id.hex.clone();
    let store_raw = store.raw.clone();
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let sid = snapdir_api::SnapshotId::from_hex(&sid_hex)
            .map_err(|e| Python::with_gil(|py| raise_snapdir_err(py, e)))?;
        let store = snapdir_api::StoreUri::parse(&store_raw)
            .map_err(|e| Python::with_gil(|py| raise_snapdir_err(py, e)))?;
        let result =
            snapdir_api::fetch(&sid, &store, &snapdir_api::TransferOptions::default()).await;
        Python::with_gil(|py| {
            api_result(py, result)?;
            Ok(py.None())
        })
    })
}

// ---------------------------------------------------------------------------
// pull() — ASYNC
// ---------------------------------------------------------------------------

/// Pulls a snapshot from `store` into `dest`, materializing its files.
///
/// ASYNC — network / I/O-bound.
#[pyfunction]
fn pull<'py>(
    py: Python<'py>,
    snapshot_id: PyRef<'_, SnapshotId>,
    store: PyRef<'_, StoreUri>,
    dest: &Bound<'_, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    let sid_hex = snapshot_id.hex.clone();
    let store_raw = store.raw.clone();
    let dest_buf = path_arg(dest)?;
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let sid = snapdir_api::SnapshotId::from_hex(&sid_hex)
            .map_err(|e| Python::with_gil(|py| raise_snapdir_err(py, e)))?;
        let store = snapdir_api::StoreUri::parse(&store_raw)
            .map_err(|e| Python::with_gil(|py| raise_snapdir_err(py, e)))?;
        let result = snapdir_api::pull(
            &sid,
            &store,
            &dest_buf,
            &snapdir_api::CheckoutOptions::default(),
        )
        .await;
        Python::with_gil(|py| {
            api_result(py, result)?;
            Ok(py.None())
        })
    })
}

// ---------------------------------------------------------------------------
// checkout() — ASYNC
// ---------------------------------------------------------------------------

/// Materializes a snapshot from the local cache to `dest`.
///
/// ASYNC — I/O-bound.
#[pyfunction]
fn checkout<'py>(
    py: Python<'py>,
    snapshot_id: PyRef<'_, SnapshotId>,
    dest: &Bound<'_, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    let sid_hex = snapshot_id.hex.clone();
    let dest_buf = path_arg(dest)?;
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let sid = snapdir_api::SnapshotId::from_hex(&sid_hex)
            .map_err(|e| Python::with_gil(|py| raise_snapdir_err(py, e)))?;
        let result =
            snapdir_api::checkout(&sid, &dest_buf, &snapdir_api::CheckoutOptions::default()).await;
        Python::with_gil(|py| {
            api_result(py, result)?;
            Ok(py.None())
        })
    })
}

// ---------------------------------------------------------------------------
// sync() — ASYNC
// ---------------------------------------------------------------------------

/// Copies a snapshot from `src` to `dst`.
///
/// ASYNC — network / I/O-bound.
#[pyfunction]
fn sync<'py>(
    py: Python<'py>,
    snapshot_id: PyRef<'_, SnapshotId>,
    src: PyRef<'_, StoreUri>,
    dst: PyRef<'_, StoreUri>,
) -> PyResult<Bound<'py, PyAny>> {
    let sid_hex = snapshot_id.hex.clone();
    let src_raw = src.raw.clone();
    let dst_raw = dst.raw.clone();
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let sid = snapdir_api::SnapshotId::from_hex(&sid_hex)
            .map_err(|e| Python::with_gil(|py| raise_snapdir_err(py, e)))?;
        let src = snapdir_api::StoreUri::parse(&src_raw)
            .map_err(|e| Python::with_gil(|py| raise_snapdir_err(py, e)))?;
        let dst = snapdir_api::StoreUri::parse(&dst_raw)
            .map_err(|e| Python::with_gil(|py| raise_snapdir_err(py, e)))?;
        let result =
            snapdir_api::sync(&sid, &src, &dst, &snapdir_api::TransferOptions::default()).await;
        Python::with_gil(|py| {
            api_result(py, result)?;
            Ok(py.None())
        })
    })
}

// ---------------------------------------------------------------------------
// diff() — ASYNC
// ---------------------------------------------------------------------------

/// Diffs two sets of stores, returning a list of `DiffEntry` objects.
///
/// ASYNC — I/O-bound. Accepts a `DiffOptions` instance.
#[pyfunction]
fn diff<'py>(py: Python<'py>, opts: PyRef<'_, DiffOptions>) -> PyResult<Bound<'py, PyAny>> {
    let from_uris = opts.from_uris.clone();
    let to_uris = opts.to_uris.clone();
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let from: Vec<snapdir_api::StoreUri> = from_uris
            .iter()
            .map(|s| snapdir_api::StoreUri::parse(s))
            .collect::<snapdir_api::Result<Vec<_>>>()
            .map_err(|e| Python::with_gil(|py| raise_snapdir_err(py, e)))?;
        let to: Vec<snapdir_api::StoreUri> = to_uris
            .iter()
            .map(|s| snapdir_api::StoreUri::parse(s))
            .collect::<snapdir_api::Result<Vec<_>>>()
            .map_err(|e| Python::with_gil(|py| raise_snapdir_err(py, e)))?;
        let api_opts = snapdir_api::DiffOptions {
            from,
            to,
            ..snapdir_api::DiffOptions::default()
        };
        let result = snapdir_api::diff(&api_opts).await;
        Python::with_gil(|py| {
            let entries = api_result(py, result)?;
            let py_entries: Vec<PyObject> = entries
                .into_iter()
                .map(|e| {
                    Ok::<PyObject, PyErr>(
                        Bound::new(
                            py,
                            DiffEntry {
                                status: diff_status_to_str(e.status),
                                path: e.path.to_string_lossy().into_owned(),
                            },
                        )?
                        .into_any()
                        .unbind(),
                    )
                })
                .collect::<PyResult<Vec<_>>>()?;
            Ok(PyList::new(py, py_entries)?.into_any().unbind())
        })
    })
}

// ---------------------------------------------------------------------------
// verify() — ASYNC
// ---------------------------------------------------------------------------

/// Verifies a snapshot in `store`. Raises `SnapdirError` on failure.
///
/// ASYNC — I/O-bound.
/// `snapshot_id` accepts a 64-hex `str` directly (for convenience) or a
/// `SnapshotId` instance. `store` is a `StoreUri` instance.
#[pyfunction]
fn verify<'py>(
    py: Python<'py>,
    snapshot_id: &Bound<'_, PyAny>,
    store: PyRef<'_, StoreUri>,
) -> PyResult<Bound<'py, PyAny>> {
    // Accept str or SnapshotId for snapshot_id argument.
    let sid_hex: String = if let Ok(sid_ref) = snapshot_id.extract::<PyRef<SnapshotId>>() {
        sid_ref.hex.clone()
    } else {
        snapshot_id.extract::<String>()?
    };
    let store_raw = store.raw.clone();
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let sid = snapdir_api::SnapshotId::from_hex(&sid_hex)
            .map_err(|e| Python::with_gil(|py| raise_snapdir_err(py, e)))?;
        let store = snapdir_api::StoreUri::parse(&store_raw)
            .map_err(|e| Python::with_gil(|py| raise_snapdir_err(py, e)))?;
        let result =
            snapdir_api::verify(&sid, &store, &snapdir_api::VerifyOptions::default()).await;
        Python::with_gil(|py| {
            let r = api_result(py, result)?;
            Ok(PyBool::new(py, r.ok).to_owned().into_any().unbind())
        })
    })
}

// ---------------------------------------------------------------------------
// Module entry point
// ---------------------------------------------------------------------------

/// PyO3 extension module: `import snapdir`.
#[pymodule]
fn snapdir(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Exception hierarchy
    m.add("SnapdirError", m.py().get_type::<SnapdirError>())?;
    m.add("HashMismatchError", m.py().get_type::<HashMismatchError>())?;
    m.add("StoreError", m.py().get_type::<StoreError>())?;
    m.add("InFluxError", m.py().get_type::<InFluxError>())?;
    m.add("CatalogError", m.py().get_type::<CatalogError>())?;

    // Result types
    m.add_class::<PathType>()?;
    m.add_class::<ManifestEntry>()?;
    m.add_class::<Manifest>()?;
    m.add_class::<DiffEntry>()?;

    // Value types
    m.add_class::<SnapshotId>()?;
    m.add_class::<StoreUri>()?;
    m.add_class::<DiffOptions>()?;

    // Sync functions
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_function(wrap_pyfunction!(id_from_manifest, m)?)?;

    // Async functions
    m.add_function(wrap_pyfunction!(manifest, m)?)?;
    m.add_function(wrap_pyfunction!(id, m)?)?;
    m.add_function(wrap_pyfunction!(stage, m)?)?;
    m.add_function(wrap_pyfunction!(push, m)?)?;
    m.add_function(wrap_pyfunction!(fetch, m)?)?;
    m.add_function(wrap_pyfunction!(pull, m)?)?;
    m.add_function(wrap_pyfunction!(checkout, m)?)?;
    m.add_function(wrap_pyfunction!(sync, m)?)?;
    m.add_function(wrap_pyfunction!(diff, m)?)?;
    m.add_function(wrap_pyfunction!(verify, m)?)?;

    Ok(())
}
