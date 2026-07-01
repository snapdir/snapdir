//! `snapdir-api` — the stable, async facade every language binding consumes.
//!
//! This crate wraps `snapdir-core`, `snapdir-stores`, and
//! `snapdir-catalog` behind a single, documented, semver-stable surface.
//! `snapdir-core` stays pure (no tokio); the async distribution functions live
//! here and `spawn_blocking` over the sync stores runtime.
//!
//! # Error handling
//!
//! All public functions return `Result<T, SnapdirError>` (see [`SnapdirError`]).
//! No `anyhow` leaks into the public surface.

#![deny(missing_docs)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use snapdir_core::store::StoreError;

// ---------------------------------------------------------------------------
// Public type alias
// ---------------------------------------------------------------------------

/// Convenience alias for results returned by this crate.
pub type Result<T> = std::result::Result<T, SnapdirError>;

// ---------------------------------------------------------------------------
// SnapdirError
// ---------------------------------------------------------------------------

/// All errors that can surface from the `snapdir-api` public surface.
///
/// `code()` returns a stable, `SCREAMING_SNAKE_CASE` string that every language
/// binding maps to its native error subtype. The codes are a frozen contract:
/// they will not be renamed or removed in a semver-compatible release.
///
/// No `anyhow` leaks here. Every variant is `Send + Sync + 'static`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SnapdirError {
    /// An underlying I/O failure.
    #[error("I/O error: {0}")]
    Io(
        #[from]
        #[source]
        std::io::Error,
    ),

    /// A content-hash mismatch (object or manifest integrity failure).
    #[error("hash mismatch: {message}")]
    HashMismatch {
        /// Human-readable description of the mismatch.
        message: String,
    },

    /// A store-level error (missing object/manifest, backend failure, etc.).
    #[error("store error: {0}")]
    StoreError(#[source] Box<StoreError>),

    /// The snapshot or object is currently being written (in-flux).
    #[error("snapshot is in flux: {message}")]
    InFlux {
        /// Human-readable description.
        message: String,
    },

    /// A catalog operation failed.
    #[error("catalog error: {message}")]
    CatalogError {
        /// Human-readable description.
        message: String,
    },

    /// The provided snapshot ID is malformed (wrong length or non-hex chars).
    #[error("invalid snapshot id: {message}")]
    InvalidId {
        /// Human-readable description.
        message: String,
    },

    /// The provided store URI uses an unknown or malformed scheme.
    #[error("invalid store URI: {message}")]
    InvalidStore {
        /// Human-readable description.
        message: String,
    },

    /// A conflict between concurrent snapshots or catalog entries.
    #[error("conflict: {message}")]
    Conflict {
        /// Human-readable description.
        message: String,
    },
}

impl SnapdirError {
    /// Returns the stable, `SCREAMING_SNAKE_CASE` code for this error variant.
    ///
    /// Language bindings map these codes to their native error subtypes. The
    /// set of codes is a frozen contract and will not change in a
    /// semver-compatible release.
    ///
    /// ```
    /// use snapdir_api::{SnapdirError, SnapshotId, StoreUri};
    ///
    /// let e = SnapshotId::from_hex("bad").unwrap_err();
    /// assert_eq!(e.code(), "INVALID_ID");
    ///
    /// let e = StoreUri::parse("nope://x").unwrap_err();
    /// assert_eq!(e.code(), "INVALID_STORE");
    /// ```
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            SnapdirError::Io(_) => "IO_ERROR",
            SnapdirError::HashMismatch { .. } => "HASH_MISMATCH",
            SnapdirError::StoreError(_) => "STORE_ERROR",
            SnapdirError::InFlux { .. } => "IN_FLUX",
            SnapdirError::CatalogError { .. } => "CATALOG_ERROR",
            SnapdirError::InvalidId { .. } => "INVALID_ID",
            SnapdirError::InvalidStore { .. } => "INVALID_STORE",
            SnapdirError::Conflict { .. } => "CONFLICT",
        }
    }
}

impl From<StoreError> for SnapdirError {
    fn from(e: StoreError) -> Self {
        // Integrity mismatches in the store map to HashMismatch.
        if let StoreError::Integrity {
            ref address,
            ref expected,
            ref actual,
        } = e
        {
            return SnapdirError::HashMismatch {
                message: format!(
                    "integrity check failed for {address}: expected {expected}, got {actual}"
                ),
            };
        }
        // IO errors inside the store surface as SnapdirError::Io so the
        // source() chain is preserved (the StoreError::Io wraps an io::Error
        // via #[from]).
        if let StoreError::Io(ref io) = e {
            return SnapdirError::Io(std::io::Error::new(io.kind(), io.to_string()));
        }
        SnapdirError::StoreError(Box::new(e))
    }
}

// ---------------------------------------------------------------------------
// SnapshotId
// ---------------------------------------------------------------------------

/// A 32-byte snapshot identifier.
///
/// `Display` / `FromStr` use 64-character lowercase hex. `from_hex` is
/// case-insensitive on input; `to_hex()` and `Display` always emit lowercase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SnapshotId([u8; 32]);

impl SnapshotId {
    /// Parses a 64-character hex string (case-insensitive) into a `SnapshotId`.
    ///
    /// Returns `Err(SnapdirError::InvalidId)` on wrong length or non-hex chars.
    ///
    /// ```
    /// use snapdir_api::SnapshotId;
    ///
    /// let id = SnapshotId::from_hex(&"a".repeat(64)).unwrap();
    /// assert_eq!(id.to_hex(), "a".repeat(64));
    /// ```
    pub fn from_hex(s: &str) -> Result<Self> {
        if s.len() != 64 {
            return Err(SnapdirError::InvalidId {
                message: format!(
                    "snapshot id must be exactly 64 hex characters, got {} characters",
                    s.len()
                ),
            });
        }
        let mut bytes = [0u8; 32];
        for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
            let hi = hex_nibble(chunk[0]).map_err(|()| SnapdirError::InvalidId {
                message: format!("non-hex character in snapshot id: {s:?}"),
            })?;
            let lo = hex_nibble(chunk[1]).map_err(|()| SnapdirError::InvalidId {
                message: format!("non-hex character in snapshot id: {s:?}"),
            })?;
            bytes[i] = (hi << 4) | lo;
        }
        Ok(Self(bytes))
    }

    /// Returns the 64-character lowercase hex representation.
    ///
    /// ```
    /// use snapdir_api::SnapshotId;
    ///
    /// let id = SnapshotId::from_hex(&"ff".repeat(32)).unwrap();
    /// assert_eq!(id.to_hex(), "ff".repeat(32));
    /// ```
    #[must_use]
    pub fn to_hex(&self) -> String {
        self.0.iter().fold(String::with_capacity(64), |mut s, b| {
            use std::fmt::Write;
            write!(s, "{b:02x}").unwrap();
            s
        })
    }

    /// Returns the raw bytes of this snapshot identifier.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Display for SnapshotId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl std::str::FromStr for SnapshotId {
    type Err = SnapdirError;

    fn from_str(s: &str) -> Result<Self> {
        Self::from_hex(s)
    }
}

/// Converts a single ASCII hex nibble byte to its numeric value.
fn hex_nibble(b: u8) -> std::result::Result<u8, ()> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(()),
    }
}

// ---------------------------------------------------------------------------
// StoreUri
// ---------------------------------------------------------------------------

/// A scheme-validated store URI.
///
/// Accepted schemes: `file`, `s3`, `gs`, `b2`, `ssh`, `sftp`.
/// Any other scheme or malformed input returns `Err(SnapdirError::InvalidStore)`.
///
/// `Display` round-trips the original URI string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreUri {
    raw: String,
    scheme: String,
}

/// Schemes accepted by `StoreUri::parse`.
const ACCEPTED_SCHEMES: &[&str] = &["file", "s3", "gs", "b2", "ssh", "sftp"];

impl StoreUri {
    /// Parses and validates a store URI.
    ///
    /// Returns `Err(SnapdirError::InvalidStore)` for unknown or malformed
    /// schemes.
    ///
    /// ```
    /// use snapdir_api::StoreUri;
    ///
    /// let uri = StoreUri::parse("file:///tmp/store").unwrap();
    /// assert_eq!(uri.scheme(), "file");
    ///
    /// assert!(StoreUri::parse("nope://x").is_err());
    /// ```
    pub fn parse(s: &str) -> Result<Self> {
        let scheme = extract_scheme(s)?;
        if !ACCEPTED_SCHEMES.contains(&scheme) {
            return Err(SnapdirError::InvalidStore {
                message: format!(
                    "unknown store scheme {scheme:?}: accepted schemes are {}",
                    ACCEPTED_SCHEMES.join(", ")
                ),
            });
        }
        Ok(Self {
            raw: s.to_owned(),
            scheme: scheme.to_owned(),
        })
    }

