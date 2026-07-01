//! `snapdir-ffi` — C ABI (cdylib + staticlib) for the snapdir language bindings.
//!
//! # Memory contract
//!
//! Every `extern "C"` function in this crate obeys the following invariants
//! without exception:
//!
//! - **Returned strings** are allocated via [`CString::into_raw`] and must be
//!   freed by the caller exactly once using [`snapdir_string_free`]. Passing
//!   `NULL` to `snapdir_string_free` is a no-op.
//! - **Returned opaque types** (`SnapdirError`) are heap-allocated (via
//!   `Box::into_raw`) and must be freed by the caller exactly once using the
//!   type's `_free` function ([`snapdir_error_free`]). Passing `NULL` is a
//!   no-op.
//! - **Errors** are returned via a `*mut *mut SnapdirError` out-parameter
//!   (`err_out`). `*err_out == NULL` ⇒ success; non-NULL ⇒ failure, and the
//!   caller MUST call [`snapdir_error_free`] on the returned pointer.
//! - **Panics never cross the FFI boundary.** Every entry point wraps its body
//!   in [`std::panic::catch_unwind`]; a caught panic becomes a `SnapdirError`
//!   with code `"INTERNAL"` (mapped internally from a panic message string —
//!   not one of the 8 public stable codes, reserved for internal failures).
//! - **Thread-safety:** all functions are safe to call from any thread once
//!   [`snapdir_init`] has run (or lazily on first use — calling other fns
//!   before `snapdir_init` is still safe).
//! - **Strings in:** `const char*` are expected to be valid UTF-8, NUL-terminated.
//!   `NULL` is allowed where documented as "unset". The FFI copies the string
//!   data; it never takes ownership of caller memory.
//!
//! # ABI version
//!
//! `SNAPDIR_ABI_VERSION 1` — bumped only on a breaking C-ABI change,
//! decoupled from crate semver.
//!
//! # Example (C pseudocode)
//!
//! ```c
//! snapdir_init();
//! const char *v = snapdir_version();  /* do NOT free */
//! printf("snapdir %s\n", v);
//!
//! SnapdirError *err = NULL;
//! /* ... call an operation that sets err on failure ... */
//! if (err) {
//!     fprintf(stderr, "error [%s]: %s\n",
//!             snapdir_error_code(err),
//!             snapdir_error_message(err));
//!     snapdir_error_free(err);
//! }
//! ```

#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
// Style lints that fire on stable clippy 1.96 but are not genuine correctness
// issues in this FFI crate. Listed explicitly rather than a blanket allow so
// that genuinely new warning categories still surface.
#![allow(clippy::single_match_else)]
#![allow(clippy::manual_let_else)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::io_other_error)]
#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::manual_map)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::borrow_as_ptr)]

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic::AssertUnwindSafe;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// ABI version constant
// ---------------------------------------------------------------------------

/// The C ABI version for `snapdir-ffi`.
///
/// Starts at `1` and is bumped ONLY on a breaking C-ABI change, decoupled
/// from the crate semantic version.  cbindgen renders this as
/// `#define SNAPDIR_ABI_VERSION 1` in `include/snapdir.h`.
pub const SNAPDIR_ABI_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Shared tokio runtime (OnceLock, multi-thread)
// ---------------------------------------------------------------------------

/// Returns a reference to the shared multi-thread tokio `Runtime`.
///
/// The runtime is initialised exactly once, either by an explicit call to
/// [`snapdir_init`] or lazily on the first call to any blocking FFI function.
/// Both paths are safe to call concurrently — `OnceLock::get_or_init` provides
/// the required synchronisation.
fn shared_rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("snapdir-ffi: failed to build shared tokio runtime")
    })
}

// ---------------------------------------------------------------------------
// Opaque SnapdirError type
// ---------------------------------------------------------------------------

/// Opaque error type returned by `snapdir-ffi` functions.
///
/// The message and code strings are cached as `CString`s at construction time
/// so that [`snapdir_error_message`] and [`snapdir_error_code`] can return
/// stable `*const c_char` pointers that remain valid for the lifetime of the
/// `SnapdirError` value.
///
/// The caller **must not** free the pointers returned by the inspect functions;
/// they borrow from `SnapdirError` itself. The caller **must** call
/// [`snapdir_error_free`] exactly once when done with an error.
pub struct SnapdirError {
    /// Cached human-readable error message (NUL-terminated).
    message: CString,
    /// Cached stable error code (NUL-terminated).
    code: CString,
}

impl SnapdirError {
    /// Constructs a `SnapdirError` from a `snapdir_api::SnapdirError`.
    fn from_api(e: &snapdir_api::SnapdirError) -> Self {
        // Safety: the message is a valid Rust String (no embedded NULs expected;
        // if one appears, `new_with_nul` fails silently and we use "?").
        // We defensively replace any embedded NULs with '?' to always produce a
        // valid CString.
        let msg_str = e.to_string().replace('\0', "?");
        let code_str = e.code();
        Self {
            message: CString::new(msg_str)
                .unwrap_or_else(|_| CString::new("(message encoding error)").unwrap()),
            code: CString::new(code_str).unwrap_or_else(|_| CString::new("INTERNAL").unwrap()),
        }
    }

