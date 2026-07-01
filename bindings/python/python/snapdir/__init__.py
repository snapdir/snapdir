"""snapdir — Python binding for content-addressable directory snapshots.

Re-exports every symbol from the compiled Rust extension (``snapdir.snapdir``).
Async Rust functions (``manifest``, ``id``, ``stage``, ``push``, ``fetch``,
``pull``, ``checkout``, ``sync``, ``diff``, ``verify``) are wrapped so that
calling them returns a native Python coroutine object *without* requiring a
running asyncio event loop at call time.  The underlying Rust future is only
scheduled once the coroutine is actually awaited inside a running loop.
"""

from __future__ import annotations

import functools

# Import everything from the compiled Rust extension.
from .snapdir import *  # noqa: F401, F403
from .snapdir import (  # explicit re-export for type checkers
    CatalogError,
    DiffEntry,
    DiffOptions,
    HashMismatchError,
    InFluxError,
    Manifest,
    ManifestEntry,
    PathType,
    SnapdirError,
    SnapshotId,
    StoreError,
    StoreUri,
    id_from_manifest,
    version,
)

# Keep references to the raw Rust async functions before wrapping.
from .snapdir import (
    checkout as _rs_checkout,
    diff as _rs_diff,
    fetch as _rs_fetch,
    id as _rs_id,
    manifest as _rs_manifest,
    pull as _rs_pull,
    push as _rs_push,
    stage as _rs_stage,
    sync as _rs_sync,
    verify as _rs_verify,
)

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


def _lazy_async(fn):
    """Wrap a Rust async function so it is callable without a running event loop.

    The wrapped function returns a native Python coroutine object immediately.
    The underlying Rust future (which requires an asyncio task context) is only
    invoked when the coroutine is awaited inside a running event loop.
    """
    @functools.wraps(fn)
    async def wrapper(*args, **kwargs):
        return await fn(*args, **kwargs)
    return wrapper


manifest = _lazy_async(_rs_manifest)
id = _lazy_async(_rs_id)
stage = _lazy_async(_rs_stage)
push = _lazy_async(_rs_push)
fetch = _lazy_async(_rs_fetch)
pull = _lazy_async(_rs_pull)
checkout = _lazy_async(_rs_checkout)
sync = _lazy_async(_rs_sync)
diff = _lazy_async(_rs_diff)
verify = _lazy_async(_rs_verify)