    /// Returns the scheme of this URI (e.g. `"file"`, `"s3"`, `"gs"`).
    #[must_use]
    pub fn scheme(&self) -> &str {
        &self.scheme
    }
}

impl std::fmt::Display for StoreUri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.raw)
    }
}

/// Extracts the scheme (text before the first `:`), or returns `InvalidStore`.
///
/// Also validates that the scheme is followed by `://` (the authority
/// separator), so bare paths such as `file:/missing-slashes` are rejected.
fn extract_scheme(uri: &str) -> Result<&str> {
    let colon = uri.find(':').ok_or_else(|| SnapdirError::InvalidStore {
        message: format!("no scheme found in store URI: {uri:?}"),
    })?;
    let scheme = &uri[..colon];
    if scheme.is_empty() {
        return Err(SnapdirError::InvalidStore {
            message: format!("empty scheme in store URI: {uri:?}"),
        });
    }
    // Scheme must be lowercase alphanumeric (mirrors router's oracle constraint).
    if !scheme
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return Err(SnapdirError::InvalidStore {
            message: format!("invalid scheme {scheme:?} in store URI: {uri:?}"),
        });
    }
    // Require "://" — a bare single-slash path like "file:/foo" is rejected.
    let after_colon = &uri[colon..];
    if !after_colon.starts_with("://") {
        return Err(SnapdirError::InvalidStore {
            message: format!("store URI must use '://' separator: {uri:?}"),
        });
    }
    Ok(scheme)
}

// ---------------------------------------------------------------------------
// PushSource
// ---------------------------------------------------------------------------

/// The source for a `push` operation: either a filesystem path or a
/// previously staged snapshot id.
///
/// The lifetime `'a` ties the borrowed reference back to the caller's storage
/// so no heap allocation is needed for the common case.
#[derive(Debug)]
pub enum PushSource<'a> {
    /// A filesystem path to push from.
    Path(&'a std::path::Path),
    /// A snapshot id that has already been staged in the local cache.
    StagedId(&'a SnapshotId),
}

// ---------------------------------------------------------------------------
// DiffStatus / DiffEntry
// ---------------------------------------------------------------------------

/// The change status of a single entry in a diff result.
///
/// `Display` renders a single-character glyph: `A` (Added), `D` (Deleted),
/// `M` (Modified), `=` (Unchanged). These glyphs are frozen — language
/// bindings parse them by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffStatus {
    /// The entry was added in the newer snapshot.
    Added,
    /// The entry was deleted relative to the older snapshot.
    Deleted,
    /// The entry exists in both snapshots but its content or metadata changed.
    Modified,
    /// The entry is byte-for-byte identical in both snapshots.
    Unchanged,
}

impl std::fmt::Display for DiffStatus {
    /// Renders the single-character glyph: `A`, `D`, `M`, or `=`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            DiffStatus::Added => "A",
            DiffStatus::Deleted => "D",
            DiffStatus::Modified => "M",
            DiffStatus::Unchanged => "=",
        };
        f.write_str(s)
    }
}

/// A single entry in a diff result.
///
/// `status` indicates what changed; `path` is the entry's path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEntry {
    /// Whether the entry was added, deleted, modified, or unchanged.
    pub status: DiffStatus,
    /// The entry's path.
    pub path: PathBuf,
}

// ---------------------------------------------------------------------------
// New result types
// ---------------------------------------------------------------------------

/// Result of a [`verify`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyResult {
    /// `true` when every object in the snapshot verified clean.
    pub ok: bool,
}

/// Result of a [`verify_cache`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyCacheResult {
    /// `true` when every cached object verified clean (or the cache is empty).
    pub ok: bool,
}

/// The effective configuration as resolved from env vars and CLI defaults.
///
/// `entries` is a list of `(key, value)` pairs in resolution order; an
/// empty list means the factory defaults apply for every key.
#[derive(Debug, Clone, Default)]
pub struct EffectiveConfig {
    /// Resolved configuration entries as `(key, value)` pairs.
    pub entries: Vec<(String, String)>,
}

/// A catalog location reference — an opaque string identifying a store
/// location in the catalog (e.g. a store URI or a catalog path).
///
/// Used as the input to [`revisions`] to ask for the revision history at a
/// particular location.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct LocationRef(String);

impl LocationRef {
    /// Creates a new `LocationRef` from any string-like value.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Returns the underlying string representation of this location reference.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A catalog location record — a store URI where a snapshot was recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// ISO-8601 timestamp when this location was recorded.
    pub created_at: String,
    /// The snapshot id recorded at this location.
    pub id: String,
    /// The store URI for this location.
    pub location: String,
}

/// A catalog ancestor record — a previous snapshot in the lineage chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ancestor {
    /// ISO-8601 timestamp of the ancestor entry.
    pub created_at: String,
    /// The ancestor snapshot id (`previous_id` in catalog terms).
    pub id: String,
    /// The store URI where the ancestor was located.
    pub location: String,
}

/// A catalog revision record — one entry in a location's revision history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    /// ISO-8601 timestamp of the revision.
    pub created_at: String,
    /// The snapshot id of this revision.
    pub id: String,
    /// The previous snapshot id in this revision chain, if any.
    pub previous_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Re-exports from core
// ---------------------------------------------------------------------------

pub use snapdir_core::manifest::PathType;

// ---------------------------------------------------------------------------
// Manifest / ManifestEntry (API-own typed wrapper types, per §3 correction)
// ---------------------------------------------------------------------------

/// A single entry in a `Manifest`.
///
/// Converts from the core [`snapdir_core::manifest::ManifestEntry`] (which
/// stores permissions/checksum as `String`s). Here they are typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    /// Whether this entry is a file or directory.
    pub path_type: PathType,
    /// Octal permission bits (e.g. `0o700`).
    pub permissions: u32,
    /// 32-byte BLAKE3 content checksum.
    pub checksum: [u8; 32],
    /// Content size in bytes.
    pub size: u64,
    /// The entry path.
    pub path: PathBuf,
}

/// An ordered collection of manifest entries plus the raw manifest text.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// The typed entries.
    pub entries: Vec<ManifestEntry>,
    /// The raw manifest text (the `Display` of the core `Manifest`).
    pub raw: String,
}

impl Manifest {
    /// Converts a core [`snapdir_core::manifest::Manifest`] into the API
    /// typed wrapper.
    fn from_core(core: &snapdir_core::manifest::Manifest) -> Self {
        let raw = core.to_string();
        let entries = core
            .entries()
            .iter()
            .map(|e| {
                let permissions = u32::from_str_radix(&e.permissions, 8).unwrap_or(0);
                let mut checksum = [0u8; 32];
                // Core stores checksums as 64-char lowercase hex; decode it.
                if e.checksum.len() == 64 {
                    for (i, chunk) in e.checksum.as_bytes().chunks(2).enumerate() {
                        let hi = hex_nibble(chunk[0]).unwrap_or(0);
                        let lo = hex_nibble(chunk[1]).unwrap_or(0);
                        checksum[i] = (hi << 4) | lo;
                    }
                }
                ManifestEntry {
                    path_type: e.path_type,
                    permissions,
                    checksum,
                    size: e.size,
                    path: PathBuf::from(&e.path),
                }
            })
            .collect();
        Self { entries, raw }
    }
}

// ---------------------------------------------------------------------------
// Options enums
// ---------------------------------------------------------------------------

/// Which checksum binary (algorithm) to use for directory walks.
///
/// Mirrors the CLI's `--checksum-bin` flag. The default is `B3sum` (BLAKE3),
/// matching the CLI's effective default when `--checksum-bin` is unset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ChecksumBin {
    /// BLAKE3 (b3sum) — the default.
    #[default]
    B3sum,
    /// MD5 (md5sum).
    Md5sum,
    /// SHA-256 (sha256sum).
    Sha256sum,
}

/// How to select the catalog adapter for recording snapshot locations.
///
/// Mirrors the CLI's `--catalog` flag. When `Default`, the catalog adapter
/// chooses its own default (i.e., the `SNAPDIR_CATALOG` env var or the
/// adapter's built-in default). `None` suppresses catalog recording.
/// `Named(s)` selects the named adapter explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum CatalogOption {
    /// Use the catalog adapter's own default (no explicit `--catalog` flag).
    #[default]
    Default,
    /// Suppress catalog recording (`--catalog none` / no adapter).
    None,
    /// Use the named catalog adapter.
    Named(String),
}