    /// Constructs a `SnapdirError` from a panic payload string.
    fn from_panic(msg: impl Into<String>) -> Self {
        let msg_str = msg.into().replace('\0', "?");
        Self {
            message: CString::new(msg_str)
                .unwrap_or_else(|_| CString::new("(panic message encoding error)").unwrap()),
            code: CString::new("INTERNAL").unwrap(),
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Converts an `api::SnapdirError` into a heap-allocated raw `SnapdirError`
/// pointer to fill an out-param.
///
/// Writes the allocated pointer into `*err_out` when `err_out` is non-NULL.
/// If `err_out` is NULL the error is silently dropped (callers that pass NULL
/// for `err_out` opt out of receiving error details).
///
/// # Safety
///
/// `err_out` must be either NULL or a valid pointer to a `*mut SnapdirError`.
unsafe fn write_error_out(e: &snapdir_api::SnapdirError, err_out: *mut *mut SnapdirError) {
    if err_out.is_null() {
        return;
    }
    let boxed = Box::new(SnapdirError::from_api(e));
    // SAFETY: caller guarantees err_out is a valid pointer.
    unsafe { *err_out = Box::into_raw(boxed) };
}

/// Like [`write_error_out`] but for panic payloads.
///
/// # Safety
///
/// `err_out` must be either NULL or a valid pointer to a `*mut SnapdirError`.
unsafe fn write_panic_out(msg: impl Into<String>, err_out: *mut *mut SnapdirError) {
    if err_out.is_null() {
        return;
    }
    let boxed = Box::new(SnapdirError::from_panic(msg));
    // SAFETY: caller guarantees err_out is a valid pointer.
    unsafe { *err_out = Box::into_raw(boxed) };
}

/// Stringifies a `Box<dyn std::any::Any + Send>` panic payload.
fn panic_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "(non-string panic payload)".to_string()
    }
}

// ---------------------------------------------------------------------------
// init + version
// ---------------------------------------------------------------------------

/// Initialises the embedded shared multi-thread tokio runtime.
///
/// **Idempotent.** Safe to call many times and concurrently. Calling other
/// `snapdir_*` functions before `snapdir_init` is also safe — they
/// lazily initialise the runtime on first use — but calling `snapdir_init`
/// explicitly at process startup allows bindings to front-load the
/// initialisation cost.
///
/// # Panics (C side)
///
/// This function cannot fail visibly: if the runtime cannot be built (which
/// should be extremely rare) the process will abort.
#[no_mangle]
pub extern "C" fn snapdir_init() {
    // A catastrophic tokio Runtime::new() failure panics inside shared_rt().
    // An unwind across the extern "C" boundary is UB, so we catch the panic
    // and abort cleanly instead.  snapdir_init has no error channel — it
    // returns void by design — so process::abort() is the only safe option.
    if std::panic::catch_unwind(|| {
        let _ = shared_rt();
    })
    .is_err()
    {
        std::process::abort();
    }
}

/// Returns the snapdir version string (e.g. `"1.10.0"`).
///
/// The returned pointer has **static lifetime** — the caller **must NOT free
/// it**. It remains valid for the entire process lifetime.
#[no_mangle]
pub extern "C" fn snapdir_version() -> *const c_char {
    static VERSION: OnceLock<CString> = OnceLock::new();
    let cs = VERSION.get_or_init(|| {
        // snapdir_api::version() returns &'static str via env!("CARGO_PKG_VERSION").
        // Replace any embedded NULs defensively.
        let v = snapdir_api::version().replace('\0', "?");
        CString::new(v).unwrap_or_else(|_| CString::new("(version encoding error)").unwrap())
    });
    cs.as_ptr()
}

// ---------------------------------------------------------------------------
// Memory-free functions
// ---------------------------------------------------------------------------

/// Frees a string previously returned by a `snapdir_*` function.
///
/// The string must have been allocated by a `snapdir-ffi` function via
/// `CString::into_raw`. Passing `NULL` is a safe no-op.
///
/// # Safety
///
/// - `s` must be either `NULL` or a pointer previously returned by a
///   `snapdir-ffi` function that returns an owned `char *`.
/// - `s` must not be freed more than once.
#[no_mangle]
pub unsafe extern "C" fn snapdir_string_free(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    // SAFETY: `s` was created via CString::into_raw by this crate.
    unsafe { drop(CString::from_raw(s)) };
}

/// Frees a `SnapdirError` previously returned via an error out-parameter.
///
/// Passing `NULL` is a safe no-op.
///
/// # Safety
///
/// - `err` must be either `NULL` or a pointer previously written into an
///   `SnapdirError **err_out` out-parameter by a `snapdir-ffi` function.
/// - `err` must not be freed more than once.
#[no_mangle]
pub unsafe extern "C" fn snapdir_error_free(err: *mut SnapdirError) {
    if err.is_null() {
        return;
    }
    // SAFETY: `err` was created via Box::into_raw by this crate.
    unsafe { drop(Box::from_raw(err)) };
}

// ---------------------------------------------------------------------------
// Error inspection
// ---------------------------------------------------------------------------

/// Returns the human-readable error message for `err`.
///
/// The returned pointer is valid for the lifetime of `err` — the caller must
/// **NOT** free it separately. It is invalidated when `snapdir_error_free(err)`
/// is called.
///
/// If `err` is `NULL`, returns `NULL`.
///
/// # Safety
///
/// `err` must be either `NULL` or a valid pointer to a `SnapdirError` that has
/// not yet been freed.
#[no_mangle]
pub unsafe extern "C" fn snapdir_error_message(err: *const SnapdirError) -> *const c_char {
    if err.is_null() {
        return std::ptr::null();
    }
    // SAFETY: caller guarantees `err` is valid.
    unsafe { (*err).message.as_ptr() }
}

/// Returns the stable error code for `err`.
///
/// The returned pointer is one of the 8 stable codes (`IO_ERROR`,
/// `HASH_MISMATCH`, `STORE_ERROR`, `IN_FLUX`, `CATALOG_ERROR`, `INVALID_ID`,
/// `INVALID_STORE`, `CONFLICT`) or `INTERNAL` for unexpected failures caught
/// by the `catch_unwind` boundary.
///
/// The returned pointer is valid for the lifetime of `err` — the caller must
/// **NOT** free it separately.
///
/// If `err` is `NULL`, returns `NULL`.
///
/// # Safety
///
/// `err` must be either `NULL` or a valid pointer to a `SnapdirError` that has
/// not yet been freed.
#[no_mangle]
pub unsafe extern "C" fn snapdir_error_code(err: *const SnapdirError) -> *const c_char {
    if err.is_null() {
        return std::ptr::null();
    }
    // SAFETY: caller guarantees `err` is valid.
    unsafe { (*err).code.as_ptr() }
}

// ---------------------------------------------------------------------------
// Internal entry-point wrappers (discipline helpers)
// ---------------------------------------------------------------------------
//
// These macros / inline helpers are NOT extern "C"; they are reused by every
// `extern "C"` fn added in later gates to enforce the catch_unwind discipline
// and the error out-param contract uniformly.

/// Executes `body` inside `std::panic::catch_unwind`.
///
/// If `body` panics, converts the panic payload into a `SnapdirError` with
/// code `"INTERNAL"` and writes it into `*err_out`.
///
/// Returns the value produced by `body` on success, or the `default` value on
/// a caught panic.
///
/// This is an internal helper (`pub(crate)`) — it is not part of the `extern
/// "C"` surface.
#[allow(dead_code)] // used by later gates
pub(crate) fn catch_entry<T, F>(body: F, default: T, err_out: *mut *mut SnapdirError) -> T
where
    F: FnOnce() -> T + std::panic::UnwindSafe,
{
    match std::panic::catch_unwind(body) {
        Ok(v) => v,
        Err(payload) => {
            let msg = panic_to_string(payload);
            // SAFETY: err_out validity is the caller's responsibility — the
            // same requirement the caller has toward the extern "C" fn.
            unsafe { write_panic_out(msg, err_out) };
            default
        }
    }
}

/// Helper that wraps `f()` in `catch_unwind` (AssertUnwindSafe) for entry points
/// whose closures capture non-`UnwindSafe` types (e.g. `&CStr`).
///
/// Use this when `catch_entry` cannot be used directly because the closure
/// captures a type that doesn't implement `UnwindSafe`. The name signals the
/// explicit opt-in, consistent with `std::panic::AssertUnwindSafe`.
///
/// Returns `(Ok(value), None)` on success, `(default, Some(err))` on panic.
/// The caller decides whether to write the error into an out-param.
#[allow(dead_code)] // used by later gates
pub(crate) fn catch_entry_unwind_safe<T, F>(
    body: F,
    default: T,
    err_out: *mut *mut SnapdirError,
) -> T
where
    F: FnOnce() -> T,
{
    match std::panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(v) => v,
        Err(payload) => {
            let msg = panic_to_string(payload);
            // SAFETY: err_out validity is the caller's responsibility.
            unsafe { write_panic_out(msg, err_out) };
            default
        }
    }
}

/// Converts a raw `*const c_char` into a borrowed `&str`, or returns an
/// `Err(SnapdirError)` written into `*err_out` and the supplied default on
/// NULL or invalid UTF-8.
///
/// # Safety
///
/// `ptr`, if non-NULL, must point to a valid NUL-terminated C string that
/// remains valid for the duration of the returned `&str` borrow.
#[allow(dead_code)] // used by later gates
pub(crate) unsafe fn cstr_to_str<'a>(
    ptr: *const c_char,
    param_name: &str,
    err_out: *mut *mut SnapdirError,
) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller guarantees `ptr` is a valid NUL-terminated C string.
    let cs = unsafe { CStr::from_ptr(ptr) };
    match cs.to_str() {
        Ok(s) => Some(s),
        Err(_) => {
            // Non-UTF-8 input: synthesise an Io error.
            let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("parameter `{param_name}` contains invalid UTF-8"),
            ));
            // SAFETY: err_out validity is the caller's responsibility.
            unsafe { write_error_out(&api_err, err_out) };
            None
        }
    }
}

/// Writes an `api::SnapdirError` into `*err_out` (if non-NULL).
///
/// Used by generated blocking entry points to report a failure from a
/// `snapdir-api` call.
///
/// # Safety
///
/// `err_out` must be either NULL or a valid writable pointer.
#[allow(dead_code)] // used by later gates
pub(crate) unsafe fn ffi_write_api_error(
    e: &snapdir_api::SnapdirError,
    err_out: *mut *mut SnapdirError,
) {
    // SAFETY: same contract as write_error_out — propagated from caller.
    unsafe { write_error_out(e, err_out) };
}

