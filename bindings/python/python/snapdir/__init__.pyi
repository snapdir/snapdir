"""Type stub for the snapdir package public surface.

The package re-exports every symbol from the compiled Rust extension
(``snapdir.snapdir``) and wraps the async functions via ``_lazy_async``
so they return native Python coroutines without a running event loop at
call time.  The wrapped names have the same type signatures as the
originals.
"""

import os

# Re-export all public names so ``import snapdir; snapdir.manifest`` etc. type-check.
from .snapdir import (
    # exceptions
    CatalogError as CatalogError,
    HashMismatchError as HashMismatchError,
    InFluxError as InFluxError,
    SnapdirError as SnapdirError,
    StoreError as StoreError,
    # result types
    DiffEntry as DiffEntry,
    Manifest as Manifest,
    ManifestEntry as ManifestEntry,
    PathType as PathType,
    # value types
    DiffOptions as DiffOptions,
    SnapshotId as SnapshotId,
    StoreUri as StoreUri,
    # sync functions
    id_from_manifest as id_from_manifest,
    version as version,
)

# Async functions — _lazy_async wraps preserve the coroutine signature.

async def manifest(
    path: str | os.PathLike[str],
    *,
    no_follow: bool = ...,
    absolute: bool = ...,
    exclude: list[str] | None = ...,
) -> Manifest: ...

async def id(
    path: str | os.PathLike[str],
    *,
    no_follow: bool = ...,
    absolute: bool = ...,
    exclude: list[str] | None = ...,
) -> str: ...

async def stage(path: str | os.PathLike[str]) -> str: ...

async def push(path: str | os.PathLike[str], store: StoreUri) -> str: ...

async def fetch(snapshot_id: SnapshotId, store: StoreUri) -> None: ...

async def pull(
    snapshot_id: SnapshotId,
    store: StoreUri,
    dest: str | os.PathLike[str],
) -> None: ...

async def checkout(
    snapshot_id: SnapshotId,
    dest: str | os.PathLike[str],
) -> None: ...

async def sync(
    snapshot_id: SnapshotId,
    src: StoreUri,
    dst: StoreUri,
) -> None: ...

async def diff(opts: DiffOptions) -> list[DiffEntry]: ...

async def verify(
    snapshot_id: SnapshotId | str,
    store: StoreUri,
) -> bool: ...

__all__ = [
    # exceptions
    "SnapdirError",
    "HashMismatchError",
    "StoreError",
    "InFluxError",
    "CatalogError",
    # result types
    "PathType",
    "ManifestEntry",
    "Manifest",
    "DiffEntry",
    # value types
    "SnapshotId",
    "StoreUri",
    "DiffOptions",
    # sync functions
    "version",
    "id_from_manifest",
    # async functions
    "manifest",
    "id",
    "stage",
    "push",
    "fetch",
    "pull",
    "checkout",
    "sync",
    "diff",
    "verify",
]