/// How to resolve a path collision when computing a diff from multiple sources.
///
/// Mirrors the CLI's `--on-conflict` flag. The default is `Error`, meaning a
/// conflict (same path, differing content on one side) is a hard error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ConflictPolicy {
    /// Treat any intra-side path collision as a hard error (default).
    #[default]
    Error,
    /// Accept the last-seen value in the case of a collision.
    LastWins,
}

// ---------------------------------------------------------------------------
// Options structs
// ---------------------------------------------------------------------------

/// Options for the `manifest()` / `id()` sync functions.
///
/// All fields default to their CLI effective defaults (i.e., the values that
/// apply when the corresponding flag or env var is absent).
///
/// Note: this struct does NOT carry `#[non_exhaustive]` — the tests use struct
/// update syntax (`{ field: val, ..Default::default() }`) from integration-test
/// crates, which is blocked by `#[non_exhaustive]` on the defining crate's
/// structs in Rust (E0639). The additive-change stability contract is maintained
/// by the `m0-api-freeze` public-api baseline snapshot instead.
#[derive(Debug, Default, Clone)]
pub struct ManifestOptions {
    /// Paths to exclude (extended-regex `-E -v` patterns, same as `--exclude`).
    pub exclude: Vec<String>,
    /// Number of parallel file-hashing workers (`None` = auto/CPU-count).
    /// Mirrors `--walk-jobs` / `SNAPDIR_WALK_JOBS`.
    pub walk_jobs: Option<usize>,
    /// Emit absolute paths instead of `./`-relative paths. Mirrors `--absolute`.
    pub absolute: bool,
    /// Do not follow symbolic links. Mirrors `--no-follow`.
    pub no_follow: bool,
    /// Checksum algorithm. Mirrors `--checksum-bin` (default: `B3sum`).
    pub checksum_bin: ChecksumBin,
    /// Catalog adapter selection. Mirrors `--catalog` / `SNAPDIR_CATALOG`.
    pub catalog: CatalogOption,
    /// Override the local object-cache directory. Mirrors `--cache-dir` /
    /// `SNAPDIR_CACHE_DIR`.
    pub cache_dir: Option<PathBuf>,
}

/// Options for async transfer functions (`fetch`, `push`, `pull`, `sync`, etc.).
///
/// Every field defaults to `None` (or the equivalent empty/default value),
/// meaning the transfer engine will apply its own resolved defaults (e.g. 5
/// retries, 250 ms base back-off, 30 000 ms max back-off). Setting a field
/// here overrides the engine default for that run.
///
/// The `catalog` field defaults to [`CatalogOption::Default`] (not `None`),
/// matching the CLI's behavior when `--catalog` is absent.
///
/// Note: not `#[non_exhaustive]` — see `ManifestOptions` for the rationale.
#[derive(Debug, Default, Clone)]
pub struct TransferOptions {
    /// Explicit store URI. Mirrors `--store` / `SNAPDIR_STORE`.
    pub store: Option<StoreUri>,
    /// Shared objects-pool store URI. Mirrors `--objects-store` /
    /// `SNAPDIR_OBJECTS_STORE`.
    pub objects_store: Option<StoreUri>,
    /// Override the local object-cache directory. Mirrors `--cache-dir` /
    /// `SNAPDIR_CACHE_DIR`.
    pub cache_dir: Option<PathBuf>,
    /// Catalog adapter selection. Mirrors `--catalog` / `SNAPDIR_CATALOG`.
    pub catalog: CatalogOption,
    /// Max concurrent object transfers (`None` = auto/CPU-count). Mirrors
    /// `--jobs` / `-j` / `SNAPDIR_JOBS`.
    pub jobs: Option<usize>,
    /// Bandwidth limit (e.g. `"10M"`, `"512K"`). Mirrors `--limit-rate` /
    /// `SNAPDIR_LIMIT_RATE`.
    pub limit_rate: Option<String>,
    /// Adaptive concurrency politeness fraction in `(0.0, 1.0]`. `None` =
    /// full speed (opt-in). Mirrors `--adaptive` / `SNAPDIR_ADAPTIVE`.
    pub adaptive: Option<f64>,
    /// Adaptive concurrency ceiling. Mirrors `--max-jobs` / `SNAPDIR_MAX_JOBS`.
    pub max_jobs: Option<usize>,
    /// Total retry attempts per request including the first (`None` = engine
    /// default of 5). Mirrors `--max-retries`.
    pub max_retries: Option<u32>,
    /// Base back-off delay in milliseconds (`None` = engine default of 250).
    /// Mirrors `--retry-base-ms`.
    pub retry_base_ms: Option<u64>,
    /// Maximum back-off delay in milliseconds (`None` = engine default of
    /// 30 000). Mirrors `--retry-max-ms`.
    pub retry_max_ms: Option<u64>,
    /// Request-rate cap in req/s (`None` = per-backend default). Mirrors
    /// `--max-requests`.
    pub max_requests: Option<u64>,
}

/// Options for `checkout` / `pull`.
///
/// Embeds [`TransferOptions`] for all network/cache knobs. The bool flags
/// mirror the CLI's `--linked`, `--force`, `--keep`, `--dryrun`, `--delete`.
///
/// Note: not `#[non_exhaustive]` — see `ManifestOptions` for the rationale.
#[derive(Debug, Default, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct CheckoutOptions {
    /// Underlying transfer options (store, jobs, retries, …).
    pub transfer: TransferOptions,
    /// Use symlinks instead of copies (`--linked`).
    pub linked: bool,
    /// Force the operation even when the destination is dangerous (`--force`).
    pub force: bool,
    /// Keep the staging directory after the transfer (`--keep`).
    pub keep: bool,
    /// Dry-run: report what would change without making changes (`--dryrun`).
    pub dryrun: bool,
    /// Exact-mirror mode: delete destination files absent from the snapshot
    /// (`--delete`).
    pub delete: bool,
    /// Paths to exclude from deletion in `--delete` mode (`--exclude`).
    pub exclude: Vec<String>,
}

/// Options for the `diff()` async function.
///
/// `from` and `to` are repeatable store URI lists (UNIONED on each side);
/// leaving both empty diffs the local cache default. `on_conflict` defaults
/// to [`ConflictPolicy::Error`].
///
/// Note: not `#[non_exhaustive]` — see `ManifestOptions` for the rationale.
#[derive(Debug, Default, Clone)]
pub struct DiffOptions {
    /// Source side store URIs (repeatable, unioned). Mirrors `--from`.
    pub from: Vec<StoreUri>,
    /// Destination side store URIs (repeatable, unioned). Mirrors `--to`.
    pub to: Vec<StoreUri>,
    /// Specific snapshot ID to diff. Mirrors `--id`.
    pub id: Option<SnapshotId>,
    /// Include unchanged entries in the output. Mirrors `--all`.
    pub all: bool,
    /// How to handle an intra-side path collision. Mirrors `--on-conflict`.
    pub on_conflict: ConflictPolicy,
}

/// Options for the `stage()` sync function.
///
/// ## Fields
///
/// - `cache_dir` — override the local cache directory used when staging objects.
///   When `None` (the default), the standard cache (`$XDG_CACHE_HOME/snapdir` or
///   `$HOME/.cache/snapdir`) is used — identical to the pre-field behaviour.
/// - `keep` — when `true` (the default), the staged manifest and objects are
///   persisted in the cache so the snapshot can later be pushed via
///   `push(PushSource::StagedId(&id), …)`. When `false`, the cache write is
///   skipped and only the snapshot id is returned (useful for id-only queries).
///
/// Note: not `#[non_exhaustive]` — see `ManifestOptions` for the rationale.
#[derive(Debug, Clone)]
pub struct StageOptions {
    /// Override the local cache directory for this staging operation.
    ///
    /// `None` (the default) uses the standard cache (`$XDG_CACHE_HOME/snapdir`
    /// or `$HOME/.cache/snapdir`).
    pub cache_dir: Option<std::path::PathBuf>,
    /// Persist the staged manifest and objects in the cache (`true`, the default).
    ///
    /// Set to `false` to compute the snapshot id without writing to the cache.
    pub keep: bool,
}

impl Default for StageOptions {
    fn default() -> Self {
        Self {
            cache_dir: None,
            keep: true,
        }
    }
}