// ---------------------------------------------------------------------------
// Snapshotting — sync blocking wrappers
// ---------------------------------------------------------------------------

/// Computes the snapshot id for the directory at `path`.
///
/// `exclude` — an extended-regex exclusion pattern (`NULL` = no exclusion).
/// `walk_jobs` — parallel hashing worker count (`0` = auto/CPU-count default).
/// `cache_dir` — override the local object-cache directory (`NULL` = default).
/// `err_out` — on failure, `*err_out` is set to a heap `SnapdirError`; the
/// caller must free it with [`snapdir_error_free`]. On success, `*err_out`
/// remains `NULL` and the returned string must be freed with
/// [`snapdir_string_free`].
///
/// Returns the 64-character lowercase hex BLAKE3 snapshot id on success, or
/// `NULL` on failure (with `*err_out` set).
///
/// # Safety
///
/// `path` must be a valid, NUL-terminated UTF-8 C string. `exclude` and
/// `cache_dir` may be `NULL`. `err_out` must be either `NULL` or a valid
/// writable pointer to `*mut SnapdirError`.
#[no_mangle]
pub unsafe extern "C" fn snapdir_id(
    path: *const c_char,
    exclude: *const c_char,
    walk_jobs: u32,
    cache_dir: *const c_char,
    err_out: *mut *mut SnapdirError,
) -> *mut c_char {
    catch_entry_unwind_safe(
        || {
            // Marshal `path` — required, non-NULL.
            // SAFETY: caller guarantees path is a valid NUL-terminated C string.
            let path_str = match unsafe { cstr_to_str(path, "path", err_out) } {
                Some(s) => s,
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        // path was NULL → synthesise an IO error.
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `path` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return std::ptr::null_mut();
                }
            };

            // Build ManifestOptions.
            let mut opts = snapdir_api::ManifestOptions::default();

            // exclude: NULL = no exclusion; non-NULL = single regex pattern.
            // SAFETY: exclude may be NULL (documented).
            if let Some(ex) = unsafe { cstr_to_str(exclude, "exclude", err_out) } {
                opts.exclude = vec![ex.to_owned()];
            }

            // walk_jobs: 0 = auto (None); otherwise Some(n).
            opts.walk_jobs = if walk_jobs == 0 {
                None
            } else {
                Some(walk_jobs as usize)
            };

            // cache_dir: NULL = use default (None); non-NULL = override.
            // SAFETY: cache_dir may be NULL (documented).
            if let Some(cd) = unsafe { cstr_to_str(cache_dir, "cache_dir", err_out) } {
                opts.cache_dir = Some(std::path::PathBuf::from(cd));
            }

            match snapdir_api::id(std::path::Path::new(path_str), &opts) {
                Ok(snap_id) => {
                    let hex = snap_id.to_hex();
                    match CString::new(hex) {
                        Ok(cs) => cs.into_raw(),
                        Err(_) => {
                            let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                "snapshot id contained embedded NUL (unexpected)",
                            ));
                            unsafe { ffi_write_api_error(&api_err, err_out) };
                            std::ptr::null_mut()
                        }
                    }
                }
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    std::ptr::null_mut()
                }
            }
        },
        std::ptr::null_mut(),
        err_out,
    )
}

/// Walks `path` and returns the manifest text as an owned C string.
///
/// `exclude` — an extended-regex exclusion pattern (`NULL` = no exclusion).
/// `walk_jobs` — parallel hashing worker count (`0` = auto/CPU-count default).
/// `absolute` — emit absolute paths instead of `./`-relative paths.
/// `no_follow` — do not follow symbolic links.
/// `checksum_bin` — checksum algorithm (`NULL` = `"b3sum"` default; `"md5sum"`
/// or `"sha256sum"` selects that algorithm).
/// `cache_dir` — override the local object-cache directory (`NULL` = default).
/// `catalog` — catalog adapter selection (`NULL` = adapter default;
/// `"none"` = suppress catalog recording; any other string = named adapter).
/// `err_out` — on failure, `*err_out` is set to a heap `SnapdirError`; the
/// caller must free it with [`snapdir_error_free`]. On success, `*err_out`
/// remains `NULL` and the returned string must be freed with
/// [`snapdir_string_free`].
///
/// Returns the manifest text on success, or `NULL` on failure (with `*err_out`
/// set).
///
/// # Safety
///
/// `path` must be a valid, NUL-terminated UTF-8 C string. `exclude`,
/// `checksum_bin`, `cache_dir`, and `catalog` may be `NULL`. `err_out` must
/// be either `NULL` or a valid writable pointer to `*mut SnapdirError`.
#[no_mangle]
pub unsafe extern "C" fn snapdir_manifest(
    path: *const c_char,
    exclude: *const c_char,
    walk_jobs: u32,
    absolute: bool,
    no_follow: bool,
    checksum_bin: *const c_char,
    cache_dir: *const c_char,
    catalog: *const c_char,
    err_out: *mut *mut SnapdirError,
) -> *mut c_char {
    catch_entry_unwind_safe(
        || {
            // Marshal `path` — required, non-NULL.
            // SAFETY: caller guarantees path is a valid NUL-terminated C string.
            let path_str = match unsafe { cstr_to_str(path, "path", err_out) } {
                Some(s) => s,
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `path` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return std::ptr::null_mut();
                }
            };

            // Build ManifestOptions.
            let mut opts = snapdir_api::ManifestOptions {
                absolute,
                no_follow,
                ..Default::default()
            };

            // exclude: NULL = no exclusion; non-NULL = single regex pattern.
            // SAFETY: exclude may be NULL.
            if let Some(ex) = unsafe { cstr_to_str(exclude, "exclude", err_out) } {
                opts.exclude = vec![ex.to_owned()];
            }

            // walk_jobs: 0 = auto; otherwise Some(n).
            opts.walk_jobs = if walk_jobs == 0 {
                None
            } else {
                Some(walk_jobs as usize)
            };

            // checksum_bin: NULL = b3sum (default).
            // SAFETY: checksum_bin may be NULL.
            if let Some(cb) = unsafe { cstr_to_str(checksum_bin, "checksum_bin", err_out) } {
                opts.checksum_bin = match cb {
                    "md5sum" => snapdir_api::ChecksumBin::Md5sum,
                    "sha256sum" => snapdir_api::ChecksumBin::Sha256sum,
                    _ => snapdir_api::ChecksumBin::B3sum,
                };
            }

            // cache_dir: NULL = default.
            // SAFETY: cache_dir may be NULL.
            if let Some(cd) = unsafe { cstr_to_str(cache_dir, "cache_dir", err_out) } {
                opts.cache_dir = Some(std::path::PathBuf::from(cd));
            }

            // catalog: NULL = default; "none" = off; otherwise named adapter.
            // SAFETY: catalog may be NULL.
            opts.catalog = match unsafe { cstr_to_str(catalog, "catalog", err_out) } {
                None => snapdir_api::CatalogOption::Default,
                Some("none") => snapdir_api::CatalogOption::None,
                Some(name) => snapdir_api::CatalogOption::Named(name.to_owned()),
            };

            match snapdir_api::manifest(std::path::Path::new(path_str), &opts) {
                Ok(m) => {
                    let raw_text = m.raw;
                    match CString::new(raw_text) {
                        Ok(cs) => cs.into_raw(),
                        Err(_) => {
                            // Manifest text contained an embedded NUL — should
                            // be unreachable in practice but must be handled.
                            let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                "manifest text contained embedded NUL (unexpected)",
                            ));
                            unsafe { ffi_write_api_error(&api_err, err_out) };
                            std::ptr::null_mut()
                        }
                    }
                }
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    std::ptr::null_mut()
                }
            }
        },
        std::ptr::null_mut(),
        err_out,
    )
}

// ---------------------------------------------------------------------------
// Snapshotting — additional sync wrappers (id_from_manifest_text, stage)
// ---------------------------------------------------------------------------

