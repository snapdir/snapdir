# snapdir — Python binding

Content-addressable directory snapshots for Python, powered by Rust.

## Installation

```
pip install snapdir
```

Requires Python 3.10+ (abi3 wheel — one wheel covers all CPython ≥ 3.10).

Supported platforms: Linux (x86-64, aarch64), macOS (x86-64, arm64).

## Quick start

```python
import asyncio
import snapdir
from snapdir import SnapshotId, StoreUri

async def main():
    # Compute the snapshot ID for a directory (no I/O to any store)
    snap_id = await snapdir.id("/path/to/project")
    print(snap_id)  # 64-char lowercase hex BLAKE3 digest

    # Push to a store (file://, s3://, gcs://, b2://)
    store = StoreUri("s3://my-bucket/snapshots")
    snap_id = await snapdir.push("/path/to/project", store)

    # Pull (fetch from store + materialise files)
    await snapdir.pull(SnapshotId(snap_id), store, "/path/to/restore")

asyncio.run(main())
```

## API reference

All I/O-bound operations are `async`. Sync helpers (`version`, `id_from_manifest`)
need no event loop.

### Async functions

| Function | Description |
|---|---|
| `await manifest(path, *, no_follow, absolute, exclude)` | Walk `path` and return a `Manifest` (entries + raw text). |
| `await id(path, *, no_follow, absolute, exclude)` | Compute the 64-hex snapshot ID without staging. |
| `await stage(path)` | Stage `path` in the local cache; returns snapshot ID. |
| `await push(path, store)` | Stage + upload to `store`; returns snapshot ID. |
| `await fetch(snapshot_id, store)` | Download snapshot from `store` into local cache. |
| `await pull(snapshot_id, store, dest)` | Fetch + materialise snapshot into `dest`. |
| `await checkout(snapshot_id, dest)` | Materialise from local cache into `dest`. |
| `await sync(snapshot_id, src, dst)` | Copy snapshot between two stores. |
| `await diff(opts)` | Compare two sets of stores; returns `list[DiffEntry]`. |
| `await verify(snapshot_id, store)` | Verify snapshot integrity in `store`. |

### Sync functions

```python
snapdir.version()               # "1.11.0"
snapdir.id_from_manifest(m)     # derive ID from an already-computed Manifest
```

### Value types

- **`SnapshotId(s)`** — validated 64-hex snapshot identifier.
- **`StoreUri(s)`** — validated store URI (`file://`, `s3://`, `gcs://`, `b2://`).
- **`DiffOptions.from_refs(from_uris, to_uris)`** — options object for `diff()`.

### Result types

- **`Manifest`** — `.entries: list[ManifestEntry]`, `.raw: str`.
- **`ManifestEntry`** — `.path`, `.path_type`, `.permissions`, `.checksum`, `.size`.
- **`DiffEntry`** — `.status` (`"A"` / `"D"` / `"M"` / `"="`), `.path`.

### Exceptions

All derive from `SnapdirError` (carries a `.code` string):

- `HashMismatchError` — content checksum mismatch.
- `StoreError` — store-level I/O / protocol error.
- `InFluxError` — snapshot still being written (concurrent modification).
- `CatalogError` — local catalog / cache integrity error.

## Diffing two snapshots

```python
import asyncio, snapdir
from snapdir import DiffOptions, SnapshotId, StoreUri

async def compare():
    store = StoreUri("s3://my-bucket/snapshots")
    opts = DiffOptions.from_refs(
        [f"{store}@{id_a}"],
        [f"{store}@{id_b}"],
    )
    for entry in await snapdir.diff(opts):
        print(entry.status, entry.path)

asyncio.run(compare())
```

## Links

- Repository: <https://github.com/snapdir/snapdir>
- Documentation: <https://snapdir.org/docs>
- PyPI: <https://pypi.org/project/snapdir>