/// Options for the `verify()` async function.
///
/// Embeds [`TransferOptions`] for store/network knobs.
///
/// Note: not `#[non_exhaustive]` — see `ManifestOptions` for the rationale.
#[derive(Debug, Default, Clone)]
pub struct VerifyOptions {
    /// Remove corrupt objects from the local cache during verification.
    /// Mirrors `--purge`.
    pub purge: bool,
    /// Underlying transfer options (store, jobs, retries, …).
    pub transfer: TransferOptions,
}

/// Options for the `verify_cache()` sync function.
///
/// ## Fields
///
/// - `cache_dir` — override the local cache directory to verify. When `None`
///   (the default), the standard cache (`$XDG_CACHE_HOME/snapdir` or
///   `$HOME/.cache/snapdir`) is used — identical to the pre-field behaviour.
///
/// Note: not `#[non_exhaustive]` — see `ManifestOptions` for the rationale.
#[derive(Debug, Default, Clone)]
pub struct VerifyCacheOptions {
    /// Override the local cache directory to verify.
    ///
    /// `None` (the default) uses the standard cache (`$XDG_CACHE_HOME/snapdir`
    /// or `$HOME/.cache/snapdir`).
    pub cache_dir: Option<std::path::PathBuf>,
}

/// Options for the `flush_cache()` sync function.
///
/// ## Fields
///
/// - `cache_dir` — override the local cache directory to flush. When `None`
///   (the default), the standard cache (`$XDG_CACHE_HOME/snapdir` or
///   `$HOME/.cache/snapdir`) is used — identical to the pre-field behaviour.
///
/// Note: not `#[non_exhaustive]` — see `ManifestOptions` for the rationale.
#[derive(Debug, Default, Clone)]
pub struct CacheOptions {
    /// Override the local cache directory to flush.
    ///
    /// `None` (the default) uses the standard cache (`$XDG_CACHE_HOME/snapdir`
    /// or `$HOME/.cache/snapdir`).
    pub cache_dir: Option<std::path::PathBuf>,
}

/// Options for the `locations()` sync function.
///
/// Currently a marker struct; fields will be added in future minor versions.
///
/// Note: not `#[non_exhaustive]` — see `ManifestOptions` for the rationale.
#[derive(Debug, Default, Clone)]
pub struct LocationsOptions {}

/// Options for the `ancestors()` sync function.
///
/// Currently a marker struct; fields will be added in future minor versions.
///
/// Note: not `#[non_exhaustive]` — see `ManifestOptions` for the rationale.
#[derive(Debug, Default, Clone)]
pub struct AncestorsOptions {}

/// Options for the `revisions()` sync function.
///
/// Currently a marker struct; fields will be added in future minor versions.
///
/// Note: not `#[non_exhaustive]` — see `ManifestOptions` for the rationale.
#[derive(Debug, Default, Clone)]
pub struct RevisionsOptions {}

// ---------------------------------------------------------------------------
// Shared tokio runtime (OnceLock, multi-thread)
//
// Used when blocking callers need to drive async functions without their own
// runtime. The async public fns (fetch, pull, …) use spawn_blocking from
// within the caller's tokio context. This runtime is for non-async callers
// that need to run async-over-blocking operations (e.g. from sync bindings).
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub(crate) fn shared_runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("snapdir-api: failed to build shared tokio runtime")
    })
}

// ---------------------------------------------------------------------------
// Cache directory helper
// ---------------------------------------------------------------------------

/// Returns the default local cache directory: `$XDG_CACHE_HOME/snapdir` or
/// `$HOME/.cache/snapdir`.
fn cache_dir_default() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let base = std::env::var("XDG_CACHE_HOME").unwrap_or_else(|_| format!("{home}/.cache"));
    PathBuf::from(format!("{base}/snapdir"))
}

// ---------------------------------------------------------------------------
// Internal walk helpers (used by manifest() and id())
// ---------------------------------------------------------------------------

/// Resolves the walk root to an absolute path, mirroring the CLI's
/// `resolve_root` (makes relative paths absolute via `current_dir`, then
/// lexically normalizes). The walk requires an absolute root.
fn resolve_api_root(path: &Path) -> Result<std::path::PathBuf> {
    use std::path::Component;
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(SnapdirError::Io)?
            .join(path)
    };
    // Lexically normalize: strip CurDir (`.`) components and trailing slashes
    // (except filesystem root `/`). Preserves `..` verbatim, no fs access.
    let mut out = std::path::PathBuf::new();
    for comp in abs.components() {
        match comp {
            Component::CurDir => {}
            Component::RootDir
            | Component::Prefix(_)
            | Component::Normal(_)
            | Component::ParentDir => out.push(comp.as_os_str()),
        }
    }
    Ok(out)
}

/// Builds [`snapdir_core::walk::WalkOptions`] from the API [`ManifestOptions`],
/// wiring the `absolute`, `no_follow`, `exclude`, and `walk_jobs` fields.
///
/// Returns an error string if an `exclude` pattern fails to compile.
fn build_walk_options(
    opts: &ManifestOptions,
) -> std::result::Result<snapdir_core::walk::WalkOptions, String> {
    use snapdir_core::excludes::{ExcludeMatcher, FollowMode};
    use snapdir_core::walk::{PathMode, WalkOptions};

    let follow = if opts.no_follow {
        FollowMode::NoFollow
    } else {
        FollowMode::Follow
    };
    let path_mode = if opts.absolute {
        PathMode::Absolute
    } else {
        PathMode::Relative
    };
    // Build an exclude matcher from the option patterns. For API callers (no
    // CLI) we do not expand `%system%`/`%common%` macros — those are CLI-env
    // runtime expansions. Plain regex patterns are compiled as-is.
    let exclude = if opts.exclude.is_empty() {
        None
    } else {
        // Wrap each pattern in a non-capturing group then OR them together,
        // mirroring the CLI's `combine_excludes`.
        let groups: Vec<String> = opts.exclude.iter().map(|p| format!("(?:{p})")).collect();
        let combined = groups.join("|");
        Some(ExcludeMatcher::new(&combined).map_err(|e| format!("invalid exclude pattern: {e}"))?)
    };
    Ok(WalkOptions {
        follow,
        path_mode,
        exclude,
        walk_jobs: opts.walk_jobs,
        ..WalkOptions::default()
    })
}

/// Walks the tree at `root` using the hasher selected by `opts.checksum_bin`,
/// returning the core manifest. Dispatches to the correct concrete hasher type
/// (required because `walk` takes `H: Hasher + HashFile + Sync`, not `dyn`).
fn walk_with_opts(
    root: &std::path::Path,
    walk_opts: &snapdir_core::walk::WalkOptions,
    opts: &ManifestOptions,
) -> std::result::Result<snapdir_core::manifest::Manifest, snapdir_core::walk::WalkError> {
    use snapdir_core::merkle::{Blake3Hasher, Md5Hasher, Sha256Hasher};
    use snapdir_core::walk::walk;
    match opts.checksum_bin {
        ChecksumBin::B3sum => walk(root, walk_opts, &Blake3Hasher::new()),
        ChecksumBin::Md5sum => walk(root, walk_opts, &Md5Hasher::new()),
        ChecksumBin::Sha256sum => walk(root, walk_opts, &Sha256Hasher::new()),
    }
}

// ---------------------------------------------------------------------------
// Sync API functions
// ---------------------------------------------------------------------------

/// Walks `path` and returns a typed `Manifest`.
///
/// This is a synchronous, CPU-bound operation (BLAKE3 hashing). Language
/// bindings should run it on a thread pool if they need non-blocking behaviour.
///
/// # Errors
///
/// Returns `Err(SnapdirError::Io)` if `path` is not accessible or the walk
/// fails for filesystem reasons.
pub fn manifest(path: &Path, options: &ManifestOptions) -> Result<Manifest> {
    let walk_opts = build_walk_options(options)
        .map_err(|e| SnapdirError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))?;
    let root = resolve_api_root(path)?;
    let core_manifest = walk_with_opts(&root, &walk_opts, options).map_err(|e| match e {
        snapdir_core::walk::WalkError::Io { path: _, source } => SnapdirError::Io(source),
        other => SnapdirError::Io(std::io::Error::other(other.to_string())),
    })?;
    Ok(Manifest::from_core(&core_manifest))
}