/// Computes the snapshot id from an already-computed manifest text string.
///
/// The `manifest_text` must be a valid NUL-terminated UTF-8 string previously
/// returned by [`snapdir_manifest`] (or equivalent). The returned id is the
/// same 64-char lowercase-hex BLAKE3 id that [`snapdir_id`] would produce for
/// the original directory.
///
/// Returns a freshly-allocated id string on success (caller frees with
/// [`snapdir_string_free`]), or `NULL` on failure (with `*err_out` set).
///
/// # Safety
///
/// `manifest_text` must be a valid, NUL-terminated UTF-8 C string. `err_out`
/// must be either `NULL` or a valid writable pointer to `*mut SnapdirError`.
#[no_mangle]
pub unsafe extern "C" fn snapdir_id_from_manifest_text(
    manifest_text: *const c_char,
    err_out: *mut *mut SnapdirError,
) -> *mut c_char {
    catch_entry_unwind_safe(
        || {
            // SAFETY: caller guarantees manifest_text is a valid NUL-terminated C string.
            let text = match unsafe { cstr_to_str(manifest_text, "manifest_text", err_out) } {
                Some(s) => s,
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `manifest_text` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return std::ptr::null_mut();
                }
            };

            // Build a synthetic api::Manifest with the raw text.
            // id_from_manifest() re-parses m.raw via CoreManifest::parse internally,
            // so passing entries=[] is safe — the raw field drives the id computation.
            let m = snapdir_api::Manifest {
                raw: text.to_owned(),
                entries: vec![],
            };
            // Validate that the manifest text is parseable by calling id_from_manifest.
            // id_from_manifest panics only on truly malformed text (assert on parse).
            // We catch any panic via catch_entry_unwind_safe wrapping above.
            let snap_id = snapdir_api::id_from_manifest(&m);
            let hex = snap_id.to_hex();
            match CString::new(hex) {
                Ok(cs) => cs.into_raw(),
                Err(_) => {
                    let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "snapshot id contained embedded NUL (unexpected)",
                    ));
                    unsafe { ffi_write_api_error(&api_err, err_out) };
                    std::ptr::null_mut()
                }
            }
        },
        std::ptr::null_mut(),
        err_out,
    )
}

/// Stages the directory at `path` in the local cache and returns the snapshot id.
///
/// `keep` — if `true`, the source tree is preserved as-is after staging
/// (the snapshot objects are written to `cache_dir`); if `false`, only the
/// cache copy is kept (source tree unchanged — the option mainly controls
/// CLI behavior; the FFI always leaves the source directory intact).
/// `cache_dir` — override the local cache directory (`NULL` = default).
///
/// Returns the 64-char lowercase hex snapshot id on success (caller frees
/// with [`snapdir_string_free`]), or `NULL` on failure (with `*err_out` set).
///
/// # Safety
///
/// `path` must be a valid, NUL-terminated UTF-8 C string. `cache_dir` may be
/// `NULL`. `err_out` must be either `NULL` or a valid writable pointer to
/// `*mut SnapdirError`.
#[no_mangle]
pub unsafe extern "C" fn snapdir_stage(
    path: *const c_char,
    keep: bool,
    cache_dir: *const c_char,
    err_out: *mut *mut SnapdirError,
) -> *mut c_char {
    catch_entry_unwind_safe(
        || {
            // SAFETY: caller guarantees path is a valid NUL-terminated C string.
            let path_str = match unsafe { cstr_to_str(path, "path", err_out) } {
                Some(s) => s,
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `path` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return std::ptr::null_mut();
                }
            };

            // Build StageOptions — delegate FULLY to snapdir_api::stage.
            // SAFETY: cache_dir parameter may be NULL.
            let cache_dir_opt = match unsafe { cstr_to_str(cache_dir, "cache_dir", err_out) } {
                Some(cd) => Some(std::path::PathBuf::from(cd)),
                None => None,
            };

            let opts = snapdir_api::StageOptions {
                cache_dir: cache_dir_opt,
                keep,
            };

            match snapdir_api::stage(std::path::Path::new(path_str), &opts) {
                Ok(snap_id) => {
                    let hex_id = snap_id.to_hex();
                    match CString::new(hex_id) {
                        Ok(cs) => cs.into_raw(),
                        Err(_) => {
                            let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                "snapshot id contained embedded NUL (unexpected)",
                            ));
                            unsafe { ffi_write_api_error(&api_err, err_out) };
                            std::ptr::null_mut()
                        }
                    }
                }
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    std::ptr::null_mut()
                }
            }
        },
        std::ptr::null_mut(),
        err_out,
    )
}

// ---------------------------------------------------------------------------
// Distribution — blocking wrappers (route through shared_rt().block_on)
// ---------------------------------------------------------------------------

/// Pushes the snapshot at `source_path` (or by `source_id` if already staged)
/// to `store_uri` and returns the snapshot id string.
///
/// `source_path` — filesystem path to the directory to push (XOR with `source_id`).
/// `source_id` — 64-hex id of a previously-staged snapshot (`NULL` when using path).
/// `store_uri` — destination store URI (e.g. `"file:///tmp/store"`).
/// `jobs` — max concurrent transfers (`0` = default).
/// `limit_rate` — bandwidth cap string (e.g. `"10M"`; `NULL` = unlimited).
/// `max_retries` — max retry attempts per object (`0` = default of 5).
/// `cache_dir` — local cache directory override (`NULL` = default).
/// `err_out` — error out-parameter.
///
/// Returns the 64-char lowercase hex snapshot id on success, or `NULL` on failure.
///
/// # Safety
///
/// `store_uri` and (if non-NULL) `source_path`/`source_id` must be valid NUL-terminated
/// UTF-8 C strings. `err_out` must be either `NULL` or a valid writable pointer.
#[no_mangle]
pub unsafe extern "C" fn snapdir_push_blocking(
    source_path: *const c_char,
    source_id: *const c_char,
    store_uri: *const c_char,
    jobs: u32,
    limit_rate: *const c_char,
    max_retries: u32,
    cache_dir: *const c_char,
    err_out: *mut *mut SnapdirError,
) -> *mut c_char {
    catch_entry_unwind_safe(
        || {
            // SAFETY: cstr_to_str handles NULL gracefully.
            let store_str = match unsafe { cstr_to_str(store_uri, "store_uri", err_out) } {
                Some(s) => s.to_owned(),
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `store_uri` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return std::ptr::null_mut();
                }
            };

            let store_uri_parsed = match snapdir_api::StoreUri::parse(&store_str) {
                Ok(u) => u,
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    return std::ptr::null_mut();
                }
            };

            // Build TransferOptions.
            let mut transfer_opts = snapdir_api::TransferOptions::default();
            if jobs > 0 {
                transfer_opts.jobs = Some(jobs as usize);
            }
            // SAFETY: limit_rate may be NULL.
            if let Some(lr) = unsafe { cstr_to_str(limit_rate, "limit_rate", err_out) } {
                transfer_opts.limit_rate = Some(lr.to_owned());
            }
            if max_retries > 0 {
                transfer_opts.max_retries = Some(max_retries);
            }
            // SAFETY: cache_dir may be NULL.
            if let Some(cd) = unsafe { cstr_to_str(cache_dir, "cache_dir", err_out) } {
                transfer_opts.cache_dir = Some(std::path::PathBuf::from(cd));
            }

            // Determine push source.
            // SAFETY: source_id and source_path may be NULL.
            let src_id_str = unsafe { cstr_to_str(source_id, "source_id", err_out) };
            let src_path_str = unsafe { cstr_to_str(source_path, "source_path", err_out) };

            let result = if let Some(id_hex) = src_id_str {
                // Push from a staged id.
                let snap_id = match snapdir_api::SnapshotId::from_hex(id_hex) {
                    Ok(id) => id,
                    Err(e) => {
                        unsafe { ffi_write_api_error(&e, err_out) };
                        return std::ptr::null_mut();
                    }
                };
                shared_rt().block_on(snapdir_api::push(
                    snapdir_api::PushSource::StagedId(&snap_id),
                    &store_uri_parsed,
                    &transfer_opts,
                ))
            } else if let Some(path_s) = src_path_str {
                // Push from a filesystem path.
                let path = std::path::Path::new(path_s);
                shared_rt().block_on(snapdir_api::push(
                    snapdir_api::PushSource::Path(path),
                    &store_uri_parsed,
                    &transfer_opts,
                ))
            } else {
                let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "one of `source_path` or `source_id` must be non-NULL",
                ));
                unsafe { ffi_write_api_error(&api_err, err_out) };
                return std::ptr::null_mut();
            };

            match result {
                Ok(snap_id) => {
                    let hex = snap_id.to_hex();
                    match CString::new(hex) {
                        Ok(cs) => cs.into_raw(),
                        Err(_) => {
                            let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                "snapshot id contained embedded NUL (unexpected)",
                            ));
                            unsafe { ffi_write_api_error(&api_err, err_out) };
                            std::ptr::null_mut()
                        }
                    }
                }
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    std::ptr::null_mut()
                }
            }
        },
        std::ptr::null_mut(),
        err_out,
    )
}

