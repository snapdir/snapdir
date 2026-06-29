"""Type stub for the compiled snapdir Rust extension (snapdir.abi3.so).

Covers the full public surface exported from ``bindings/python/src/lib.rs``.
Every symbol here corresponds 1:1 to a ``#[pyclass]`` / ``#[pyfunction]``
in the Rust source.  The async functions return ``Coroutine`` objects —
declared here as ``async def`` so type checkers understand ``await`` on them.
"""

import os
from typing import final

# ---------------------------------------------------------------------------
# Exception hierarchy
# ---------------------------------------------------------------------------

class SnapdirError(Exception):
    """Base exception for all snapdir errors. Carries a ``.code`` attribute."""
    code: str

class HashMismatchError(SnapdirError):
    """Raised when a content checksum does not match the manifest record."""
    code: str

class StoreError(SnapdirError):
    """Raised for store-level I/O or protocol errors."""
    code: str

class InFluxError(SnapdirError):
    """Raised when a snapshot is still being written (concurrent modification)."""
    code: str

class CatalogError(SnapdirError):
    """Raised for local catalog / cache integrity errors."""
    code: str

# ---------------------------------------------------------------------------
# Result types (frozen — attribute access only, not subscriptable)
# ---------------------------------------------------------------------------

@final
class PathType:
    """Discriminates ``File`` vs ``Directory`` manifest entries.

    The ``.name`` attribute is either ``"File"`` or ``"Directory"``.
    """
    name: str
    def __str__(self) -> str: ...
    def __repr__(self) -> str: ...

@final
class ManifestEntry:
    """A single entry in a directory snapshot manifest.

    All attributes are read-only (frozen PyO3 class).
    ``size`` is an arbitrary-precision Python ``int`` (mapped from Rust ``u64``).
    ``checksum`` is a 64-character lowercase hex BLAKE3 digest string.
    ``path_type`` is a :class:`PathType` instance.
    """
    path: str
    path_type: PathType
    permissions: int
    checksum: str
    size: int
    def __repr__(self) -> str: ...

@final
class Manifest:
    """An ordered collection of manifest entries plus the raw manifest text.

    ``entries`` preserves the order returned by the directory walk.
    ``raw`` is the Display-rendered manifest string (same as ``snapdir manifest``
    CLI output).
    """
    entries: list[ManifestEntry]
    raw: str
    def __repr__(self) -> str: ...

@final
class DiffEntry:
    """A single entry in a diff result.

    ``status`` is one of ``"A"`` (added), ``"D"`` (deleted), ``"M"`` (modified),
    or ``"="`` (unchanged).
    """
    status: str
    path: str
    def __repr__(self) -> str: ...

# ---------------------------------------------------------------------------
# Value types
# ---------------------------------------------------------------------------

@final
class SnapshotId:
    """A validated 32-byte snapshot identifier (64-char lowercase hex).

    Raises :class:`SnapdirError` with code ``"INVALID_ID"`` when the input
    string is not exactly 64 lowercase hex characters.
    """
    def __new__(cls, s: str) -> SnapshotId: ...
    def __str__(self) -> str: ...
    def __repr__(self) -> str: ...

@final
class StoreUri:
    """A validated store URI (e.g. ``"file:///tmp/store"``, ``"s3://bucket/path"``).

    Raises :class:`SnapdirError` with code ``"INVALID_STORE"`` for unknown or
    malformed URI schemes.
    """
    def __new__(cls, s: str) -> StoreUri: ...
    def __str__(self) -> str: ...
    def __repr__(self) -> str: ...

@final
class DiffOptions:
    """Options for :func:`diff`. Constructed via :meth:`from_refs`.

    ``from`` is a reserved Python keyword so the constructor is named
    ``from_refs`` instead.
    """
    @classmethod
    def from_refs(
        cls,
        from_uris: list[StoreUri] | list[str],
        to_uris: list[StoreUri] | list[str],
    ) -> DiffOptions: ...
    def __repr__(self) -> str: ...

# ---------------------------------------------------------------------------
# Sync functions (no event loop required)
# ---------------------------------------------------------------------------

def version() -> str:
    """Return the ``snapdir-api`` crate version string (e.g. ``"1.10.0"``)."""
    ...

def id_from_manifest(m: Manifest) -> str:
    """Derive the snapshot ID from an already-computed :class:`Manifest`.

    Pure compute — no I/O, callable without a running event loop.
    Returns a 64-character lowercase hex string.
    """
    ...

# ---------------------------------------------------------------------------
# Async functions (all return awaitables / coroutines)
# ---------------------------------------------------------------------------

async def manifest(
    path: str | os.PathLike[str],
    *,
    no_follow: bool = ...,
    absolute: bool = ...,
    exclude: list[str] | None = ...,
) -> Manifest:
    """Walk ``path`` and return a typed :class:`Manifest`.

    Runs the BLAKE3 directory walk on a blocking thread pool so the asyncio
    event loop is not starved.  Accepts ``str`` or ``pathlib.Path``.

    Keyword-only options (all optional):
      - ``no_follow``: do not follow symlinks (default ``False``)
      - ``absolute``: render absolute paths instead of ``./``-relative (default ``False``)
      - ``exclude``: list of extended-regex patterns to exclude (default ``None``)
    """
    ...

async def id(
    path: str | os.PathLike[str],
    *,
    no_follow: bool = ...,
    absolute: bool = ...,
    exclude: list[str] | None = ...,
) -> str:
    """Compute the snapshot ID for ``path``.

    Runs on a blocking thread pool.  Returns a 64-character lowercase hex string.

    Keyword-only options mirror :func:`manifest`.
    """
    ...

async def stage(path: str | os.PathLike[str]) -> str:
    """Stage ``path`` in the local cache and return the snapshot ID.

    Runs the walk and cache write on a blocking thread pool.
    Returns a 64-character lowercase hex string.
    """
    ...

async def push(path: str | os.PathLike[str], store: StoreUri) -> str:
    """Push a snapshot from ``path`` to ``store`` and return the snapshot ID.

    Network / I/O-bound async operation.
    Returns a 64-character lowercase hex string.
    """
    ...

async def fetch(snapshot_id: SnapshotId, store: StoreUri) -> None:
    """Fetch a snapshot from ``store`` into the local cache.

    Network / I/O-bound async operation.
    """
    ...

async def pull(
    snapshot_id: SnapshotId,
    store: StoreUri,
    dest: str | os.PathLike[str],
) -> None:
    """Pull a snapshot from ``store`` into ``dest``, materializing its files.

    Network / I/O-bound async operation.
    """
    ...

async def checkout(snapshot_id: SnapshotId, dest: str | os.PathLike[str]) -> None:
    """Materialize a snapshot from the local cache to ``dest``.

    I/O-bound async operation.
    """
    ...

async def sync(
    snapshot_id: SnapshotId,
    src: StoreUri,
    dst: StoreUri,
) -> None:
    """Copy a snapshot from ``src`` store to ``dst`` store.

    Network / I/O-bound async operation.
    """
    ...

async def diff(opts: DiffOptions) -> list[DiffEntry]:
    """Diff two sets of stores, returning a list of :class:`DiffEntry` objects.

    I/O-bound async operation.
    """
    ...

async def verify(
    snapshot_id: SnapshotId | str,
    store: StoreUri,
) -> bool:
    """Verify a snapshot in ``store``. Raises :class:`SnapdirError` on failure.

    ``snapshot_id`` accepts a 64-hex ``str`` directly (for convenience) or a
    :class:`SnapshotId` instance.  I/O-bound async operation.
    """
    ...