/// Computes the snapshot id for the directory at `path`.
///
/// Walks `path` (same as [`manifest`]) and hashes the manifest text via
/// BLAKE3 to produce the content-addressed snapshot id.
///
/// # Errors
///
/// Returns `Err(SnapdirError::Io)` if `path` is not accessible or the walk
/// fails.
pub fn id(path: &Path, options: &ManifestOptions) -> Result<SnapshotId> {
    use snapdir_core::merkle::{snapshot_id, Blake3Hasher};

    let walk_opts = build_walk_options(options)
        .map_err(|e| SnapdirError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, e)))?;
    let root = resolve_api_root(path)?;
    let core_manifest = walk_with_opts(&root, &walk_opts, options).map_err(|e| match e {
        snapdir_core::walk::WalkError::Io { path: _, source } => SnapdirError::Io(source),
        other => SnapdirError::Io(std::io::Error::other(other.to_string())),
    })?;
    // The snapshot id is always BLAKE3 of the manifest text, regardless of
    // checksum_bin (the CLI comments confirm: `id` ignores --checksum-bin).
    let b3 = Blake3Hasher::new();
    let hex = snapshot_id(&core_manifest, &b3);
    SnapshotId::from_hex(&hex)
}

/// Computes the snapshot id from an already-computed [`Manifest`].
///
/// This is a pure, infallible operation: it parses `m.raw` back to the core
/// manifest format and applies the same BLAKE3 snapshot-id function. The
/// result is identical to calling [`id`] on the original path.
///
/// ```text
/// # Requires a real filesystem path; run via integration tests.
/// use snapdir_api::{manifest, id_from_manifest, ManifestOptions};
/// let m = manifest(std::path::Path::new("/tmp"), &ManifestOptions::default()).unwrap();
/// let _ = id_from_manifest(&m);
/// ```
#[must_use]
pub fn id_from_manifest(m: &Manifest) -> SnapshotId {
    use snapdir_core::manifest::Manifest as CoreManifest;
    use snapdir_core::merkle::{snapshot_id, Blake3Hasher};

    let hasher = Blake3Hasher::new();
    // Re-parse from the raw text so we use the same format as the oracle.
    let core = CoreManifest::parse(&m.raw).expect("raw manifest text must be valid");
    let hex = snapshot_id(&core, &hasher);
    SnapshotId::from_hex(&hex).expect("snapshot_id always returns a valid 64-hex string")
}

/// Stages `path` in the local cache and returns the snapshot id.
///
/// Walking, hashing, and pushing to the local cache are all performed
/// synchronously. The snapshot can then be pushed to a remote store via
/// `push(PushSource::StagedId(&id), &store, &opts)`.
///
/// # Errors
///
/// Returns `Err(SnapdirError)` if the walk fails or the cache write fails.
pub fn stage(path: &Path, _options: &StageOptions) -> Result<SnapshotId> {
    use snapdir_core::merkle::{snapshot_id, Blake3Hasher};
    use snapdir_core::store::Store;
    use snapdir_core::walk::{walk, WalkOptions};
    use snapdir_stores::file_store::FileStore;

    let hasher = Blake3Hasher::new();
    let walk_opts = WalkOptions::default();
    let root = resolve_api_root(path)?;
    let core_manifest = walk(&root, &walk_opts, &hasher).map_err(|e| match e {
        snapdir_core::walk::WalkError::Io { path: _, source } => SnapdirError::Io(source),
        other => SnapdirError::Io(std::io::Error::other(other.to_string())),
    })?;
    let hex = snapshot_id(&core_manifest, &hasher);
    let snap_id = SnapshotId::from_hex(&hex)?;

    // Push the manifest and objects into the local cache store, unless the
    // caller requested a no-cache run (`keep: false`).
    if _options.keep {
        let cache_dir = _options.cache_dir.clone().unwrap_or_else(cache_dir_default);
        let cache_str = format!("file://{}", cache_dir.display());
        let cache_fs = FileStore::new(&cache_str);
        cache_fs
            .push(&core_manifest, path)
            .map_err(SnapdirError::from)?;
    }

    Ok(snap_id)
}

/// Verifies every object in the local cache.
///
/// Returns a [`VerifyCacheResult`] indicating whether the cache is clean.
/// An empty or missing cache is always clean.
///
/// # Errors
///
/// Returns `Err(SnapdirError::Io)` on filesystem traversal failure.
pub fn verify_cache(_o: &VerifyCacheOptions) -> Result<VerifyCacheResult> {
    use snapdir_core::merkle::Blake3Hasher;
    use snapdir_core::{verify_cache as core_verify_cache, CacheError};

    let cache_dir = _o.cache_dir.clone().unwrap_or_else(cache_dir_default);
    let hasher = Blake3Hasher::new();
    match core_verify_cache(&cache_dir, false, &hasher) {
        Ok(report) => Ok(VerifyCacheResult {
            ok: report.is_clean(),
        }),
        Err(CacheError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            // Missing cache dir = clean.
            Ok(VerifyCacheResult { ok: true })
        }
        Err(e) => Err(SnapdirError::Io(std::io::Error::other(e.to_string()))),
    }
}

/// Flushes (empties) the local cache.
///
/// Idempotent: a missing cache directory is a successful no-op.
///
/// # Errors
///
/// Returns `Err(SnapdirError::Io)` on a removal failure (other than the cache
/// simply being absent).
pub fn flush_cache(_o: &CacheOptions) -> Result<()> {
    use snapdir_core::flush_cache as core_flush_cache;

    let cache_dir = _o.cache_dir.clone().unwrap_or_else(cache_dir_default);
    core_flush_cache(&cache_dir).map_err(|e| SnapdirError::Io(std::io::Error::other(e.to_string())))
}

/// Resolves the catalog database path.
///
/// Resolution order:
/// 1. `$SNAPDIR_CATALOG_DB_PATH` env var (set by tests and callers).
/// 2. `$XDG_CACHE_HOME/snapdir/catalog.redb` (or `$HOME/.cache/…`).
///
/// Returns `None` when the resolved path does not exist on disk, so the
/// caller can short-circuit to `Ok(vec![])` without opening a file.
fn resolve_catalog_db() -> Option<PathBuf> {
    // 1. Explicit env var (used by integration tests and callers that want a
    //    specific catalog without modifying the options struct).
    if let Ok(p) = std::env::var("SNAPDIR_CATALOG_DB_PATH") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
        // Env var was set but the file does not exist yet → empty catalog.
        return None;
    }
    // 2. Default location derived from the cache dir.
    let default_path = cache_dir_default().join("catalog.redb");
    if default_path.exists() {
        Some(default_path)
    } else {
        None
    }
}

/// Maps a [`snapdir_catalog::CatalogError`] into [`SnapdirError::CatalogError`].
fn catalog_err(e: &snapdir_catalog::CatalogError) -> SnapdirError {
    SnapdirError::CatalogError {
        message: e.to_string(),
    }
}

/// Opens the catalog at `db_path`, retrying on transient lock contention.
///
/// redb uses an exclusive file lock; if another process (or test thread) holds
/// the lock transiently, a brief spin avoids a spurious error. We try up to 10
/// times with 20 ms between attempts before surfacing the error to the caller.
///
/// Returns `Ok(None)` when the database file has disappeared since we checked
/// (TOCTOU: the file existed at `resolve_catalog_db` time but was deleted
/// concurrently). The callers treat `Ok(None)` as "no catalog".
fn open_catalog_with_retry(db_path: &std::path::Path) -> Result<Option<snapdir_catalog::Catalog>> {
    let mut last_err = None;
    for attempt in 0..10u32 {
        // If the file vanished between resolve_catalog_db and here, treat as
        // "no catalog" rather than an error (handles TempDir cleanup races).
        if !db_path.exists() {
            return Ok(None);
        }
        match snapdir_catalog::Catalog::open(db_path) {
            Ok(cat) => return Ok(Some(cat)),
            Err(e) => {
                let msg = e.to_string();
                // "Cannot acquire lock" / "already open" → exclusive-lock contention.
                // "No such file or directory" → TOCTOU deletion race.
                if msg.contains("Cannot acquire lock")
                    || msg.contains("already open")
                    || msg.contains("No such file or directory")
                {
                    last_err = Some(e);
                    // Exponential-ish back-off: 20, 20, 40, 40, … ms
                    let delay = 20 * u64::from((attempt / 2) + 1);
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                } else {
                    return Err(catalog_err(&e));
                }
            }
        }
    }
    Err(catalog_err(&last_err.expect("attempted at least once")))
}

/// Lists all catalog locations.
///
/// Reads the catalog database whose path is resolved from
/// `$SNAPDIR_CATALOG_DB_PATH` (if set) or the default cache-dir location.
/// Returns `Ok(vec![])` when no catalog database exists on disk.
///
/// # Errors
///
/// Returns `Err(SnapdirError::CatalogError)` on a catalog backend failure.
pub fn locations(_o: &LocationsOptions) -> Result<Vec<Location>> {
    let Some(db_path) = resolve_catalog_db() else {
        return Ok(vec![]);
    };
    let Some(cat) = open_catalog_with_retry(&db_path)? else {
        return Ok(vec![]);
    };
    let records = cat.locations().map_err(|e| catalog_err(&e))?;
    Ok(records
        .into_iter()
        .map(|r| Location {
            created_at: r.created_at,
            id: r.id,
            location: r.location,
        })
        .collect())
}