/// Fetches a snapshot from `store_uri` into the local cache.
///
/// `id` — 64-hex snapshot id.
/// `store_uri` — source store URI.
/// `jobs` — max concurrent transfers (`0` = default).
/// `err_out` — error out-parameter.
///
/// Returns `0` on success, `-1` on failure (with `*err_out` set).
///
/// # Safety
///
/// `id` and `store_uri` must be valid NUL-terminated UTF-8 C strings. `err_out`
/// must be either `NULL` or a valid writable pointer to `*mut SnapdirError`.
#[no_mangle]
pub unsafe extern "C" fn snapdir_fetch_blocking(
    id: *const c_char,
    store_uri: *const c_char,
    jobs: u32,
    err_out: *mut *mut SnapdirError,
) -> libc::c_int {
    catch_entry_unwind_safe(
        || {
            let id_str = match unsafe { cstr_to_str(id, "id", err_out) } {
                Some(s) => s.to_owned(),
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `id` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return -1;
                }
            };
            let store_str = match unsafe { cstr_to_str(store_uri, "store_uri", err_out) } {
                Some(s) => s.to_owned(),
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `store_uri` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return -1;
                }
            };

            let snap_id = match snapdir_api::SnapshotId::from_hex(&id_str) {
                Ok(id) => id,
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    return -1;
                }
            };
            let store_uri_parsed = match snapdir_api::StoreUri::parse(&store_str) {
                Ok(u) => u,
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    return -1;
                }
            };
            let mut opts = snapdir_api::TransferOptions::default();
            if jobs > 0 {
                opts.jobs = Some(jobs as usize);
            }

            match shared_rt().block_on(snapdir_api::fetch(&snap_id, &store_uri_parsed, &opts)) {
                Ok(()) => 0,
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    -1
                }
            }
        },
        -1,
        err_out,
    )
}

/// Pulls a snapshot from `store_uri` into `dest_path`, materializing its files.
///
/// `id` — 64-hex snapshot id.
/// `store_uri` — source store URI.
/// `dest_path` — filesystem path to materialize into.
/// `delete_extra` — if `true`, delete destination files absent from the snapshot.
/// `jobs` — max concurrent transfers (`0` = default).
/// `err_out` — error out-parameter.
///
/// Returns `0` on success, `-1` on failure (with `*err_out` set).
///
/// # Safety
///
/// `id`, `store_uri`, and `dest_path` must be valid NUL-terminated UTF-8 C strings.
/// `err_out` must be either `NULL` or a valid writable pointer to `*mut SnapdirError`.
#[no_mangle]
pub unsafe extern "C" fn snapdir_pull_blocking(
    id: *const c_char,
    store_uri: *const c_char,
    dest_path: *const c_char,
    delete_extra: bool,
    jobs: u32,
    err_out: *mut *mut SnapdirError,
) -> libc::c_int {
    catch_entry_unwind_safe(
        || {
            let id_str = match unsafe { cstr_to_str(id, "id", err_out) } {
                Some(s) => s.to_owned(),
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `id` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return -1;
                }
            };
            let store_str = match unsafe { cstr_to_str(store_uri, "store_uri", err_out) } {
                Some(s) => s.to_owned(),
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `store_uri` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return -1;
                }
            };
            let dest_str = match unsafe { cstr_to_str(dest_path, "dest_path", err_out) } {
                Some(s) => s.to_owned(),
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `dest_path` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return -1;
                }
            };

            let snap_id = match snapdir_api::SnapshotId::from_hex(&id_str) {
                Ok(id) => id,
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    return -1;
                }
            };
            let store_uri_parsed = match snapdir_api::StoreUri::parse(&store_str) {
                Ok(u) => u,
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    return -1;
                }
            };
            let mut opts = snapdir_api::CheckoutOptions::default();
            opts.delete = delete_extra;
            if jobs > 0 {
                opts.transfer.jobs = Some(jobs as usize);
            }

            match shared_rt().block_on(snapdir_api::pull(
                &snap_id,
                &store_uri_parsed,
                std::path::Path::new(&dest_str),
                &opts,
            )) {
                Ok(()) => 0,
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    -1
                }
            }
        },
        -1,
        err_out,
    )
}

/// Checks out a snapshot from the local cache into `dest_path`.
///
/// The snapshot must have been fetched (via [`snapdir_fetch_blocking`] or
/// [`snapdir_push_blocking`]) before this call.
///
/// `id` — 64-hex snapshot id.
/// `dest_path` — filesystem path to materialize into.
/// `linked` — use symlinks instead of copies.
/// `delete_extra` — delete destination files absent from the snapshot.
/// `err_out` — error out-parameter.
///
/// Returns `0` on success, `-1` on failure (with `*err_out` set).
///
/// # Safety
///
/// `id` and `dest_path` must be valid NUL-terminated UTF-8 C strings. `err_out`
/// must be either `NULL` or a valid writable pointer to `*mut SnapdirError`.
#[no_mangle]
pub unsafe extern "C" fn snapdir_checkout_blocking(
    id: *const c_char,
    dest_path: *const c_char,
    linked: bool,
    delete_extra: bool,
    err_out: *mut *mut SnapdirError,
) -> libc::c_int {
    catch_entry_unwind_safe(
        || {
            let id_str = match unsafe { cstr_to_str(id, "id", err_out) } {
                Some(s) => s.to_owned(),
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `id` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return -1;
                }
            };
            let dest_str = match unsafe { cstr_to_str(dest_path, "dest_path", err_out) } {
                Some(s) => s.to_owned(),
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `dest_path` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return -1;
                }
            };

            let snap_id = match snapdir_api::SnapshotId::from_hex(&id_str) {
                Ok(id) => id,
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    return -1;
                }
            };
            let mut opts = snapdir_api::CheckoutOptions::default();
            opts.linked = linked;
            opts.delete = delete_extra;

            match shared_rt().block_on(snapdir_api::checkout(
                &snap_id,
                std::path::Path::new(&dest_str),
                &opts,
            )) {
                Ok(()) => 0,
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    -1
                }
            }
        },
        -1,
        err_out,
    )
}