/// Lists the ancestors of `id` in the catalog lineage chain.
///
/// Reads the catalog database whose path is resolved from
/// `$SNAPDIR_CATALOG_DB_PATH` (if set) or the default cache-dir location.
/// Returns `Ok(vec![])` when no catalog database exists on disk.
///
/// # Errors
///
/// Returns `Err(SnapdirError::CatalogError)` on a catalog backend failure.
pub fn ancestors(id: &SnapshotId, _o: &AncestorsOptions) -> Result<Vec<Ancestor>> {
    let Some(db_path) = resolve_catalog_db() else {
        return Ok(vec![]);
    };
    let hex = id.to_hex();
    let Some(cat) = open_catalog_with_retry(&db_path)? else {
        return Ok(vec![]);
    };
    let records = cat.ancestors(&hex, None).map_err(|e| catalog_err(&e))?;
    Ok(records
        .into_iter()
        .map(|r| Ancestor {
            created_at: r.created_at,
            id: r.id,
            location: r.location,
        })
        .collect())
}

/// Lists the revision history at `location` in the catalog.
///
/// Reads the catalog database whose path is resolved from
/// `$SNAPDIR_CATALOG_DB_PATH` (if set) or the default cache-dir location.
/// Returns `Ok(vec![])` when no catalog database exists on disk.
///
/// # Errors
///
/// Returns `Err(SnapdirError::CatalogError)` on a catalog backend failure.
pub fn revisions(location: &LocationRef, _o: &RevisionsOptions) -> Result<Vec<Revision>> {
    let Some(db_path) = resolve_catalog_db() else {
        return Ok(vec![]);
    };
    let loc_str = location.as_str();
    let Some(cat) = open_catalog_with_retry(&db_path)? else {
        return Ok(vec![]);
    };
    let records = cat.revisions(loc_str).map_err(|e| catalog_err(&e))?;
    Ok(records
        .into_iter()
        .map(|r| Revision {
            created_at: r.created_at,
            id: r.id,
            previous_id: r.previous_id,
        })
        .collect())
}

/// Returns the effective configuration as resolved from env vars and defaults.
///
/// For M0, returns an empty entry list (all factory defaults apply).
#[must_use]
pub fn defaults() -> EffectiveConfig {
    EffectiveConfig { entries: vec![] }
}

// ---------------------------------------------------------------------------
// Async API functions (spawn_blocking over the sync stores engine)
// ---------------------------------------------------------------------------

/// Fetches a snapshot from `store` into the local cache (manifest + all objects).
///
/// After a successful `fetch`, the snapshot is available for [`checkout`]
/// (materialise to a directory) without hitting the remote store again.
///
/// # Errors
///
/// Returns `Err(SnapdirError)` if the store is unreachable, the snapshot is
/// absent, or the local cache write fails.
pub async fn fetch(id: &SnapshotId, store: &StoreUri, _options: &TransferOptions) -> Result<()> {
    let hex_id = id.to_hex();
    let store_str = store.raw.clone();

    tokio::task::spawn_blocking(move || fetch_sync(&hex_id, &store_str))
        .await
        .map_err(|e| SnapdirError::Io(std::io::Error::other(e.to_string())))?
}

/// Pulls a snapshot from `store` into `dest`, materializing its files.
///
/// Async wrapper over the sync stores fetch path. A missing store or absent
/// snapshot surfaces as `STORE_ERROR`.
///
/// # Errors
///
/// Returns `Err(SnapdirError)` on any fetch or materialize failure.
pub async fn pull(
    id: &SnapshotId,
    store: &StoreUri,
    dest: &Path,
    _options: &CheckoutOptions,
) -> Result<()> {
    let hex_id = id.to_hex();
    let store_str = store.raw.clone();
    let dest = dest.to_path_buf();

    tokio::task::spawn_blocking(move || pull_sync(&hex_id, &store_str, &dest))
        .await
        .map_err(|e| SnapdirError::Io(std::io::Error::other(e.to_string())))?
}

/// Pushes a snapshot to `store` and returns the snapshot id.
///
/// `src` can be a filesystem path ([`PushSource::Path`]) or a previously
/// staged snapshot id ([`PushSource::StagedId`]). In both cases the returned
/// id is the content-addressed id of the snapshot data.
///
/// # Errors
///
/// Returns `Err(SnapdirError)` if the walk fails, the store write fails, or
/// the staged manifest is not found in the local cache.
pub async fn push(
    src: PushSource<'_>,
    store: &StoreUri,
    _o: &TransferOptions,
) -> Result<SnapshotId> {
    let store_str = store.raw.clone();

    match src {
        PushSource::Path(path) => {
            // Compute manifest + id synchronously (CPU-bound, fast enough inline).
            use snapdir_core::merkle::{snapshot_id, Blake3Hasher};
            use snapdir_core::walk::{walk, WalkOptions};

            let hasher = Blake3Hasher::new();
            let walk_opts = WalkOptions::default();
            let root = resolve_api_root(path)?;
            let path_buf = root.clone();
            let core_manifest = walk(&root, &walk_opts, &hasher).map_err(|e| match e {
                snapdir_core::walk::WalkError::Io { path: _, source } => SnapdirError::Io(source),
                other => SnapdirError::Io(std::io::Error::other(other.to_string())),
            })?;
            let hex = snapshot_id(&core_manifest, &hasher);
            let snap_id = SnapshotId::from_hex(&hex)?;

            tokio::task::spawn_blocking(move || {
                let fs = open_store(&store_str)?;
                fs.push(&core_manifest, &path_buf)
                    .map_err(SnapdirError::from)?;
                Ok(snap_id)
            })
            .await
            .map_err(|e| SnapdirError::Io(std::io::Error::other(e.to_string())))?
        }
        PushSource::StagedId(staged_id) => {
            let hex_id = staged_id.to_hex();
            let snap_id = *staged_id;

            tokio::task::spawn_blocking(move || {
                use snapdir_core::load_cached_manifest;
                use snapdir_stores::file_store::FileStore;

                let cache_dir = cache_dir_default();

                // Load the manifest from the local cache.
                let core_manifest = load_cached_manifest(&cache_dir, &hex_id)
                    .map_err(|e| SnapdirError::Io(std::io::Error::other(e.to_string())))?;

                // Sync from cache to the destination store using StreamStore.
                let cache_str = format!("file://{}", cache_dir.display());
                let cache_fs = FileStore::new(&cache_str);
                let dst_fs = open_stream_store(&store_str)?;
                snapdir_stores::sync_snapshot(
                    &cache_fs,
                    &*dst_fs,
                    &hex_id,
                    &snapdir_stores::transfer::TransferConfig::default(),
                    false,
                    None,
                )
                .map_err(SnapdirError::from)?;

                // Verify the manifest was written to the destination.
                dst_fs.get_manifest(&hex_id).map_err(SnapdirError::from)?;
                drop(core_manifest);
                Ok(snap_id)
            })
            .await
            .map_err(|e| SnapdirError::Io(std::io::Error::other(e.to_string())))?
        }
    }
}

/// Checks out a snapshot from the local cache into `dest`.
///
/// The snapshot must have been fetched into the local cache first (via
/// [`fetch`] or [`stage`] + [`push`]). Materialises all files at their
/// relative paths under `dest`.
///
/// # Errors
///
/// Returns `Err(SnapdirError)` if the manifest is not found in the cache or
/// file materialization fails.
pub async fn checkout(id: &SnapshotId, dest: &Path, _o: &CheckoutOptions) -> Result<()> {
    let hex_id = id.to_hex();
    let dest_buf = dest.to_path_buf();

    tokio::task::spawn_blocking(move || {
        use snapdir_core::store::Store;
        use snapdir_stores::file_store::FileStore;

        let cache_dir = cache_dir_default();
        let cache_str = format!("file://{}", cache_dir.display());
        let cache_fs = FileStore::new(&cache_str);
        let manifest = cache_fs.get_manifest(&hex_id).map_err(SnapdirError::from)?;
        cache_fs
            .fetch_files(&manifest, &dest_buf)
            .map_err(SnapdirError::from)?;
        restore_permissions(&manifest, &dest_buf)
    })
    .await
    .map_err(|e| SnapdirError::Io(std::io::Error::other(e.to_string())))?
}