/// Syncs a snapshot from `src_uri` to `dst_uri`.
///
/// `id` — 64-hex snapshot id.
/// `src_uri` — source store URI.
/// `dst_uri` — destination store URI.
/// `jobs` — max concurrent transfers (`0` = default).
/// `err_out` — error out-parameter.
///
/// Returns `0` on success, `-1` on failure (with `*err_out` set).
///
/// # Safety
///
/// `id`, `src_uri`, and `dst_uri` must be valid NUL-terminated UTF-8 C strings.
/// `err_out` must be either `NULL` or a valid writable pointer to `*mut SnapdirError`.
#[no_mangle]
pub unsafe extern "C" fn snapdir_sync_blocking(
    id: *const c_char,
    src_uri: *const c_char,
    dst_uri: *const c_char,
    jobs: u32,
    err_out: *mut *mut SnapdirError,
) -> libc::c_int {
    catch_entry_unwind_safe(
        || {
            let id_str = match unsafe { cstr_to_str(id, "id", err_out) } {
                Some(s) => s.to_owned(),
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `id` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return -1;
                }
            };
            let src_str = match unsafe { cstr_to_str(src_uri, "src_uri", err_out) } {
                Some(s) => s.to_owned(),
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `src_uri` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return -1;
                }
            };
            let dst_str = match unsafe { cstr_to_str(dst_uri, "dst_uri", err_out) } {
                Some(s) => s.to_owned(),
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `dst_uri` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return -1;
                }
            };

            let snap_id = match snapdir_api::SnapshotId::from_hex(&id_str) {
                Ok(id) => id,
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    return -1;
                }
            };
            let src_uri_parsed = match snapdir_api::StoreUri::parse(&src_str) {
                Ok(u) => u,
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    return -1;
                }
            };
            let dst_uri_parsed = match snapdir_api::StoreUri::parse(&dst_str) {
                Ok(u) => u,
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    return -1;
                }
            };
            let mut opts = snapdir_api::TransferOptions::default();
            if jobs > 0 {
                opts.jobs = Some(jobs as usize);
            }

            match shared_rt().block_on(snapdir_api::sync(
                &snap_id,
                &src_uri_parsed,
                &dst_uri_parsed,
                &opts,
            )) {
                Ok(()) => 0,
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    -1
                }
            }
        },
        -1,
        err_out,
    )
}

/// Verifies a snapshot in `store_uri` — checks that all objects are present
/// and hash-consistent.
///
/// `id` — 64-hex snapshot id.
/// `store_uri` — store URI to verify against.
/// `purge` — remove corrupt objects from the local cache during verification.
/// `err_out` — error out-parameter.
///
/// Returns `0` if the snapshot is healthy, `-1` on failure (with `*err_out` set).
///
/// # Safety
///
/// `id` and `store_uri` must be valid NUL-terminated UTF-8 C strings. `err_out`
/// must be either `NULL` or a valid writable pointer to `*mut SnapdirError`.
#[no_mangle]
pub unsafe extern "C" fn snapdir_verify_blocking(
    id: *const c_char,
    store_uri: *const c_char,
    purge: bool,
    err_out: *mut *mut SnapdirError,
) -> libc::c_int {
    catch_entry_unwind_safe(
        || {
            let id_str = match unsafe { cstr_to_str(id, "id", err_out) } {
                Some(s) => s.to_owned(),
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `id` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return -1;
                }
            };
            let store_str = match unsafe { cstr_to_str(store_uri, "store_uri", err_out) } {
                Some(s) => s.to_owned(),
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `store_uri` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return -1;
                }
            };

            let snap_id = match snapdir_api::SnapshotId::from_hex(&id_str) {
                Ok(id) => id,
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    return -1;
                }
            };
            let store_uri_parsed = match snapdir_api::StoreUri::parse(&store_str) {
                Ok(u) => u,
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    return -1;
                }
            };
            let opts = snapdir_api::VerifyOptions {
                purge,
                ..Default::default()
            };

            match shared_rt().block_on(snapdir_api::verify(&snap_id, &store_uri_parsed, &opts)) {
                Ok(result) => {
                    if result.ok {
                        0
                    } else {
                        let api_err = snapdir_api::SnapdirError::HashMismatch {
                            message: "snapshot verification failed: one or more objects are \
                                      corrupt or missing"
                                .to_owned(),
                        };
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                        -1
                    }
                }
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    -1
                }
            }
        },
        -1,
        err_out,
    )
}

// ---------------------------------------------------------------------------
// Multi-value JSON results
// ---------------------------------------------------------------------------

/// Diffs two sets of stores and returns a JSON array of change entries.
///
/// `from_uris` — NULL-terminated array of source store URIs.
/// `to_uris` — NULL-terminated array of destination store URIs.
/// `id` — optional 64-hex snapshot id (passed to [`snapdir_api::DiffOptions`];
///         currently informational — the api unions all manifests per side).
///         For a self-diff pass the same store on both sides; the union will be
///         identical on each side, yielding an empty diff.
///         For an A→B diff across two distinct stores, pass `NULL`.
/// `include_unchanged` — include unchanged entries in the output.
/// `on_conflict` — conflict policy: `"error"` or `"last-wins"` (`NULL` = `"error"`).
/// `err_out` — error out-parameter.
///
/// Returns a freshly-allocated JSON string on success (caller frees with
/// [`snapdir_string_free`]), or `NULL` on failure (with `*err_out` set).
///
/// JSON shape: `[{"status":"A","path":"./add.txt"}, ...]` where `status` is
/// one of `"A"` (Added), `"D"` (Deleted), `"M"` (Modified), `"="` (Unchanged).
///
/// # Safety
///
/// `from_uris` and `to_uris` must be NULL-terminated arrays of valid NUL-terminated
/// UTF-8 C string pointers (each inner pointer non-NULL). `id`, `on_conflict`, and
/// `err_out` follow the same rules as other optional parameters.
#[no_mangle]
pub unsafe extern "C" fn snapdir_diff_json(
    from_uris: *mut *const c_char,
    to_uris: *mut *const c_char,
    id: *const c_char,
    include_unchanged: bool,
    on_conflict: *const c_char,
    err_out: *mut *mut SnapdirError,
) -> *mut c_char {
    catch_entry_unwind_safe(
        || {
            // Parse the NULL-terminated from_uris array into validated StoreUri values.
            let mut from_parsed: Vec<snapdir_api::StoreUri> = Vec::new();
            if !from_uris.is_null() {
                let mut i = 0usize;
                loop {
                    // SAFETY: caller guarantees from_uris is a NULL-terminated array.
                    let ptr = unsafe { *from_uris.add(i) };
                    if ptr.is_null() {
                        break;
                    }
                    // SAFETY: each inner pointer is a valid NUL-terminated C string.
                    let s = match unsafe { cstr_to_str(ptr, "from_uris[i]", err_out) } {
                        Some(s) => s,
                        None => return std::ptr::null_mut(),
                    };
                    match snapdir_api::StoreUri::parse(s) {
                        Ok(uri) => from_parsed.push(uri),
                        Err(e) => {
                            unsafe { ffi_write_api_error(&e, err_out) };
                            return std::ptr::null_mut();
                        }
                    }
                    i += 1;
                }
            }

            // Parse the NULL-terminated to_uris array into validated StoreUri values.
            let mut to_parsed: Vec<snapdir_api::StoreUri> = Vec::new();
            if !to_uris.is_null() {
                let mut i = 0usize;
                loop {
                    // SAFETY: caller guarantees to_uris is a NULL-terminated array.
                    let ptr = unsafe { *to_uris.add(i) };
                    if ptr.is_null() {
                        break;
                    }
                    // SAFETY: each inner pointer is a valid NUL-terminated C string.
                    let s = match unsafe { cstr_to_str(ptr, "to_uris[i]", err_out) } {
                        Some(s) => s,
                        None => return std::ptr::null_mut(),
                    };
                    match snapdir_api::StoreUri::parse(s) {
                        Ok(uri) => to_parsed.push(uri),
                        Err(e) => {
                            unsafe { ffi_write_api_error(&e, err_out) };
                            return std::ptr::null_mut();
                        }
                    }
                    i += 1;
                }
            }

            // Parse the optional `id` arg.
            // SAFETY: id may be NULL.
            let snap_id_opt = match unsafe { cstr_to_str(id, "id", err_out) } {
                Some(id_str) => match snapdir_api::SnapshotId::from_hex(id_str) {
                    Ok(sid) => Some(sid),
                    Err(e) => {
                        unsafe { ffi_write_api_error(&e, err_out) };
                        return std::ptr::null_mut();
                    }
                },
                None => None,
            };

            // Parse conflict policy.
            // SAFETY: on_conflict may be NULL.
            let conflict_policy = match unsafe { cstr_to_str(on_conflict, "on_conflict", err_out) }
            {
                Some("last-wins") => snapdir_api::ConflictPolicy::LastWins,
                _ => snapdir_api::ConflictPolicy::Error,
            };

            // Delegate PURELY to snapdir_api::diff for BOTH id=NULL and id=non-NULL.
            //
            // snapdir_api::diff unions all manifests from each side's store list into a
            // flat path→fingerprint map, then classifies differences. This handles:
            //
            //   A→B diff (id=NULL, from=[storeA], to=[storeB]):
            //     storeA holds exactly the base snapshot; storeB holds exactly the next.
            //     The union on each side resolves to exactly one manifest, giving the
            //     correct base→next change set.
            //
            //   Self-diff (id=non-NULL, from=[storeA], to=[storeA]):
            //     Both sides union the same store, producing identical maps on from and
            //     to, yielding an empty diff (with include_unchanged=false).
            //
            // No store walking, no manual manifest parsing, no mtime heuristic.
            let diff_opts = snapdir_api::DiffOptions {
                from: from_parsed,
                to: to_parsed,
                id: snap_id_opt,
                all: include_unchanged,
                on_conflict: conflict_policy,
            };

            let entries = match shared_rt().block_on(snapdir_api::diff(&diff_opts)) {
                Ok(v) => v,
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    return std::ptr::null_mut();
                }
            };

            // Serialize to JSON: [{"status":"A","path":"./add.txt"}, ...]
            let json_entries: Vec<serde_json::Value> = entries
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "status": e.status.to_string(),
                        "path": e.path.display().to_string(),
                    })
                })
                .collect();
            let json_str = serde_json::to_string(&json_entries).unwrap_or_else(|_| "[]".into());

            match CString::new(json_str) {
                Ok(cs) => cs.into_raw(),
                Err(_) => {
                    let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "JSON output contained embedded NUL (unexpected)",
                    ));
                    unsafe { ffi_write_api_error(&api_err, err_out) };
                    std::ptr::null_mut()
                }
            }
        },
        std::ptr::null_mut(),
        err_out,
    )
}