/// Syncs a snapshot from `src` store to `dst` store.
///
/// Copies the manifest and all referenced objects from `src` to `dst` through
/// memory only (no local filesystem staging). Skips objects the destination
/// already holds.
///
/// # Errors
///
/// Returns `Err(SnapdirError)` if either store is unreachable or the snapshot
/// is absent in `src`.
pub async fn sync(
    id: &SnapshotId,
    src: &StoreUri,
    dst: &StoreUri,
    _o: &TransferOptions,
) -> Result<()> {
    let hex_id = id.to_hex();
    let src_str = src.raw.clone();
    let dst_str = dst.raw.clone();

    tokio::task::spawn_blocking(move || {
        let src_fs = open_stream_store(&src_str)?;
        let dst_fs = open_stream_store(&dst_str)?;
        snapdir_stores::sync_snapshot(
            &*src_fs,
            &*dst_fs,
            &hex_id,
            &snapdir_stores::transfer::TransferConfig::default(),
            false,
            None,
        )
        .map(|_| ())
        .map_err(SnapdirError::from)
    })
    .await
    .map_err(|e| SnapdirError::Io(std::io::Error::other(e.to_string())))?
}

/// Verifies a snapshot in `store` — checks that all objects are present and
/// hash-consistent.
///
/// Returns a [`VerifyResult`] with `ok: true` when the snapshot is healthy.
///
/// # Errors
///
/// Returns `Err(SnapdirError)` if the store is unreachable, the snapshot is
/// absent, or a transport error occurs.
pub async fn verify(id: &SnapshotId, store: &StoreUri, _o: &VerifyOptions) -> Result<VerifyResult> {
    let hex_id = id.to_hex();
    let store_str = store.raw.clone();

    tokio::task::spawn_blocking(move || {
        let fs = open_stream_store(&store_str)?;
        let manifest = fs.get_manifest(&hex_id).map_err(SnapdirError::from)?;
        // Verify each file object is present and hash-consistent.
        for entry in manifest.entries() {
            if entry.path_type == snapdir_core::manifest::PathType::File {
                fs.get_object(&entry.checksum).map_err(SnapdirError::from)?;
            }
        }
        Ok(VerifyResult { ok: true })
    })
    .await
    .map_err(|e| SnapdirError::Io(std::io::Error::other(e.to_string())))?
}

/// Diffs two sets of stores, returning structured change entries.
///
/// For each store in `o.from`, loads all manifests and unions the entries into
/// a "from" map. Likewise for `o.to`. Classifies each path as `Added`,
/// `Deleted`, `Modified`, or (with `o.all`) `Unchanged`.
///
/// When a single side (`from` or `to`) unions multiple store URIs that carry
/// the **same path with differing content** (an intra-side path collision),
/// `o.on_conflict` controls the outcome:
///
/// - [`ConflictPolicy::Error`] (the default) — returns
///   `Err(SnapdirError::Conflict)` whose `.code()` is `"CONFLICT"`. The error
///   message names the colliding path.
/// - [`ConflictPolicy::LastWins`] — resolves deterministically to the last
///   URI's entry (the final store listed in `o.from` / `o.to` wins), mirroring
///   the CLI's `--on-conflict last-wins` semantics.
///
/// Single-store-per-side diffs (no collision possible) are behaviour-identical
/// to before this policy was wired.
///
/// # Errors
///
/// Returns `Err(SnapdirError)` if any store is unreachable, a manifest cannot
/// be read, or `on_conflict=Error` and an intra-side collision is detected.
pub async fn diff(o: &DiffOptions) -> Result<Vec<DiffEntry>> {
    let from_stores: Vec<String> = o.from.iter().map(|s| s.raw.clone()).collect();
    let to_stores: Vec<String> = o.to.iter().map(|s| s.raw.clone()).collect();
    let include_unchanged = o.all;
    let on_conflict = o.on_conflict;

    tokio::task::spawn_blocking(move || {
        diff_sync(&from_stores, &to_stores, include_unchanged, on_conflict)
    })
    .await
    .map_err(|e| SnapdirError::Io(std::io::Error::other(e.to_string())))?
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn fetch_sync(hex_id: &str, store_str: &str) -> Result<()> {
    use snapdir_stores::file_store::FileStore;

    // Get the manifest from the remote store, then sync everything to cache.
    let src_fs = open_stream_store(store_str)?;
    let cache_dir = cache_dir_default();
    let cache_str = format!("file://{}", cache_dir.display());
    let cache_fs = FileStore::new(&cache_str);

    snapdir_stores::sync_snapshot(
        &*src_fs,
        &cache_fs,
        hex_id,
        &snapdir_stores::transfer::TransferConfig::default(),
        false,
        None,
    )
    .map(|_| ())
    .map_err(SnapdirError::from)
}

fn pull_sync(hex_id: &str, store_str: &str, dest: &Path) -> Result<()> {
    let store = open_store(store_str)?;
    let manifest = store.get_manifest(hex_id).map_err(SnapdirError::from)?;
    store
        .fetch_files(&manifest, dest)
        .map_err(SnapdirError::from)?;
    restore_permissions(&manifest, dest)
}

// Restores the per-entry permission modes recorded in `manifest` onto the
// materialized tree at `dest`.  Files are handled first; directories are
// applied deepest-first so tightening a parent's mode never blocks setting a
// child's.  Mirrors the CLI's `restore_permissions` / `apply_mode` pair in
// `snapdir-cli/src/cli.rs`.
fn restore_permissions(manifest: &snapdir_core::manifest::Manifest, dest: &Path) -> Result<()> {
    use snapdir_core::manifest::PathType;

    // Files first.
    for entry in manifest.entries() {
        if entry.path_type == PathType::Directory {
            continue;
        }
        apply_mode(dest, entry)?;
    }

    // Directories last, deepest-first.
    let mut dirs: Vec<&snapdir_core::manifest::ManifestEntry> = manifest
        .entries()
        .iter()
        .filter(|e| e.path_type == PathType::Directory)
        .collect();
    dirs.sort_by_key(|e| std::cmp::Reverse(e.path.len()));
    for entry in dirs {
        apply_mode(dest, entry)?;
    }
    Ok(())
}

// Parses a manifest entry's octal permission string and applies it to the
// entry's path under `dest`.
fn apply_mode(dest: &Path, entry: &snapdir_core::manifest::ManifestEntry) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let rel = entry.path.strip_prefix("./").unwrap_or(&entry.path);
    let rel = rel.strip_suffix('/').unwrap_or(rel);
    let target = if rel.is_empty() {
        dest.to_path_buf()
    } else {
        dest.join(rel)
    };
    let mode = u32::from_str_radix(&entry.permissions, 8).map_err(|_| {
        SnapdirError::Io(std::io::Error::other(format!(
            "invalid permissions {:?}",
            entry.permissions
        )))
    })?;
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode)).map_err(|e| {
        SnapdirError::Io(std::io::Error::other(format!(
            "setting permissions on {}: {e}",
            target.display()
        )))
    })
}

/// A fingerprint for a manifest entry as used by diff classification.
///
/// Mirrors the CLI's `diff::fingerprint`: for FILES we compare
/// (`path_type`, permissions, checksum, size); for DIRECTORIES we compare
/// only (`path_type`, permissions) — the directory checksum/size change whenever
/// any descendant changes, but those changes are reported on the descendants'
/// own lines. A file↔directory type change still surfaces as Modified.
#[derive(PartialEq)]
struct EntryFingerprint {
    path_type: snapdir_core::manifest::PathType,
    /// `Some(checksum)` for files; `None` for directories.
    checksum: Option<String>,
    /// `Some(size)` for files; `None` for directories.
    size: Option<u64>,
    permissions: String,
}

impl EntryFingerprint {
    fn from_entry(e: &snapdir_core::manifest::ManifestEntry) -> Self {
        use snapdir_core::manifest::PathType;
        match e.path_type {
            PathType::File => Self {
                path_type: PathType::File,
                checksum: Some(e.checksum.clone()),
                size: Some(e.size),
                permissions: e.permissions.clone(),
            },
            PathType::Directory => Self {
                path_type: PathType::Directory,
                checksum: None,
                size: None,
                permissions: e.permissions.clone(),
            },
        }
    }

    fn is_dir(&self) -> bool {
        self.path_type == snapdir_core::manifest::PathType::Directory
    }
}