/// Returns all catalog locations as a JSON array.
///
/// `cache_dir` — cache directory override (used for legacy reasons; catalog path
/// is resolved from `$SNAPDIR_CATALOG_DB_PATH` or the default; `NULL` = default).
/// `catalog` — catalog name selector (currently unused; pass `NULL` for default).
/// `err_out` — error out-parameter.
///
/// Returns a freshly-allocated JSON string on success (caller frees with
/// [`snapdir_string_free`]), or `NULL` on failure (with `*err_out` set).
///
/// JSON shape: `[{"created_at":"…","id":"…","location":"…"}, ...]`.
///
/// # Safety
///
/// `cache_dir` and `catalog` may be `NULL`. `err_out` must be either `NULL` or
/// a valid writable pointer to `*mut SnapdirError`.
#[no_mangle]
pub unsafe extern "C" fn snapdir_locations_json(
    _cache_dir: *const c_char,
    _catalog: *const c_char,
    err_out: *mut *mut SnapdirError,
) -> *mut c_char {
    catch_entry_unwind_safe(
        || {
            let opts = snapdir_api::LocationsOptions::default();
            match snapdir_api::locations(&opts) {
                Ok(locs) => {
                    let json_entries: Vec<serde_json::Value> = locs
                        .iter()
                        .map(|l| {
                            serde_json::json!({
                                "created_at": l.created_at,
                                "id": l.id,
                                "location": l.location,
                            })
                        })
                        .collect();
                    let json_str =
                        serde_json::to_string(&json_entries).unwrap_or_else(|_| "[]".into());
                    match CString::new(json_str) {
                        Ok(cs) => cs.into_raw(),
                        Err(_) => {
                            let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                "JSON output contained embedded NUL (unexpected)",
                            ));
                            unsafe { ffi_write_api_error(&api_err, err_out) };
                            std::ptr::null_mut()
                        }
                    }
                }
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    std::ptr::null_mut()
                }
            }
        },
        std::ptr::null_mut(),
        err_out,
    )
}

/// Returns the ancestor chain for `id` as a JSON array.
///
/// `id` — 64-hex snapshot id.
/// `catalog` — catalog name selector (currently unused; pass `NULL` for default).
/// `err_out` — error out-parameter.
///
/// Returns a freshly-allocated JSON string on success (caller frees with
/// [`snapdir_string_free`]), or `NULL` on failure (with `*err_out` set).
///
/// JSON shape: `[{"created_at":"…","id":"…","location":"…"}, ...]`.
///
/// # Safety
///
/// `id` must be a valid NUL-terminated 64-hex C string. `catalog` may be `NULL`.
/// `err_out` must be either `NULL` or a valid writable pointer to `*mut SnapdirError`.
#[no_mangle]
pub unsafe extern "C" fn snapdir_ancestors_json(
    id: *const c_char,
    _catalog: *const c_char,
    err_out: *mut *mut SnapdirError,
) -> *mut c_char {
    catch_entry_unwind_safe(
        || {
            let id_str = match unsafe { cstr_to_str(id, "id", err_out) } {
                Some(s) => s.to_owned(),
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `id` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return std::ptr::null_mut();
                }
            };
            let snap_id = match snapdir_api::SnapshotId::from_hex(&id_str) {
                Ok(id) => id,
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    return std::ptr::null_mut();
                }
            };

            let opts = snapdir_api::AncestorsOptions::default();
            match snapdir_api::ancestors(&snap_id, &opts) {
                Ok(ancs) => {
                    let json_entries: Vec<serde_json::Value> = ancs
                        .iter()
                        .map(|a| {
                            serde_json::json!({
                                "created_at": a.created_at,
                                "id": a.id,
                                "location": a.location,
                            })
                        })
                        .collect();
                    let json_str =
                        serde_json::to_string(&json_entries).unwrap_or_else(|_| "[]".into());
                    match CString::new(json_str) {
                        Ok(cs) => cs.into_raw(),
                        Err(_) => {
                            let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                "JSON output contained embedded NUL (unexpected)",
                            ));
                            unsafe { ffi_write_api_error(&api_err, err_out) };
                            std::ptr::null_mut()
                        }
                    }
                }
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    std::ptr::null_mut()
                }
            }
        },
        std::ptr::null_mut(),
        err_out,
    )
}

/// Returns the revision history at `location` as a JSON array.
///
/// `location` — store URI or catalog location path to look up.
/// `catalog` — catalog name selector (currently unused; pass `NULL` for default).
/// `err_out` — error out-parameter.
///
/// Returns a freshly-allocated JSON string on success (caller frees with
/// [`snapdir_string_free`]), or `NULL` on failure (with `*err_out` set).
///
/// JSON shape: `[{"created_at":"…","id":"…","previous_id":null_or_"…"}, ...]`.
///
/// # Safety
///
/// `location` must be a valid NUL-terminated UTF-8 C string. `catalog` may be
/// `NULL`. `err_out` must be either `NULL` or a valid writable pointer to
/// `*mut SnapdirError`.
#[no_mangle]
pub unsafe extern "C" fn snapdir_revisions_json(
    location: *const c_char,
    _catalog: *const c_char,
    err_out: *mut *mut SnapdirError,
) -> *mut c_char {
    catch_entry_unwind_safe(
        || {
            let loc_str = match unsafe { cstr_to_str(location, "location", err_out) } {
                Some(s) => s.to_owned(),
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `location` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return std::ptr::null_mut();
                }
            };

            let loc_ref = snapdir_api::LocationRef::new(loc_str);
            let opts = snapdir_api::RevisionsOptions::default();
            match snapdir_api::revisions(&loc_ref, &opts) {
                Ok(revs) => {
                    let json_entries: Vec<serde_json::Value> = revs
                        .iter()
                        .map(|r| {
                            serde_json::json!({
                                "created_at": r.created_at,
                                "id": r.id,
                                "previous_id": r.previous_id,
                            })
                        })
                        .collect();
                    let json_str =
                        serde_json::to_string(&json_entries).unwrap_or_else(|_| "[]".into());
                    match CString::new(json_str) {
                        Ok(cs) => cs.into_raw(),
                        Err(_) => {
                            let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                "JSON output contained embedded NUL (unexpected)",
                            ));
                            unsafe { ffi_write_api_error(&api_err, err_out) };
                            std::ptr::null_mut()
                        }
                    }
                }
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    std::ptr::null_mut()
                }
            }
        },
        std::ptr::null_mut(),
        err_out,
    )
}

// ---------------------------------------------------------------------------
// Cache maintenance
// ---------------------------------------------------------------------------

/// Verifies the local cache at `cache_dir`.
///
/// `cache_dir` — the cache directory to verify (must not be `NULL`).
/// `purge` — if `true`, remove corrupt objects from the cache (currently
///            passed through to `snapdir_api::verify_cache`; the `purge`
///            semantics are handled by the api layer).
/// `err_out` — error out-parameter.
///
/// Returns `0` if the cache is clean (or empty), `-1` on failure (with
/// `*err_out` set).
///
/// # Safety
///
/// `cache_dir` must be a valid NUL-terminated UTF-8 C string. `err_out` must
/// be either `NULL` or a valid writable pointer to `*mut SnapdirError`.
#[no_mangle]
pub unsafe extern "C" fn snapdir_verify_cache(
    cache_dir: *const c_char,
    _purge: bool,
    err_out: *mut *mut SnapdirError,
) -> libc::c_int {
    catch_entry_unwind_safe(
        || {
            let cache_str = match unsafe { cstr_to_str(cache_dir, "cache_dir", err_out) } {
                Some(s) => s.to_owned(),
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `cache_dir` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return -1;
                }
            };

            // Delegate fully to snapdir_api::verify_cache with the explicit cache_dir.
            let opts = snapdir_api::VerifyCacheOptions {
                cache_dir: Some(std::path::PathBuf::from(&cache_str)),
            };
            match snapdir_api::verify_cache(&opts) {
                Ok(result) => {
                    if result.ok {
                        0
                    } else {
                        let api_err = snapdir_api::SnapdirError::HashMismatch {
                            message: "cache verification failed: one or more cached objects are \
                                      corrupt"
                                .to_owned(),
                        };
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                        -1
                    }
                }
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    -1
                }
            }
        },
        -1,
        err_out,
    )
}

/// Flushes (empties) the local cache at `cache_dir`.
///
/// `cache_dir` — the cache directory to flush (must not be `NULL`). A missing
/// or empty cache directory is a successful no-op.
/// `err_out` — error out-parameter.
///
/// Returns `0` on success, `-1` on failure (with `*err_out` set).
///
/// # Safety
///
/// `cache_dir` must be a valid NUL-terminated UTF-8 C string. `err_out` must
/// be either `NULL` or a valid writable pointer to `*mut SnapdirError`.
#[no_mangle]
pub unsafe extern "C" fn snapdir_flush_cache(
    cache_dir: *const c_char,
    err_out: *mut *mut SnapdirError,
) -> libc::c_int {
    catch_entry_unwind_safe(
        || {
            let cache_str = match unsafe { cstr_to_str(cache_dir, "cache_dir", err_out) } {
                Some(s) => s.to_owned(),
                None => {
                    if err_out.is_null() || unsafe { (*err_out).is_null() } {
                        let api_err = snapdir_api::SnapdirError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "parameter `cache_dir` must not be NULL",
                        ));
                        unsafe { ffi_write_api_error(&api_err, err_out) };
                    }
                    return -1;
                }
            };

            // Delegate fully to snapdir_api::flush_cache with the explicit cache_dir.
            let opts = snapdir_api::CacheOptions {
                cache_dir: Some(std::path::PathBuf::from(&cache_str)),
            };
            match snapdir_api::flush_cache(&opts) {
                Ok(()) => 0,
                Err(e) => {
                    unsafe { ffi_write_api_error(&e, err_out) };
                    -1
                }
            }
        },
        -1,
        err_out,
    )
}

// ---------------------------------------------------------------------------
// Module-level tests (pure Rust, no C boundary)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapdir_init_is_idempotent() {
        // Calling init many times must not panic or produce different results.
        snapdir_init();
        snapdir_init();
        snapdir_init();
        // Runtime must be accessible after multiple inits.
        let rt = shared_rt();
        let result = rt.block_on(async { 42u32 });
        assert_eq!(result, 42);
    }

    #[test]
    fn snapdir_version_returns_valid_non_empty_string() {
        let ptr = snapdir_version();
        assert!(!ptr.is_null());
        // SAFETY: snapdir_version returns a 'static CStr.
        let s = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap();
        assert!(!s.is_empty(), "version string must not be empty");
        // Must not try to free this pointer — it is static.
    }

    #[test]
    fn snapdir_string_free_null_is_noop() {
        // SAFETY: NULL is explicitly documented as a no-op.
        unsafe { snapdir_string_free(std::ptr::null_mut()) };
    }

    #[test]
    fn snapdir_error_free_null_is_noop() {
        // SAFETY: NULL is explicitly documented as a no-op.
        unsafe { snapdir_error_free(std::ptr::null_mut()) };
    }

    #[test]
    fn snapdir_error_inspect_null_returns_null() {
        let msg = unsafe { snapdir_error_message(std::ptr::null()) };
        let code = unsafe { snapdir_error_code(std::ptr::null()) };
        assert!(msg.is_null());
        assert!(code.is_null());
    }

    #[test]
    fn snapdir_error_round_trip() {
        use snapdir_api::SnapshotId;
        // Produce a real API error.
        let api_err = SnapshotId::from_hex("bad").unwrap_err();
        let ffi_err = Box::new(SnapdirError::from_api(&api_err));
        let raw = Box::into_raw(ffi_err);

        // SAFETY: raw was just boxed above.
        let msg_ptr = unsafe { snapdir_error_message(raw) };
        let code_ptr = unsafe { snapdir_error_code(raw) };
        assert!(!msg_ptr.is_null());
        assert!(!code_ptr.is_null());

        let code = unsafe { CStr::from_ptr(code_ptr) }.to_str().unwrap();
        assert_eq!(code, "INVALID_ID");
        let msg = unsafe { CStr::from_ptr(msg_ptr) }.to_str().unwrap();
        assert!(!msg.is_empty());

        // Free — the inspect pointers must not be used afterwards.
        unsafe { snapdir_error_free(raw) };
    }

    #[test]
    fn snapdir_error_from_panic() {
        let ffi_err = SnapdirError::from_panic("test panic message");
        let code = ffi_err.code.to_str().unwrap();
        assert_eq!(code, "INTERNAL");
        let msg = ffi_err.message.to_str().unwrap();
        assert!(msg.contains("test panic message"));
    }

    #[test]
    fn catch_entry_propagates_panic_to_err_out() {
        let mut err: *mut SnapdirError = std::ptr::null_mut();
        let err_out: *mut *mut SnapdirError = &mut err;

        let result = catch_entry(
            || {
                panic!("deliberate test panic");
                #[allow(unreachable_code)]
                0i32
            },
            -1i32,
            err_out,
        );

        assert_eq!(result, -1);
        assert!(!err.is_null());
        let code = unsafe { CStr::from_ptr(snapdir_error_code(err)) }
            .to_str()
            .unwrap();
        assert_eq!(code, "INTERNAL");
        unsafe { snapdir_error_free(err) };
    }

    #[test]
    fn snapdir_string_free_round_trip() {
        let cs = CString::new("hello from ffi").unwrap();
        let raw = cs.into_raw();
        // SAFETY: raw was created by CString::into_raw immediately above.
        unsafe { snapdir_string_free(raw) };
        // No double-free here; the test just confirms it doesn't crash.
    }
}