/// Unions all manifests from a slice of store URI strings into a
/// `path → fingerprint` map, honoring `on_conflict` for intra-side collisions.
///
/// A collision is detected when a path is already present in the map with a
/// **different** fingerprint. Same-content entries (identical fingerprint) are
/// silently accepted (not a collision — the CLI behaves identically).
///
/// Returns `Err` on `ConflictPolicy::Error` collision, or the completed map
/// on `ConflictPolicy::LastWins` (last store's entry wins deterministically).
fn union_side(
    stores: &[String],
    on_conflict: ConflictPolicy,
    side_label: &str,
) -> Result<std::collections::BTreeMap<String, EntryFingerprint>> {
    use std::collections::BTreeMap;

    let mut map: BTreeMap<String, EntryFingerprint> = BTreeMap::new();
    for store_str in stores {
        let fs = match open_stream_store(store_str) {
            Ok(fs) => fs,
            Err(_) => continue,
        };
        let Ok(ids) = fs.list_manifest_ids() else {
            continue;
        };
        for id in ids {
            if let Ok(m) = fs.get_manifest(&id) {
                for entry in m.entries() {
                    let fp = EntryFingerprint::from_entry(entry);
                    if let Some(existing) = map.get(&entry.path) {
                        if existing != &fp {
                            // Differing content for the same path on one side =
                            // an intra-side collision.
                            match on_conflict {
                                ConflictPolicy::Error => {
                                    return Err(SnapdirError::Conflict {
                                        message: format!(
                                            "intra-side path collision on '{}' side: \
                                             path {:?} appears with differing content \
                                             across multiple store URIs; use \
                                             ConflictPolicy::LastWins to resolve",
                                            side_label, entry.path,
                                        ),
                                    });
                                }
                                ConflictPolicy::LastWins => {
                                    // Last store wins — overwrite with this entry.
                                    map.insert(entry.path.clone(), fp);
                                }
                            }
                        }
                        // Same fingerprint = same content, not a collision; skip.
                    } else {
                        map.insert(entry.path.clone(), fp);
                    }
                }
            }
        }
    }
    Ok(map)
}

/// Diffs two sets of stores (sync inner).
fn diff_sync(
    from_stores: &[String],
    to_stores: &[String],
    include_unchanged: bool,
    on_conflict: ConflictPolicy,
) -> Result<Vec<DiffEntry>> {
    // Union all manifests from the "from" stores into a path -> fingerprint map,
    // applying the collision policy for intra-side duplicates.
    let from_map = union_side(from_stores, on_conflict, "from")?;

    // Union all manifests from the "to" stores into a path -> fingerprint map.
    let to_map = union_side(to_stores, on_conflict, "to")?;

    // Classify differences.
    // Mirror the CLI's `diff::classify`: skip directory-only paths (a directory
    // is never its own diff row — its descendants are). Only a file↔directory
    // type change at the same path surfaces as M.
    let mut all_paths: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for k in from_map.keys() {
        all_paths.insert(k.as_str());
    }
    for k in to_map.keys() {
        all_paths.insert(k.as_str());
    }

    let mut entries: Vec<DiffEntry> = Vec::new();
    for path in &all_paths {
        let in_from = from_map.get(*path);
        let in_to = to_map.get(*path);

        // Skip if the path is a directory on every side it appears (and never a
        // file on any side): directory merkle/size changes surface via children.
        let present_only_dirs = match (in_from, in_to) {
            (Some(f), Some(t)) => f.is_dir() && t.is_dir(),
            (Some(e), None) | (None, Some(e)) => e.is_dir(),
            (None, None) => false,
        };
        if present_only_dirs {
            continue;
        }

        let status = match (in_from, in_to) {
            (None, Some(_)) => DiffStatus::Added,
            (Some(_), None) => DiffStatus::Deleted,
            (Some(f), Some(t)) => {
                if f == t {
                    DiffStatus::Unchanged
                } else {
                    DiffStatus::Modified
                }
            }
            (None, None) => unreachable!("path must be in at least one side"),
        };
        if status == DiffStatus::Unchanged && !include_unchanged {
            continue;
        }
        entries.push(DiffEntry {
            status,
            path: PathBuf::from(path),
        });
    }

    Ok(entries)
}

// ---------------------------------------------------------------------------
// Store dispatch helpers (scheme → concrete backend)
// ---------------------------------------------------------------------------

/// Routes a store URI to its concrete [`snapdir_core::store::Store`]
/// implementation, dispatching on the URI scheme.
///
/// - `file://` → [`snapdir_stores::file_store::FileStore`]
/// - `s3://`   → [`snapdir_stores::S3Store`] (endpoint from
///               `SNAPDIR_S3_STORE_ENDPOINT_URL`)
/// - `b2://`   → [`snapdir_stores::B2Store`] (endpoint from
///               `SNAPDIR_B2_TEST_ENDPOINT` or `SNAPDIR_S3_STORE_ENDPOINT_URL`;
///               region from `SNAPDIR_B2_REGION` / `AWS_REGION`)
/// - `gs://`   → [`snapdir_stores::GcsStore`] (ADC credential chain)
/// - any other → [`snapdir_stores::ExternalStore`] (emit-command shim,
///               dispatches to a `snapdir-<scheme>-store` binary on PATH)
fn open_store(uri: &str) -> Result<Box<dyn snapdir_core::store::Store>> {
    use snapdir_stores::router::{resolve_adapter, Adapter};
    use snapdir_stores::{file_store::FileStore, B2Store, ExternalStore, GcsStore, S3Store};

    let adapter = resolve_adapter(uri).map_err(|e| SnapdirError::InvalidStore {
        message: e.to_string(),
    })?;
    match adapter {
        Adapter::File => Ok(Box::new(FileStore::new(uri))),
        Adapter::S3 => {
            let endpoint = std::env::var("SNAPDIR_S3_STORE_ENDPOINT_URL").ok();
            Ok(Box::new(
                S3Store::connect(uri, endpoint.as_deref()).map_err(SnapdirError::from)?,
            ))
        }
        Adapter::B2 => {
            // B2Store::connect checks SNAPDIR_B2_TEST_ENDPOINT automatically when
            // endpoint is None; region falls back to SNAPDIR_B2_REGION / AWS_REGION.
            Ok(Box::new(
                B2Store::connect(uri, None, None).map_err(SnapdirError::from)?,
            ))
        }
        Adapter::Gcs => Ok(Box::new(
            GcsStore::connect(uri).map_err(SnapdirError::from)?,
        )),
        Adapter::External { .. } => Ok(Box::new(
            ExternalStore::new(uri).map_err(SnapdirError::from)?,
        )),
    }
}

/// Like [`open_store`], but builds a [`snapdir_stores::stream::StreamStore`]
/// for operations that require streaming access (fetch, sync, verify).
///
/// External schemes (`ssh://`, `sftp://`, and any other third-party adapter)
/// do not implement [`snapdir_stores::stream::StreamStore`] and return
/// `Err(SnapdirError::InvalidStore)` — mirroring the CLI's `stream_store_for_adapter`
/// restriction.
fn open_stream_store(uri: &str) -> Result<Box<dyn snapdir_stores::stream::StreamStore + Sync>> {
    use snapdir_stores::router::{resolve_adapter, Adapter};
    use snapdir_stores::{file_store::FileStore, B2Store, GcsStore, S3Store};

    let adapter = resolve_adapter(uri).map_err(|e| SnapdirError::InvalidStore {
        message: e.to_string(),
    })?;
    match adapter {
        Adapter::File => Ok(Box::new(FileStore::new(uri))),
        Adapter::S3 => {
            let endpoint = std::env::var("SNAPDIR_S3_STORE_ENDPOINT_URL").ok();
            Ok(Box::new(
                S3Store::connect(uri, endpoint.as_deref()).map_err(SnapdirError::from)?,
            ))
        }
        Adapter::B2 => Ok(Box::new(
            B2Store::connect(uri, None, None).map_err(SnapdirError::from)?,
        )),
        Adapter::Gcs => Ok(Box::new(
            GcsStore::connect(uri).map_err(SnapdirError::from)?,
        )),
        Adapter::External { name } => Err(SnapdirError::InvalidStore {
            message: format!(
                "store-to-store streaming (fetch/sync/verify) requires an in-process store \
                 (file/s3/b2/gs); external `snapdir-{name}-store` URLs are not supported"
            ),
        }),
    }
}

// ---------------------------------------------------------------------------
// Crate version
// ---------------------------------------------------------------------------

/// Returns the version of this crate (tracks the snapdir CLI version).
///
/// ```
/// assert!(!snapdir_api::version().is_empty());
/// ```
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
